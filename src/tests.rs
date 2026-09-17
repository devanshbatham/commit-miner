use crate::{
    git,
    jev::{Event, Jev},
    miner,
    model::*,
    store::Store,
};
use serde_json::{Value, json};
use std::{path::Path, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::mpsc,
};
use tokio_util::sync::CancellationToken;
fn fixture() -> Commit {
    Commit {
        sha: "a".repeat(40),
        parents: vec!["b".repeat(40)],
        date: "2026-01-01T00:00:00Z".into(),
        committed_at: "2026-01-01T00:00:00Z".into(),
        author: "Tester".into(),
        message: "Optimize request handling".into(),
        files: vec!["src/access.rs".into()],
        merge: false,
        excluded_files: 0,
    }
}
fn options(source: String) -> Options {
    Options {
        source,
        commit: None,
        limit: Some(10),
        since: None,
        until: None,
        first_parent: false,
        threshold: 0.65,
        concurrency: 2,
        cache: true,
    }
}
async fn mock(statuses: Vec<u16>) -> (String, tokio::task::JoinHandle<Vec<Value>>) {
    mock_scores(
        statuses,
        vec!["t0_security_fix", "t0_cwe_862", "t0_e0_security"],
    )
    .await
}
async fn mock_scores(
    statuses: Vec<u16>,
    positive: Vec<&'static str>,
) -> (String, tokio::task::JoinHandle<Vec<Value>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let mut bodies = vec![];
        for status in statuses {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = vec![];
            let boundary = loop {
                let mut b = [0; 4096];
                let n = socket.read(&mut b).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&b[..n]);
                if let Some(i) = bytes.windows(4).position(|s| s == b"\r\n\r\n") {
                    break i + 4;
                }
            };
            let head = String::from_utf8_lossy(&bytes[..boundary]);
            let len = head
                .lines()
                .find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .and_then(|s| s.trim().parse::<usize>().ok())
                })
                .unwrap();
            while bytes.len() < boundary + len {
                let mut b = [0; 4096];
                let n = socket.read(&mut b).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&b[..n]);
            }
            let body: Value = serde_json::from_slice(&bytes[boundary..boundary + len]).unwrap();
            let answers=body["questions"].as_object().unwrap().keys().map(|id|(id.clone(),json!({"type":"noul","noul":if positive.contains(&id.as_str()){0.97}else{0.03}}))).collect::<serde_json::Map<_,_>>();
            let payload = json!({"model":"mock-jev","answers":answers,"usage":{"input_tokens":100,"output_tokens":3}}).to_string();
            socket.write_all(format!("HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nRetry-After: 0\r\nConnection: close\r\n\r\n{payload}",payload.len()).as_bytes()).await.unwrap();
            let final_review = status == 200
                && matches!(
                    body["state"]["reviews"][0]["coverage"]["stage"].as_str(),
                    Some("final_review" | "complete_commit" | "metadata_review")
                );
            bodies.push(body);
            if final_review {
                break;
            }
        }
        bodies
    });
    (url, task)
}
async fn parallel_mock() -> (String, tokio::task::JoinHandle<(Vec<Value>, usize)>) {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let (done, mut finished) = mpsc::unbounded_channel();
        let mut work = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                _ = finished.recv() => break,
                accepted = listener.accept() => {
                    let (mut socket,_) = accepted.unwrap();
                    let (bodies,active,peak,done) = (bodies.clone(),active.clone(),peak.clone(),done.clone());
                    work.spawn(async move {
                        let mut bytes = vec![];
                        let boundary = loop {
                            let mut chunk=[0;4096];let n=socket.read(&mut chunk).await.unwrap();assert!(n>0);bytes.extend_from_slice(&chunk[..n]);
                            if let Some(i)=bytes.windows(4).position(|p|p==b"\r\n\r\n"){break i+4;}
                        };
                        let head=String::from_utf8_lossy(&bytes[..boundary]);
                        let len=head.lines().find_map(|l|l.to_ascii_lowercase().strip_prefix("content-length:").and_then(|s|s.trim().parse::<usize>().ok())).unwrap();
                        while bytes.len()<boundary+len {let mut chunk=[0;4096];let n=socket.read(&mut chunk).await.unwrap();assert!(n>0);bytes.extend_from_slice(&chunk[..n]);}
                        let body:Value=serde_json::from_slice(&bytes[boundary..boundary+len]).unwrap();
                        peak.fetch_max(active.fetch_add(1,Ordering::SeqCst)+1,Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(40)).await;
                        let answers=body["questions"].as_object().unwrap().keys().map(|id|(id.clone(),json!({"type":"noul","noul":if ["t0_security_fix","t0_cwe_862","t0_e0_security"].contains(&id.as_str()){0.97}else{0.03}}))).collect::<serde_json::Map<_,_>>();
                        let payload=json!({"model":"mock-jev","answers":answers}).to_string();
                        let final_review=body["state"]["reviews"][0]["coverage"]["stage"]=="final_review";
                        bodies.lock().unwrap().push(body);
                        active.fetch_sub(1,Ordering::SeqCst);
                        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",payload.len()).as_bytes()).await.unwrap();
                        if final_review {done.send(()).unwrap();}
                    });
                }
            }
        }
        while let Some(result) = work.join_next().await {
            result.unwrap();
        }
        let output = bodies.lock().unwrap().clone();
        (output, peak.load(Ordering::SeqCst))
    });
    (url, task)
}
#[tokio::test]
async fn transport_retry_validation_and_cache() {
    let temp = tempfile::tempdir().unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let (url, server) = mock(vec![429, 200]).await;
    let j = Jev::new(
        "fixture-key".into(),
        "jev-latest".into(),
        temp.path().into(),
        true,
        4,
        tx,
    )
    .unwrap()
    .endpoint(url);
    let body = miner::request(&fixture(), &[], 0, "complete_commit", "jev-latest");
    let c = CancellationToken::new();
    let (response, cached) = j.evaluate(&body, &c).await.unwrap();
    assert!(!cached);
    assert_eq!(response["answers"]["t0_security_fix"]["noul"], 0.97);
    let (_, cached) = j.evaluate(&body, &c).await.unwrap();
    assert!(cached);
    let sent = server.await.unwrap();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0], sent[1]);
    let mut retries = 0;
    let mut usage = (0, 0);
    while let Ok(e) = rx.try_recv() {
        if let Event::Usage {
            input_tokens,
            output_tokens,
            ..
        } = e
        {
            usage.0 += input_tokens;
            usage.1 += output_tokens;
        }
        if matches!(e, Event::Retry { status: 429, .. }) {
            retries += 1;
        }
    }
    assert_eq!(retries, 1);
    assert_eq!(
        usage,
        (100, 3),
        "Cached responses must not count as paid usage"
    );
    let mut invalid = response;
    invalid["answers"]["t0_security_fix"]["noul"] = json!(1.5);
    assert!(crate::jev::validate(&body, &invalid).is_err());
}
#[tokio::test]
async fn auth_failure_does_not_retry() {
    let temp = tempfile::tempdir().unwrap();
    let (tx, _) = mpsc::unbounded_channel();
    let (url, server) = mock(vec![401]).await;
    let j = Jev::new(
        "secret-key".into(),
        "jev-latest".into(),
        temp.path().into(),
        false,
        1,
        tx,
    )
    .unwrap()
    .endpoint(url);
    let e = j
        .evaluate(
            &miner::request(&fixture(), &[], 0, "complete_commit", "jev-latest"),
            &CancellationToken::new(),
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("rejected"));
    assert!(!e.contains("secret-key"));
    assert_eq!(server.await.unwrap().len(), 1);
}
#[tokio::test]
async fn cancellation_interrupts_inflight_request() {
    let temp = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let (tx, _) = mpsc::unbounded_channel();
    let j = Jev::new(
        "key".into(),
        "jev-latest".into(),
        temp.path().into(),
        false,
        1,
        tx,
    )
    .unwrap()
    .endpoint(format!("http://{}", listener.local_addr().unwrap()));
    let c = CancellationToken::new();
    let stop = c.clone();
    tokio::spawn(async move {
        let _socket = listener.accept().await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        stop.cancel();
    });
    let start = std::time::Instant::now();
    assert!(
        j.evaluate(
            &miner::request(&fixture(), &[], 0, "complete_commit", "jev-latest"),
            &c
        )
        .await
        .is_err()
    );
    assert!(start.elapsed() < Duration::from_secs(2));
}
fn cmd(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
fn init(dir: &Path) {
    cmd(dir, &["init", "-q"]);
    cmd(dir, &["config", "user.name", "Test"]);
    cmd(dir, &["config", "user.email", "test@example.invalid"]);
}
fn save(dir: &Path, message: &str, date: &str) {
    cmd(dir, &["add", "."]);
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "core.hooksPath=/dev/null", "commit", "-qm", message])
        .env("GIT_AUTHOR_DATE", "2020-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", date)
        .output()
        .unwrap();
    assert!(out.status.success());
}
#[tokio::test]
async fn local_history_dates_literal_paths_and_model_diff() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    init(&repo);
    let file = "[access].rs";
    std::fs::write(repo.join(file), "fn update() { save(); }\n").unwrap();
    save(&repo, "initial", "2026-02-10T10:00:00Z");
    std::fs::write(repo.join(file), "fn update() { if owner() { save(); } }\n").unwrap();
    save(&repo, "Optimize handling", "2026-01-10T10:00:00Z");
    std::fs::write(repo.join("README.md"), "notes").unwrap();
    save(&repo, "docs", "2026-03-01T10:00:00Z");
    let c = CancellationToken::new();
    let mut o = options(repo.to_string_lossy().into());
    let actual = git::repository(&o.source, temp.path(), 32, &c)
        .await
        .unwrap();
    assert_eq!(actual, repo);
    o.since = Some("2026-02-01".into());
    o.until = Some("2026-02-28".into());
    o.limit = None;
    let ids = git::history(&repo, &o, &c).await.unwrap();
    assert_eq!(ids.len(), 1);
    assert_eq!(
        git::commit(&repo, &ids[0], &c).await.unwrap().message,
        "initial"
    );
    o.since = Some("2026-01-10".into());
    o.until = o.since.clone();
    let ids = git::history(&repo, &o, &c).await.unwrap();
    assert_eq!(ids.len(), 1);
    let (tx, mut rx) = mpsc::unbounded_channel();
    let (url, server) = mock(vec![200]).await;
    let j = Jev::new(
        "key".into(),
        "jev-latest".into(),
        temp.path().into(),
        true,
        2,
        tx.clone(),
    )
    .unwrap()
    .endpoint(url);
    miner::mine(&repo, &ids[0], &o, &j, &c, &tx).await.unwrap();
    let sent = server.await.unwrap();
    let review = &sent[0]["state"]["reviews"][0];
    assert_eq!(review["commit"]["message"], "Optimize handling");
    let diff = review["diff_sections"][0]["diff"].as_str().unwrap();
    assert!(diff.lines().any(|line| line == "-fn update() { save(); }"));
    assert!(
        diff.lines()
            .any(|line| line == "+fn update() { if owner() { save(); } }")
    );
    let mut result = None;
    while let Ok(event) = rx.try_recv() {
        if let Event::Commit(r) = event {
            result = Some(*r);
        }
    }
    let r = result.unwrap();
    assert_eq!(r.categories.len(), 1);
    assert_eq!(r.categories[0].id, "cwe_862");
    assert_eq!(r.commit.files, vec![file]);
    let store = Store::new(temp.path().join("data")).unwrap();
    let mut scan = store.create(o).unwrap();
    store.save_commit(&scan.summary.id, &r).unwrap();
    scan.results.push(r.clone());
    scan.results[0].evidence.clear();
    store.save(&scan).unwrap();
    let report = store
        .complete(store.get(&scan.summary.id).unwrap())
        .unwrap();
    assert_eq!(report.results[0].evidence.len(), r.evidence.len());
}
#[test]
fn calendar_and_git_url_validation() {
    assert!(git::date("2026-02-30").is_err());
    assert!(git::date("2026-2-01").is_err());
    assert!(git::date("2024-02-29").is_ok());
    for bad in [
        "https://evil.invalid/a/b",
        "https://user:pass@github.com/a/b",
        "https://github.com/a/b?x=y",
        "file:///tmp/repo",
    ] {
        assert!(git::github_url(bad).is_err());
    }
    assert_eq!(
        git::github_url("https://github.com/owner/repo/").unwrap(),
        "https://github.com/owner/repo.git"
    );
}
#[tokio::test]
async fn dependency_version_bumps_reach_jev_and_the_table() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    init(&repo);
    for version in ["1.0.0", "1.1.0"] {
        std::fs::write(
            repo.join("package.json"),
            format!("{{\"dependencies\":{{\"example\":\"{version}\"}}}}\n"),
        )
        .unwrap();
        std::fs::write(
            repo.join("yarn.lock"),
            format!("example@{version}:\n  version \"{version}\"\n"),
        )
        .unwrap();
        save(&repo, "Maintenance", "2026-01-01T00:00:00Z");
    }
    let c = CancellationToken::new();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let (url, server) = mock_scores(
        vec![200; 10],
        vec!["t0_change_dependency", "t0_e0_change", "t0_e1_change"],
    )
    .await;
    let j = Jev::new(
        "key".into(),
        "jev-latest".into(),
        temp.path().into(),
        false,
        2,
        tx.clone(),
    )
    .unwrap()
    .endpoint(url);
    miner::mine(
        &repo,
        "HEAD",
        &options(repo.to_string_lossy().into()),
        &j,
        &c,
        &tx,
    )
    .await
    .unwrap();
    let sent = server.await.unwrap();
    let review = &sent[0]["state"]["reviews"][0];
    assert_eq!(review["coverage"]["metadata_only"], false);
    for path in ["package.json", "yarn.lock"] {
        let section = review["diff_sections"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["path"] == path)
            .unwrap();
        let diff = section["diff"].as_str().unwrap();
        assert!(
            diff.lines()
                .any(|l| l.starts_with('-') && l.contains("1.0.0"))
        );
        assert!(
            diff.lines()
                .any(|l| l.starts_with('+') && l.contains("1.1.0"))
        );
    }
    let mut result = None;
    while let Ok(event) = rx.try_recv() {
        if let Event::Commit(r) = event {
            result = Some(*r);
        }
    }
    let mut result = result.unwrap();
    assert_eq!(result.primary_classification(0.65).0, "Dependency");
    let terminal = crate::terminal::Terminal {
        color: false,
        width: 100,
    };
    assert!(terminal.table(&[&result], 0.65).contains("Dependency"));
    result.probabilities.insert("bug_fix".into(), 0.9);
    assert_eq!(result.primary_classification(0.65).0, "Bug fix");
    result.probabilities.insert("cwe_400".into(), 0.9);
    assert_eq!(result.primary_classification(0.65).0, "Security fix");
}
#[test]
fn diff_coordinates() {
    let h = git::parse_diff(
        "diff --git a/a.rs b/a.rs\n@@ -4,2 +4,3 @@\n keep\n-old\n+new\n+more\n\\ No newline at end of file\n",
    );
    let l = &h[0].1;
    assert_eq!(l[1].old_line, Some(5));
    assert_eq!(l[1].new_line, None);
    assert_eq!(l[3].new_line, Some(6));
    assert_eq!(l[4].kind, "meta");
}
#[tokio::test]
async fn large_commit_reviews_every_section_then_jev_selected_final_review() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    init(&repo);
    let source = (0..100)
        .map(|i| format!("fn action_{i}() {{ persist(\"{}\"); }}\n", "a".repeat(70)))
        .collect::<String>();
    std::fs::write(repo.join("main.rs"), &source).unwrap();
    save(&repo, "initial", "2026-01-01T00:00:00Z");
    std::fs::write(
        repo.join("main.rs"),
        source.replace("persist(", "check_owner(); persist("),
    )
    .unwrap();
    save(&repo, "update", "2026-01-02T00:00:00Z");
    let c = CancellationToken::new();
    let sha = git::run(Some(&repo), &["rev-parse", "HEAD"], &c, 1000)
        .await
        .unwrap();
    let sha = sha.trim();
    let commit = git::commit(&repo, sha, &c).await.unwrap();
    let (evidence, warnings) = git::evidence(&repo, &commit, &c).await.unwrap();
    assert!(warnings.is_empty());
    assert!(evidence.len() > 3);
    let (tx, mut rx) = mpsc::unbounded_channel();
    let (url, server) = parallel_mock().await;
    let j = Jev::new(
        "key".into(),
        "jev-latest".into(),
        temp.path().into(),
        false,
        2,
        tx.clone(),
    )
    .unwrap()
    .endpoint(url);
    tokio::time::timeout(
        Duration::from_secs(10),
        miner::mine(
            &repo,
            sha,
            &options(repo.to_string_lossy().into()),
            &j,
            &c,
            &tx,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let (sent, peak) = server.await.unwrap();
    assert_eq!(
        peak, 2,
        "Independent sections should use both workers without exceeding the shared limit"
    );
    assert!(sent.len() > 2);
    // Sections share requests and ask only the support questions consumed by selection.
    assert!(sent.len() < evidence.len());
    let last = sent.last().unwrap();
    assert_eq!(
        last["state"]["reviews"][0]["coverage"]["stage"],
        "final_review"
    );
    let mut ids = std::collections::BTreeSet::new();
    for body in &sent[..sent.len() - 1] {
        assert!(serde_json::to_vec(body).unwrap().len() <= crate::jev::REQUEST_BUDGET);
        let section_count = body["state"]["reviews"][0]["diff_sections"]
            .as_array()
            .unwrap()
            .len();
        assert_eq!(
            body["questions"].as_object().unwrap().len(),
            section_count * 3
        );
        for e in body["state"]["reviews"][0]["diff_sections"]
            .as_array()
            .unwrap()
        {
            ids.insert(e["id"].as_str().unwrap().to_string());
        }
    }
    assert_eq!(ids, evidence.iter().map(|e| e.id.clone()).collect());
    assert!(
        last["state"]["reviews"][0]["diff_sections"]
            .as_array()
            .unwrap()
            .len()
            < evidence.len()
    );
    let mut completed = None;
    while let Ok(e) = rx.try_recv() {
        if let Event::Commit(r) = e {
            completed = Some(r);
        }
    }
    let result = completed.unwrap();
    assert_eq!(result.evaluated, evidence.len());
    assert_eq!(
        result
            .evidence
            .iter()
            .map(|s| &s.evidence.id)
            .collect::<Vec<_>>(),
        evidence.iter().map(|e| &e.id).collect::<Vec<_>>()
    );
    assert_eq!(result.review_coverage, "selected");
    assert_eq!(result.probabilities["security_fix"], 0.97);
}

#[tokio::test]
async fn small_commits_are_prioritized_without_losing_history() {
    let temp = tempfile::tempdir().unwrap();
    init(temp.path());
    std::fs::write(temp.path().join("main.rs"), "fn main() {}\n").unwrap();
    save(temp.path(), "small", "2026-01-01T00:00:00Z");
    std::fs::write(temp.path().join("main.rs"), "fn task() {}\n".repeat(100)).unwrap();
    save(temp.path(), "large", "2026-01-02T00:00:00Z");
    std::fs::write(temp.path().join("README.md"), "notes\n".repeat(1000)).unwrap();
    save(temp.path(), "docs", "2026-01-03T00:00:00Z");
    let c = CancellationToken::new();
    let ids = git::history(
        temp.path(),
        &options(temp.path().to_string_lossy().into()),
        &c,
    )
    .await
    .unwrap();
    let ordered = git::prioritize(temp.path(), &ids, &c).await.unwrap();
    assert_eq!(
        ordered,
        vec![ids[0].clone(), ids[2].clone(), ids[1].clone()]
    );
    assert_eq!(
        git::history(
            temp.path(),
            &options(temp.path().to_string_lossy().into()),
            &c
        )
        .await
        .unwrap(),
        ids
    );
}
#[tokio::test]
async fn linked_worktree_and_subdirectory_use_their_own_head() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    init(&repo);
    std::fs::write(repo.join("a.rs"), "fn main() {}\n").unwrap();
    save(&repo, "initial", "2026-01-01T00:00:00Z");
    let worktree = temp.path().join("worktree");
    cmd(
        &repo,
        &["worktree", "add", "-b", "work", worktree.to_str().unwrap()],
    );
    std::fs::create_dir(worktree.join("sub")).unwrap();
    std::fs::write(worktree.join("sub/b.rs"), "fn b() {}\n").unwrap();
    save(&worktree, "worktree commit", "2026-01-02T00:00:00Z");
    let c = CancellationToken::new();
    let root = git::repository(worktree.join("sub").to_str().unwrap(), temp.path(), 32, &c)
        .await
        .unwrap();
    assert_eq!(root, worktree);
    let ids = git::history(&root, &options(root.to_string_lossy().into()), &c)
        .await
        .unwrap();
    assert_eq!(
        git::commit(&root, &ids[0], &c).await.unwrap().message,
        "worktree commit"
    );
    let original = git::history(&repo, &options(repo.to_string_lossy().into()), &c)
        .await
        .unwrap();
    assert_eq!(original.len(), 1);
}
#[test]
fn jev_schema_questions_carry_cwe_semantics_and_reject_invalid_answers() {
    let body = miner::request(&fixture(), &[], 0, "complete_commit", "jev-latest");
    crate::jev::validate_request(&body).unwrap();
    assert!(
        body["questions"]["t0_cwe_862"]["instructions"]
            .as_str()
            .unwrap()
            .contains("CWE-862")
    );
    assert!(
        body["questions"]["t0_cwe_862"]["instructions"]
            .as_str()
            .unwrap()
            .contains("Missing authorization")
    );
    let answers = body["questions"]
        .as_object()
        .unwrap()
        .keys()
        .map(|id| (id.clone(), json!({"type":"noul","noul":0.9})))
        .collect::<serde_json::Map<_, _>>();
    let response = json!({"model":"jev-latest","answers":answers,"usage":{"input_tokens":300,"output_tokens":50}});
    crate::jev::validate(&body, &response).unwrap();
    let mut bad = response.clone();
    bad["answers"].as_object_mut().unwrap().remove("t0_cwe_862");
    assert!(crate::jev::validate(&body, &bad).is_err());
    let mut bad = response.clone();
    bad["answers"]["t0_cwe_862"] = json!({"type":"choice","choice":"yes","confidence":0.99});
    assert!(crate::jev::validate(&body, &bad).is_err());
    let mut bad = response;
    bad["usage"]["input_tokens"] = json!(-1);
    assert!(crate::jev::validate(&body, &bad).is_err());
    let mut bad = body;
    bad["state"] = json!(null);
    assert!(crate::jev::validate_request(&bad).is_err());
}

