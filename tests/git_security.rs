#![cfg(unix)]
use commit_miner::{git, model::*, store::Store};
use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Command, Stdio},
};
use tokio_util::sync::CancellationToken;

fn raw(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().into()
}
fn init(repo: &Path) {
    fs::create_dir_all(repo).unwrap();
    raw(repo, &["init", "-q"]);
    raw(repo, &["config", "user.name", "Fixture"]);
    raw(repo, &["config", "user.email", "fixture@example.invalid"]);
    fs::write(repo.join("main.rs"), "fn before() {}\n").unwrap();
    raw(repo, &["add", "."]);
    raw(
        repo,
        &[
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgSign=false",
            "commit",
            "-qm",
            "initial",
        ],
    );
    fs::write(repo.join("main.rs"), "fn after() {}\n").unwrap();
    raw(repo, &["add", "."]);
    raw(
        repo,
        &[
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgSign=false",
            "commit",
            "-qm",
            "change",
        ],
    );
}
fn payload(root: &Path) -> String {
    let path = root.join("payload");
    fs::write(
        &path,
        format!(
            "#!/bin/sh\nprintf executed > '{}'\nexit 1\n",
            root.join("executed").display()
        ),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path.to_str().unwrap().into()
}
fn options(repo: &Path) -> Options {
    Options {
        source: repo.to_str().unwrap().into(),
        commit: None,
        limit: Some(2),
        since: None,
        until: None,
        first_parent: false,
        threshold: 0.65,
        concurrency: 1,
        cache: false,
    }
}

#[tokio::test]
async fn shallow_local_history_never_executes_transport_commands() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    init(&repo);
    let head = raw(&repo, &["rev-parse", "HEAD"]);
    fs::write(repo.join(".git/shallow"), format!("{head}\n")).unwrap();
    let script = payload(tmp.path());
    raw(
        &repo,
        &["config", "remote.origin.url", "ssh://example.invalid/repo"],
    );
    raw(&repo, &["config", "core.sshCommand", &script]);
    let c = CancellationToken::new();
    for selection in ["count", "date", "missing", "boundary"] {
        let mut o = options(&repo);
        match selection {
            "date" => o.since = Some("2020-01-01".into()),
            "missing" => {
                o.limit = None;
                o.commit = Some("not-present".into());
            }
            "boundary" => {
                o.limit = None;
                o.commit = Some("HEAD".into());
            }
            _ => {}
        }
        assert!(git::history(&repo, &o, &c).await.is_err(), "{selection}");
        assert!(!tmp.path().join("executed").exists(), "{selection}");
    }
    // Exercise the real CLI too: no Jev request is needed to trigger this bug.
    let output = Command::new(env!("CARGO_BIN_EXE_commit-miner"))
        .args(["--plain", "--data-dir"])
        .arg(tmp.path().join("data"))
        .arg("scan")
        .arg(&repo)
        .args(["-n", "1"])
        .env("TYPESAFE_API_KEY", "security-fixture-no-network")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("local shallow boundary"));
    assert!(!tmp.path().join("executed").exists());
    let scans = Store::new(tmp.path().join("data")).unwrap().list().unwrap();
    assert_eq!(scans[0].summary.status, "failed");
}

