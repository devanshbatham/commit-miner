#![cfg(unix)]
use commit_miner::store::Store;
use std::{
    fs,
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

struct Running(Child);
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn wait_until(mut ready: impl FnMut() -> bool) {
    let start = Instant::now();
    while !ready() {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "fixture timed out"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn alive(pid: i32) -> bool {
    #[cfg(target_os = "linux")]
    if let Ok(s) = fs::read_to_string(format!("/proc/{pid}/stat")) {
        return s
            .rsplit_once(") ")
            .is_some_and(|(_, tail)| !tail.starts_with('Z'));
    }
    unsafe { libc::kill(pid, 0) == 0 }
}
fn signal(child: &Child) {
    assert_eq!(unsafe { libc::kill(-(child.id() as i32), libc::SIGINT) }, 0);
}
fn spawn(root: &Path, source: &str, shim: &str) -> Running {
    fs::create_dir_all(root.join("bin")).unwrap();
    let git = root.join("bin/git");
    fs::write(&git, shim).unwrap();
    fs::set_permissions(&git, fs::Permissions::from_mode(0o755)).unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_commit-miner"))
        .args(["--plain", "--data-dir"])
        .arg(root.join("data"))
        .args(["scan", source, "-n", "2", "--workers", "1"])
        .env(
            "PATH",
            format!(
                "{}:{}",
                root.join("bin").display(),
                std::env::var("PATH").unwrap()
            ),
        )
        .env("TYPESAFE_API_KEY", "interrupt-fixture-no-network")
        .env("JEV_MODEL", "jev-latest")
        .env("FIXTURE_ROOT", root)
        .stdout(Stdio::null())
        .stderr(fs::File::create(root.join("stderr")).unwrap())
        .process_group(0)
        .spawn()
        .unwrap();
    Running(child)
}
const BLOCK_CLONE: &str = r#"#!/bin/sh
for destination do :; done
mkdir -p "$destination"
trap '' INT TERM
sleep 30 &
printf '%s\n' "$!" > "$FIXTURE_ROOT/helper"
printf '%s\n' "$$" > "$FIXTURE_ROOT/ready"
wait
"#;
#[test]
fn ctrl_c_stops_clone_helpers_and_removes_partial_clone() {
    let tmp = tempfile::tempdir().unwrap();
    let mut process = spawn(tmp.path(), "https://github.com/fixture/repo", BLOCK_CLONE);
    wait_until(|| tmp.path().join("ready").exists());
    let helper: i32 = fs::read_to_string(tmp.path().join("helper"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let start = Instant::now();
    signal(&process.0);
    let mut status = None;
    wait_until(|| {
        status = process.0.try_wait().unwrap();
        status.is_some()
    });
    assert_eq!(status.unwrap().code(), Some(130));
    assert!(start.elapsed() < Duration::from_secs(2));
    wait_until(|| !alive(helper));
    let store = Store::new(tmp.path().join("data")).unwrap();
    let scans = store.list().unwrap();
    assert_eq!(scans.len(), 1);
    assert_eq!(scans[0].summary.status, "cancelled");
    assert_eq!(scans[0].summary.progress.active, 0);
    assert_eq!(
        fs::read_dir(tmp.path().join("data/repos")).unwrap().count(),
        0
    );
}
#[test]
fn second_ctrl_c_forces_exit_and_kills_helpers() {
    let tmp = tempfile::tempdir().unwrap();
    let mut process = spawn(tmp.path(), "https://github.com/fixture/repo", BLOCK_CLONE);
    wait_until(|| tmp.path().join("ready").exists());
    let helper: i32 = fs::read_to_string(tmp.path().join("helper"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    signal(&process.0);
    std::thread::sleep(Duration::from_millis(50));
    let start = Instant::now();
    signal(&process.0);
    let mut status = None;
    wait_until(|| {
        status = process.0.try_wait().unwrap();
        status.is_some()
    });
    assert_eq!(status.unwrap().code(), Some(130));
    assert!(start.elapsed() < Duration::from_millis(400));
    wait_until(|| !alive(helper));
    assert_eq!(
        Store::new(tmp.path().join("data"))
            .unwrap()
            .list()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn interrupt_preserves_completed_commits_during_next_diff() {
    use commit_miner::{git, miner, model::hash, store};
    use serde_json::json;
    use tokio_util::sync::CancellationToken;
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo).unwrap();
    let git_cmd = |args: &[&str]| {
        let result = Command::new("/usr/bin/git")
            .arg("-C")
            .arg(&repo)
            .args(args)
            .output()
            .unwrap();
        assert!(result.status.success());
        String::from_utf8(result.stdout).unwrap().trim().to_string()
    };
    git_cmd(&["init", "-q"]);
    git_cmd(&["config", "user.name", "Test"]);
    git_cmd(&["config", "user.email", "test@example.invalid"]);
    fs::write(repo.join("main.rs"), "fn main() {}\n").unwrap();
    git_cmd(&["add", "."]);
    git_cmd(&["-c", "core.hooksPath=/dev/null", "commit", "-qm", "small"]);
    let first = git_cmd(&["rev-parse", "HEAD"]);
    fs::write(repo.join("main.rs"), "fn action() {}\n".repeat(100)).unwrap();
    git_cmd(&["add", "."]);
    git_cmd(&["-c", "core.hooksPath=/dev/null", "commit", "-qm", "large"]);
    let second = git_cmd(&["rev-parse", "HEAD"]);
    fs::write(tmp.path().join("slowsha"), &second).unwrap();
    let cancel = CancellationToken::new();
    let commit = git::commit(&repo, &first, &cancel).await.unwrap();
    let (evidence, _) = git::evidence(&repo, &commit, &cancel).await.unwrap();
    let body = miner::request(
        &commit,
        &evidence,
        evidence.len(),
        "complete_commit",
        "jev-latest",
    );
    let answers = body["questions"]
        .as_object()
        .unwrap()
        .keys()
        .map(|k| (k.clone(), json!({"type":"noul","noul":0.05})))
        .collect::<serde_json::Map<_, _>>();
    let key = hash(
        serde_json::to_vec(&json!({"version":"rust-commit-evidence-v1","body":body})).unwrap(),
    );
    let cache = tmp.path().join("data/commit-cache");
    store::private_dir(&cache).unwrap();
    store::atomic_json(&cache.join(format!("{key}.json")), &json!({"saved":chrono::Utc::now().timestamp_millis(),"response":{"model":"fixture","answers":answers}})).unwrap();
    let shim = r#"#!/bin/sh
slow=$(cat "$FIXTURE_ROOT/slowsha")
case " $* " in
  *" diff "*"$slow"*)
    trap '' INT TERM
    sleep 30 &
    printf '%s\n' "$!" > "$FIXTURE_ROOT/helper"
    printf '%s\n' "$$" > "$FIXTURE_ROOT/ready"
    wait
    exit 1
    ;;
esac
exec /usr/bin/git "$@"
"#;
    let mut process = spawn(tmp.path(), repo.to_str().unwrap(), shim);
    wait_until(|| tmp.path().join("ready").exists());
    let store = Store::new(tmp.path().join("data")).unwrap();
    wait_until(|| {
        store
            .list()
            .unwrap()
            .first()
            .is_some_and(|s| s.summary.progress.classified == 1)
    });
    signal(&process.0);
    let mut status = None;
    wait_until(|| {
        status = process.0.try_wait().unwrap();
        status.is_some()
    });
    assert_eq!(status.unwrap().code(), Some(130));
    let scan = store.complete(store.list().unwrap().remove(0)).unwrap();
    assert_eq!(scan.summary.status, "cancelled");
    assert_eq!(scan.summary.progress.classified, 1);
    assert_eq!(scan.summary.progress.calls, 0);
    assert_eq!(scan.results.len(), 2);
    assert_eq!(scan.results[0].commit.sha, second); // Display retains history order.
    assert_eq!(scan.results[1].commit.sha, first);
    assert!(scan.results[1].reviewed());
    assert!(!scan.results[1].evidence.is_empty());
}