#[test]
fn compact_schema_preserves_diff_text_and_final_classification_questions() {
    let commit = fixture();
    let evidence = git::metadata_sections(
        &commit.sha,
        "src/access.rs",
        "λ <tag> \"quoted\"\\n\nline two",
    )
    .unwrap();
    let full = miner::request(
        &commit,
        &evidence,
        evidence.len(),
        "complete_commit",
        "jev-latest",
    );
    let sections = miner::request(
        &commit,
        &evidence,
        evidence.len(),
        "section_review",
        "jev-latest",
    );
    let final_review = miner::request(
        &commit,
        &evidence,
        evidence.len(),
        "final_review",
        "jev-latest",
    );
    assert_eq!(
        full["state"]["reviews"][0]["diff_sections"],
        final_review["state"]["reviews"][0]["diff_sections"]
    );
    for (i, original) in evidence.iter().enumerate() {
        let wire = &full["state"]["reviews"][0]["diff_sections"][i];
        assert_eq!(wire["path"], original.path);
        for line in &original.lines {
            assert!(wire["diff"].as_str().unwrap().contains(&line.text));
        }
    }
    assert_eq!(
        sections["questions"].as_object().unwrap().len(),
        evidence.len() * 3
    );
    assert_eq!(
        final_review["questions"].as_object().unwrap().len(),
        taxonomy().categories.len() + taxonomy().core_questions.len()
    );
    for (id, question) in final_review["questions"].as_object().unwrap() {
        assert_eq!(full["questions"][id], *question);
    }
}