#[tokio::test]
async fn signed_commits_and_diff_drivers_do_not_execute_helpers() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    init(&repo);
    let script = payload(tmp.path());
    let tree = raw(&repo, &["rev-parse", "HEAD^{tree}"]);
    let parent = raw(&repo, &["rev-parse", "HEAD~1"]);
    let object = format!(
        "tree {tree}\nparent {parent}\nauthor Fixture <fixture@example.invalid> 1700000000 +0000\ncommitter Fixture <fixture@example.invalid> 1700000000 +0000\ngpgsig -----BEGIN PGP SIGNATURE-----\n \n ZmFrZQ==\n -----END PGP SIGNATURE-----\n\nsigned fixture\n"
    );
    let mut child = Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["hash-object", "-w", "-t", "commit", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(object.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    let sha = String::from_utf8(out.stdout).unwrap().trim().to_owned();
    raw(&repo, &["update-ref", "HEAD", &sha]);
    for (key, value) in [
        ("log.showSignature", "true"),
        ("gpg.program", script.as_str()),
        ("gpg.ssh.program", script.as_str()),
        ("core.fsmonitor", script.as_str()),
        ("diff.external", script.as_str()),
        ("diff.evil.command", script.as_str()),
        ("diff.evil.textconv", script.as_str()),
        ("filter.evil.process", script.as_str()),
    ] {
        raw(&repo, &["config", key, value]);
    }
    fs::write(
        repo.join(".git/info/attributes"),
        "*.rs diff=evil filter=evil\n",
    )
    .unwrap();
    let c = CancellationToken::new();
    let ids = git::history(&repo, &options(&repo), &c).await.unwrap();
    assert_eq!(git::prioritize(&repo, &ids, &c).await.unwrap().len(), 2);
    let commit = git::commit(&repo, &sha, &c).await.unwrap();
    assert_eq!(commit.message, "signed fixture");
    let (evidence, warnings) = git::evidence(&repo, &commit, &c).await.unwrap();
    assert!(warnings.is_empty());
    assert!(
        evidence
            .iter()
            .flat_map(|e| &e.lines)
            .any(|l| l.text == "fn after() {}")
    );
    assert!(!tmp.path().join("executed").exists());
}

#[tokio::test]
async fn missing_partial_clone_objects_cannot_trigger_lazy_fetch_helpers() {
    for transport in ["ssh", "ext", "helper"] {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        init(&repo);
        let script = payload(tmp.path());
        let blob = raw(&repo, &["rev-parse", "HEAD:main.rs"]);
        let head = raw(&repo, &["rev-parse", "HEAD"]);
        raw(&repo, &["config", "remote.origin.promisor", "true"]);
        raw(&repo, &["config", "core.sshCommand", &script]);
        raw(&repo, &["config", "protocol.ext.allow", "always"]);
        let remote = match transport {
            "ext" => format!("ext::{script}"),
            "helper" => {
                raw(&repo, &["config", "remote.origin.vcs", "ext"]);
                script.clone()
            }
            _ => "ssh://example.invalid/repo".into(),
        };
        raw(&repo, &["config", "remote.origin.url", &remote]);
        fs::remove_file(repo.join(".git/objects").join(&blob[..2]).join(&blob[2..])).unwrap();
        let c = CancellationToken::new();
        let commit = git::commit(&repo, &head, &c).await.unwrap();
        assert!(git::prioritize(&repo, &[head], &c).await.is_err());
        let (_, warnings) = git::evidence(&repo, &commit, &c).await.unwrap();
        assert!(!warnings.is_empty(), "Missing evidence must be reported");
        assert!(!tmp.path().join("executed").exists(), "{transport}");
    }
}

#[tokio::test]
async fn poisoned_managed_clones_are_rejected_before_fetch() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("source");
    init(&source);
    let remote = "https://github.com/example/security-fixture.git";
    let root = tmp.path().join("data");
    let cached = root.join("repos").join(&hash(remote)[..20]);
    fs::create_dir_all(cached.parent().unwrap()).unwrap();
    raw(
        tmp.path(),
        &[
            "clone",
            "--no-checkout",
            source.to_str().unwrap(),
            cached.to_str().unwrap(),
        ],
    );
    raw(&cached, &["remote", "set-url", "origin", remote]);
    let script = payload(tmp.path());
    let include = tmp.path().join("included-config");
    fs::write(&include, format!("[credential]\n helper = !{script}\n")).unwrap();
    for (key, value) in [
        ("core.sshCommand", script.clone()),
        ("credential.helper", format!("!{script}")),
        ("credential.https://github.com.helper", format!("!{script}")),
        ("core.askPass", script.clone()),
        ("core.alternateRefsCommand", script.clone()),
        ("remote.origin.vcs", "ext".into()),
        ("include.path", include.to_str().unwrap().into()),
        (
            "url.ssh://example.invalid/.insteadOf",
            "https://github.com/".into(),
        ),
        ("extensions.worktreeConfig", "true".into()),
    ] {
        raw(&cached, &["config", key, &value]);
        let error = git::repository(remote, &root, 32, &CancellationToken::new())
            .await
            .unwrap_err();
        assert!(
            format!("{error:#}").contains("unsupported Git configuration"),
            "{key}: {error:#}"
        );
        assert!(!tmp.path().join("executed").exists(), "{key}");
        raw(&cached, &["config", "--unset-all", key]);
    }
}