#[tokio::test]
async fn oversized_questions_share_state_without_losing_answers() {
    let temp = tempfile::tempdir().unwrap();
    let (tx, _) = mpsc::unbounded_channel();
    let (url, server) = mock(vec![200, 200]).await;
    let j = Jev::new(
        "key".into(),
        "jev-latest".into(),
        temp.path().into(),
        true,
        8,
        tx,
    )
    .unwrap()
    .endpoint(url);
    let body = json!({"model":"jev-latest", "state":{"context":"x".repeat(8000)},
        "questions":{"first":{"type":"noul","instructions":"a".repeat(10000)},
                     "second":{"type":"noul","instructions":"b".repeat(10000)}}});
    let (answer, cached) = j.evaluate(&body, &CancellationToken::new()).await.unwrap();
    assert!(!cached);
    assert_eq!(answer["answers"].as_object().unwrap().len(), 2);
    let sent = server.await.unwrap();
    assert_eq!(sent.len(), 2);
    for request in sent {
        assert_eq!(request["state"], body["state"]);
        assert!(serde_json::to_vec(&request).unwrap().len() <= crate::jev::REQUEST_BUDGET);
    }
    let (again, cached) = j.evaluate(&body, &CancellationToken::new()).await.unwrap();
    assert!(cached);
    assert_eq!(again, answer);
}

#[tokio::test]
async fn diffs_exceeding_old_file_and_commit_caps_keep_their_tails() {
    let temp = tempfile::tempdir().unwrap();
    init(temp.path());
    for i in 0..3 {
        let source = format!(
            "{}\nfn tail_{i}() {{}}\n",
            "let value = 1;\n".repeat(215_000)
        );
        assert!(source.len() > 2 * 1024 * 1024);
        std::fs::write(temp.path().join(format!("part_{i}.rs")), source).unwrap();
    }
    save(temp.path(), "large initial commit", "2026-01-01T00:00:00Z");
    let c = CancellationToken::new();
    let commit = git::commit(temp.path(), "HEAD", &c).await.unwrap();
    let (evidence, warnings) = git::evidence(temp.path(), &commit, &c).await.unwrap();
    assert!(warnings.is_empty());
    for i in 0..3 {
        assert!(evidence.iter().any(|e| {
            e.path == format!("part_{i}.rs")
                && e.lines
                    .iter()
                    .any(|l| l.text == format!("fn tail_{i}() {{}}") && l.new_line == Some(215_002))
        }));
    }
}

#[tokio::test]
async fn merge_history_and_repository_activity_are_reported() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    init(&repo);
    std::fs::write(repo.join("a.rs"), "fn a() {}\n").unwrap();
    save(&repo, "initial", "2026-01-01T00:00:00Z");
    for i in 0..5 {
        std::fs::write(repo.join("a.rs"), format!("fn a_{i}() {{}}\n")).unwrap();
        save(&repo, "older history", "2026-01-01T01:00:00Z");
    }
    cmd(&repo, &["checkout", "-qb", "feature"]);
    std::fs::write(repo.join("b.rs"), "fn b() {}\n").unwrap();
    save(&repo, "feature", "2026-01-02T00:00:00Z");
    cmd(&repo, &["checkout", "-qb", "trunk", "HEAD~1"]);
    std::fs::write(repo.join("c.rs"), "fn c() {}\n").unwrap();
    save(&repo, "trunk", "2026-01-03T00:00:00Z");
    cmd(
        &repo,
        &[
            "-c",
            "core.hooksPath=/dev/null",
            "merge",
            "--no-ff",
            "feature",
            "-m",
            "merge",
        ],
    );
    let events = std::sync::Mutex::new(Vec::new());
    let progress =
        |event: git::GitProgress<'_>| events.lock().unwrap().push(event.text().to_string());
    let c = CancellationToken::new();
    git::repository_with_progress(repo.to_str().unwrap(), temp.path(), 32, &c, &progress)
        .await
        .unwrap();
    let ids = git::history_with_progress(
        &repo,
        &options(repo.to_string_lossy().into()),
        &c,
        &progress,
    )
    .await
    .unwrap();
    assert_eq!(ids.len(), 9);
    assert!(git::commit(&repo, &ids[0], &c).await.unwrap().merge);
    assert!(
        events
            .lock()
            .unwrap()
            .iter()
            .any(|s| s.contains("Opening local"))
    );
    // Serve real Git history locally; URL rewriting keeps this test offline.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let exec_path = std::process::Command::new("git")
        .arg("--exec-path")
        .output()
        .unwrap();
    let daemon_path = std::path::PathBuf::from(String::from_utf8(exec_path.stdout).unwrap().trim())
        .join("git-daemon");
    let mut daemon = tokio::process::Command::new(daemon_path)
        .args([
            "--export-all",
            "--reuseaddr",
            "--listen=127.0.0.1",
            &format!("--port={port}"),
            &format!("--base-path={}", temp.path().display()),
        ])
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .is_ok()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let remote = "https://github.com/example/fixture.git";
    let clone = temp.path().join("repos").join(&hash(remote)[..20]);
    std::fs::create_dir_all(clone.parent().unwrap()).unwrap();
    cmd(
        temp.path(),
        &[
            "clone",
            "--no-checkout",
            repo.to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );
    cmd(&clone, &["remote", "set-url", "origin", remote]);
    cmd(
        &clone,
        &[
            "config",
            &format!("url.git://127.0.0.1:{port}/repo.insteadOf"),
            remote,
        ],
    );
    std::fs::write(repo.join("latest.rs"), "fn latest() {}\n").unwrap();
    save(&repo, "new remote commit", "2026-01-04T00:00:00Z");
    std::fs::write(clone.join("sentinel"), "keep me").unwrap();
    assert_eq!(
        git::repository_with_progress(remote, temp.path(), 32, &c, &progress)
            .await
            .unwrap(),
        clone
    );
    assert_eq!(
        git::commit(&clone, "HEAD", &c).await.unwrap().message,
        "new remote commit"
    );
    assert_eq!(
        std::fs::read_to_string(clone.join("sentinel")).unwrap(),
        "keep me"
    );
    assert!(!clone.join("latest.rs").exists());
    assert!(
        events
            .lock()
            .unwrap()
            .iter()
            .any(|s| s.contains("Reusing saved"))
    );
    assert!(
        events
            .lock()
            .unwrap()
            .iter()
            .any(|s| s.contains("Fetching latest"))
    );
    assert!(
        events
            .lock()
            .unwrap()
            .iter()
            .any(|s| s.contains("Repository updated"))
    );
    git::repository_with_progress(remote, temp.path(), 32, &c, &progress)
        .await
        .unwrap();
    assert!(
        events
            .lock()
            .unwrap()
            .iter()
            .any(|s| s.contains("up to date"))
    );
    // A specific commit outside a shallow clone is fetched without moving HEAD.
    let shallow_clone = temp.path().join("shallow");
    cmd(
        temp.path(),
        &[
            "clone",
            "--depth",
            "1",
            "--no-checkout",
            &format!("git://127.0.0.1:{port}/repo"),
            shallow_clone.to_str().unwrap(),
        ],
    );
    let initial_head = git::commit(&repo, "HEAD", &c).await.unwrap().sha;
    let root_sha = git::commit(&repo, "HEAD~2", &c).await.unwrap().sha;
    let mut selected = options(remote.into());
    selected.limit = None;
    selected.commit = Some(root_sha.clone());
    assert_eq!(
        git::history(&shallow_clone, &selected, &c).await.unwrap(),
        vec![root_sha.clone()]
    );
    let count = git::run(
        Some(&shallow_clone),
        &["rev-list", "--count", &root_sha],
        &c,
        1000,
    )
    .await
    .unwrap();
    assert_eq!(
        count.trim(),
        "2",
        "A targeted fetch must not download the entire ancestry"
    );
    // HEAD itself is still a boundary: restore its parent before diffing it.
    selected.commit = Some("HEAD".into());
    assert_eq!(
        git::history(&shallow_clone, &selected, &c).await.unwrap(),
        vec![initial_head.clone()]
    );
    let target = git::commit(&shallow_clone, "HEAD", &c).await.unwrap();
    assert!(!target.parents.is_empty());
    assert_eq!(target.sha, initial_head);
    let (_, warnings) = git::evidence(&shallow_clone, &target, &c).await.unwrap();
    assert!(warnings.is_empty());
    // Short SHAs and relative refs missing from a shallow clone resolve after deepening.
    let second = temp.path().join("short-sha");
    cmd(
        temp.path(),
        &[
            "clone",
            "--depth",
            "1",
            "--no-checkout",
            &format!("git://127.0.0.1:{port}/repo"),
            second.to_str().unwrap(),
        ],
    );
    selected.commit = Some(root_sha[..10].into());
    assert_eq!(
        git::history(&second, &selected, &c).await.unwrap(),
        vec![root_sha]
    );
    selected.commit = Some("HEAD~1".into());
    let merge = git::history(&second, &selected, &c).await.unwrap();
    assert_eq!(merge.len(), 1);
    assert!(git::commit(&second, &merge[0], &c).await.unwrap().merge);
    daemon.kill().await.unwrap();
    assert!(
        git::repository_with_progress(remote, temp.path(), 32, &c, &progress)
            .await
            .unwrap_err()
            .to_string()
            .contains("stale results")
    );
    let stop = CancellationToken::new();
    stop.cancel();
    assert!(
        git::repository_with_progress(
            "https://github.com/example/uncloned",
            temp.path(),
            32,
            &stop,
            &progress
        )
        .await
        .is_err()
    );
    assert!(
        events
            .lock()
            .unwrap()
            .iter()
            .any(|s| s.contains("Cloning repository"))
    );
}
