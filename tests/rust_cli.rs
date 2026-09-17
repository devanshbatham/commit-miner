use commit_miner::{git, miner, model::*, store};
use serde_json::{Value, json};
use std::{path::Path, process::Command};
use tokio_util::sync::CancellationToken;
fn git_cmd(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
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
fn cli(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_commit-miner"))
        .arg("--data-dir")
        .arg(root)
        .args(args)
        .env("TYPESAFE_API_KEY", "test-cache-only")
        .env("JEV_MODEL", "jev-latest")
        .output()
        .unwrap()
}
fn html_data(bytes: &[u8]) -> Value {
    let html = std::str::from_utf8(bytes).unwrap();
    assert!(html.starts_with("<!doctype html>"));
    serde_json::from_str(
        html.split("<script id=\"report-data\" type=\"application/json\">")
            .nth(1)
            .unwrap()
            .split("</script>")
            .next()
            .unwrap(),
    )
    .unwrap()
}
#[tokio::test]
async fn native_cli_local_repository_cache_stream_and_all_exports() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("local repository");
    std::fs::create_dir(&repo).unwrap();
    git_cmd(&repo, &["init", "-q"]);
    git_cmd(&repo, &["config", "user.name", "Tester"]);
    git_cmd(&repo, &["config", "user.email", "test@example.invalid"]);
    std::fs::write(repo.join("main.rs"), "fn main() { run(); }\n").unwrap();
    git_cmd(&repo, &["add", "."]);
    git_cmd(
        &repo,
        &["-c", "core.hooksPath=/dev/null", "commit", "-qm", "Initial"],
    );
    std::fs::write(repo.join("main.rs"), "fn main() { authorize(); run(); }\n").unwrap();
    git_cmd(&repo, &["add", "."]);
    git_cmd(
        &repo,
        &[
            "-c",
            "core.hooksPath=/dev/null",
            "commit",
            "-qm",
            "=fix </script><script>window.injected=1</script>",
        ],
    );
    let data = temp.path().join("data");
    let cache = data.join("commit-cache");
    store::private_dir(&cache).unwrap();
    let c = CancellationToken::new();
    let sha = git_cmd(&repo, &["rev-parse", "HEAD"]);
    let commit = git::commit(&repo, &sha, &c).await.unwrap();
    let (evidence, w) = git::evidence(&repo, &commit, &c).await.unwrap();
    assert!(w.is_empty());
    let body = miner::request(
        &commit,
        &evidence,
        evidence.len(),
        "complete_commit",
        "jev-latest",
    );
    let answers=body["questions"].as_object().unwrap().keys().map(|id|(id.clone(),json!({"type":"noul","noul":if id=="t0_security_fix"||id=="t0_cwe_862"{0.98}else{0.05}}))).collect::<serde_json::Map<_,_>>();
    let key = hash(
        serde_json::to_vec(&json!({"version":"rust-commit-evidence-v1","body":body})).unwrap(),
    );
    store::atomic_json(&cache.join(format!("{key}.json")),&json!({"saved":chrono::Utc::now().timestamp_millis(),"response":{"model":"fixture-jev","answers":answers}})).unwrap();
    // Dirty working tree must not change the historical evidence or be modified.
    std::fs::write(repo.join("main.rs"), "uncommitted content\n").unwrap();
    let report = temp.path().join("report.html");
    let out = cli(
        &data,
        &[
            "scan",
            repo.to_str().unwrap(),
            "-n",
            "1",
            "--plain",
            "--workers",
            "32",
            "-o",
            report.to_str().unwrap(),
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("Missing authorization"));
    assert!(String::from_utf8_lossy(&out.stderr).contains("est. $0.000000"));
    assert!(!String::from_utf8_lossy(&out.stdout).contains("test-cache-only"));
    let diagnostics = String::from_utf8_lossy(&out.stderr);
    assert!(diagnostics.contains("Workers capped at 8 (requested 32)"));
    for stage in [
        "8 workers",
        "Opening local repository",
        "Reading commit history",
        "Analyzing 1 commit",
        "Saving results",
        "Exporting HTML",
        "Report saved",
        "Scan saved",
    ] {
        assert!(
            diagnostics.contains(stage),
            "Missing {stage}: {diagnostics}"
        );
    }
    let footer = diagnostics.trim_end().lines().last().unwrap();
    assert!(footer.contains("100%") && footer.contains("elapsed 00:"));
    let saved: Value = html_data(&std::fs::read(report).unwrap());
    assert_eq!(saved["summary"]["progress"]["cached"], 1);
    assert_eq!(saved["summary"]["progress"]["calls"], 0);
    assert!(
        saved["results"][0]["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["evidence"]["lines"]
                .as_array()
                .unwrap()
                .iter()
                .any(|l| l["text"] == "fn main() { authorize(); run(); }"))
    );
    let streamed = String::from_utf8_lossy(&out.stdout);
    for header in ["COMMIT", "MESSAGE", "TYPE / CWE", "DATE"] {
        assert!(streamed.contains(header));
    }
    assert_eq!(
        std::fs::read_to_string(repo.join("main.rs")).unwrap(),
        "uncommitted content\n"
    );
    assert!(!data.join("repos").exists());
    let id = saved["summary"]["id"].as_str().unwrap();
    let list = cli(&data, &["list"]);
    assert!(String::from_utf8_lossy(&list.stdout).contains(id));
    let show = cli(&data, &["show", id, "--commit", &sha[..8]]);
    assert!(show.status.success());
    assert!(String::from_utf8_lossy(&show.stdout).contains(&sha[..8]));
    assert!(String::from_utf8_lossy(&show.stdout).contains("authorize();"));
    for format in ["html", "csv"] {
        let out = cli(&data, &["export", id, "-f", format]);
        assert!(out.status.success());
        let text = String::from_utf8(out.stdout).unwrap();
        if format == "html" {
            assert!(text.starts_with("<!doctype html>"));
            assert!(text.contains("\\u003c/script\\u003e"));
            assert!(text.contains("data:font/woff2;base64,"));
            assert!(!text.contains("<script>window.injected=1</script>"));
        } else if format == "csv" {
            assert!(text.contains("\"'=fix"));
            assert!(text.contains("CWE-862"));
        }
    }
    let machine = cli(
        &data,
        &[
            "scan",
            repo.to_str().unwrap(),
            "-n",
            "1",
            "--plain",
            "--format",
            "HTML",
        ],
    );
    assert!(machine.status.success());
    html_data(&machine.stdout);
    let filtered = cli(
        &data,
        &[
            "scan",
            repo.to_str().unwrap(),
            "-n",
            "1",
            "--only",
            "change_performance",
            "--format",
            "html",
            "--plain",
        ],
    );
    assert!(filtered.status.success());
    let report: Value = html_data(&filtered.stdout);
    assert_eq!(report["selection"]["matched"], 0);
    assert_eq!(report["summary"]["progress"]["classified"], 1);
    assert!(report["results"].as_array().unwrap().is_empty());
    let stored = cli(
        &data,
        &[
            "export",
            report["summary"]["id"].as_str().unwrap(),
            "-f",
            "html",
        ],
    );
    assert_eq!(
        html_data(&stored.stdout)["results"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    // Select the cached historical commit after HEAD has moved forward.
    git_cmd(&repo, &["tag", "release-fixture", &sha]);
    git_cmd(&repo, &["add", "."]);
    git_cmd(
        &repo,
        &[
            "-c",
            "core.hooksPath=/dev/null",
            "commit",
            "-qm",
            "Newer commit",
        ],
    );
    let head = git_cmd(&repo, &["rev-parse", "HEAD"]);
    for reference in [&sha[..10], &sha, "HEAD~1", "release-fixture"] {
        let out = cli(
            &data,
            &[
                "scan",
                repo.to_str().unwrap(),
                "--commit",
                reference,
                "--format",
                "html",
                "--plain",
            ],
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let selected: Value = html_data(&out.stdout);
        assert_eq!(selected["results"].as_array().unwrap().len(), 1);
        assert_eq!(selected["results"][0]["commit"]["sha"], sha);
        assert_eq!(selected["summary"]["options"]["commit"], reference);
        assert!(selected["summary"]["options"].get("limit").is_none());
        assert_eq!(selected["summary"]["progress"]["total"], 1);
        assert_eq!(selected["summary"]["progress"]["calls"], 0);
        assert_eq!(selected["summary"]["progress"]["cached"], 1);
        let id = selected["summary"]["id"].as_str().unwrap();
        let csv = cli(&data, &["export", id, "-f", "csv"]);
        let text = String::from_utf8(csv.stdout).unwrap();
        assert_eq!(
            text.lines().next().unwrap(),
            "\"Commit\",\"Message\",\"Type\",\"CWE\",\"Date\""
        );
        assert!(text.contains(&sha));
        assert!(text.contains("CWE-862"));
    }
    assert_eq!(git_cmd(&repo, &["rev-parse", "HEAD"]), head);
    for reference in ["missing-reference", "HEAD:main.rs", "HEAD..HEAD~1", ""] {
        let out = cli(
            &data,
            &[
                "scan",
                repo.to_str().unwrap(),
                "--commit",
                reference,
                "--plain",
            ],
        );
        assert!(!out.status.success());
        let diagnostics = String::from_utf8_lossy(&out.stderr);
        assert!(
            diagnostics.contains("exactly one commit") || diagnostics.contains("Pass one commit"),
            "{diagnostics}"
        );
    }
}
#[tokio::test]
async fn invalid_inputs_and_local_excluded_only_bare_repo() {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    for args in [
        vec!["scan", ".", "--workers", "0"],
        vec!["scan", ".", "--commit", "HEAD", "-n", "1"],
        vec!["scan", ".", "--commit", "HEAD", "--since", "2026-01-01"],
        vec!["scan", ".", "--commit", "HEAD", "--until", "2026-01-01"],
        vec!["scan", ".", "--commit", "HEAD", "--first-parent"],
        vec!["scan", ".", "--since", "2026-02-30"],
        vec![
            "scan",
            ".",
            "--since",
            "2026-02-02",
            "--until",
            "2026-01-01",
        ],
    ] {
        assert!(!cli(&data, &args).status.success());
    }
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git_cmd(&repo, &["init", "-q"]);
    git_cmd(&repo, &["config", "user.name", "Test"]);
    git_cmd(&repo, &["config", "user.email", "test@example.invalid"]);
    std::fs::write(repo.join("README.md"), "hello").unwrap();
    git_cmd(&repo, &["add", "."]);
    git_cmd(
        &repo,
        &["-c", "core.hooksPath=/dev/null", "commit", "-qm", "docs"],
    );
    let bare = temp.path().join("repo.git");
    git_cmd(
        temp.path(),
        &[
            "clone",
            "--bare",
            repo.to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
    );
    let cancel = CancellationToken::new();
    let sha = git_cmd(&bare, &["rev-parse", "HEAD"]);
    let mut commit = git::commit(&bare, &sha, &cancel).await.unwrap();
    commit.excluded_files = commit.files.len();
    commit.files.clear();
    let (evidence, _) = git::evidence(&bare, &commit, &cancel).await.unwrap();
    let body = miner::request(
        &commit,
        &evidence,
        evidence.len(),
        "metadata_review",
        "jev-latest",
    );
    let answers = body["questions"]
        .as_object()
        .unwrap()
        .keys()
        .map(|id| (id.clone(), json!({"type":"noul","noul":0.05})))
        .collect::<serde_json::Map<_, _>>();
    let cache = data.join("commit-cache");
    store::private_dir(&cache).unwrap();
    let key = hash(
        serde_json::to_vec(&json!({"version":"rust-commit-evidence-v1","body":body})).unwrap(),
    );
    store::atomic_json(&cache.join(format!("{key}.json")),&json!({"saved":chrono::Utc::now().timestamp_millis(),"response":{"model":"mock-jev","answers":answers}})).unwrap();
    let out = cli(
        &data,
        &[
            "scan",
            bare.to_str().unwrap(),
            "--format",
            "html",
            "--plain",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let scan: Value = html_data(&out.stdout);
    assert_eq!(scan["summary"]["progress"]["excludedCommits"], 0);
    assert_eq!(scan["summary"]["progress"]["metadataOnly"], 1);
    assert_eq!(scan["results"].as_array().unwrap().len(), 1);
    assert_eq!(scan["results"][0]["reviewCoverage"], "metadata");
    assert_eq!(scan["summary"]["progress"]["calls"], 0);
}
fn report_fixture(root: &Path) -> String {
    let html = include_str!("../examples/sample.html");
    let data = html
        .split("<script id=\"report-data\" type=\"application/json\">")
        .nth(1)
        .unwrap()
        .split("</script>")
        .next()
        .unwrap();
    let mut scan: Scan = serde_json::from_str(data).unwrap();
    scan.summary.status = "completed".into();
    let first = &mut scan.results[0];
    first.commit.message = "Security review with Unicode 界界 and a long descriptive title".into();
    first.probabilities.insert("cwe_862".into(), 0.96);
    let mut second = first.clone();
    second.commit.sha = "b".repeat(40);
    second.commit.message = "SECURITY CWE-862 words alone do not classify this commit".into();
    second.probabilities.clear();
    second
        .probabilities
        .insert("change_performance".into(), 0.91);
    second.probabilities.insert("bug_fix".into(), 0.05);
    second.probabilities.insert("security_fix".into(), 0.05);
    second.categories = vec![Label {
        id: "change_performance".into(),
        probability: 0.91,
    }];
    let mut third = second.clone();
    third.commit.sha = "c".repeat(40);
    third.commit.message = "No detected category".into();
    third.probabilities.clear();
    third.categories.clear();
    scan.results.push(second);
    scan.results.push(third);
    scan.summary.progress.classified = 3;
    let id = scan.summary.id.clone();
    store::Store::new(root.into()).unwrap().save(&scan).unwrap();
    id
}
#[test]
fn filters_use_jev_probabilities_and_keep_saved_scans_intact() {
    let temp = tempfile::tempdir().unwrap();
    let id = report_fixture(temp.path());
    let cases = [
        (vec!["--only", "security"], 1),
        (vec!["--only", "security,change_performance"], 2),
        (vec!["--cwe", "CWE-862"], 1),
        (vec!["--only", "change", "--cwe", "862"], 0),
        (vec!["--only", "unclassified"], 1),
        (vec!["--only", "security", "--min-probability", "0.999"], 0),
    ];
    for (flags, count) in cases {
        let mut args = vec!["export", &id, "-f", "html"];
        args.extend(flags);
        let out = cli(temp.path(), &args);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let value: Value = html_data(&out.stdout);
        assert_eq!(value["results"].as_array().unwrap().len(), count);
        assert_eq!(value["selection"]["classified"], 3);
        assert_eq!(value["summary"]["progress"]["classified"], 3);
    }
    let csv = cli(temp.path(), &["export", &id, "--cwe", "862", "-f", "csv"]);
    let text = String::from_utf8(csv.stdout).unwrap();
    assert!(text.contains("CWE-862"));
    assert!(!text.contains("words alone"));
    let html = cli(
        temp.path(),
        &["export", &id, "--only", "change_performance", "-f", "html"],
    );
    let text = String::from_utf8(html.stdout).unwrap();
    let value: Value = serde_json::from_str(
        text.split("<script id=\"report-data\" type=\"application/json\">")
            .nth(1)
            .unwrap()
            .split("</script>")
            .next()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(value["results"].as_array().unwrap().len(), 1);
    assert_eq!(value["selection"]["only"][0], "change_performance");
    let all = cli(temp.path(), &["export", &id, "-f", "html"]);
    assert_eq!(
        html_data(&all.stdout)["results"].as_array().unwrap().len(),
        3
    );
    for bad in [
        vec!["--only", "imaginary"],
        vec!["--cwe", "999999"],
        vec!["--min-probability", "NaN"],
    ] {
        let mut args = vec!["show", &id];
        args.extend(bad);
        assert!(!cli(temp.path(), &args).status.success());
    }
}
#[test]
fn terminal_color_controls_cwe_display_and_machine_output() {
    let temp = tempfile::tempdir().unwrap();
    let id = report_fixture(temp.path());
    let colored = cli(
        temp.path(),
        &["show", &id, "--only", "security", "--color", "always"],
    );
    let text = String::from_utf8(colored.stdout).unwrap();
    assert!(text.contains("\u{1b}["));
    assert!(text.contains("CWE-862"));
    assert!(text.contains("Missing authorization"));
    assert!(!text.contains("words alone"));
    for flags in [
        vec!["--color", "never"],
        vec!["--plain", "--color", "always"],
        vec![],
    ] {
        let mut args = vec!["show", &id];
        args.extend(flags);
        assert!(!cli(temp.path(), &args).stdout.contains(&27));
    }
    for format in ["html", "csv"] {
        let out = cli(
            temp.path(),
            &["export", &id, "-f", format, "--color", "always"],
        );
        assert!(out.status.success());
        assert!(!out.stdout.contains(&27));
        if format == "html" {
            html_data(&out.stdout);
        } else {
            assert!(
                out.stdout
                    .starts_with(b"\"Commit\",\"Message\",\"Type\",\"CWE\",\"Date\"")
            );
        }
    }
    assert!(
        !cli(temp.path(), &["export", &id, "-f", "json"])
            .status
            .success()
    );
    assert!(!cli(temp.path(), &["show", &id, "--json"]).status.success());
    let cats = cli(temp.path(), &["categories", "--plain"]);
    let text = String::from_utf8(cats.stdout).unwrap();
    assert!(text.contains("cwe_79"));
    assert!(text.contains("CWE-79"));
    assert!(text.contains("change_performance"));
    let record = store::Store::new(temp.path().into())
        .unwrap()
        .get(&id)
        .unwrap()
        .results
        .remove(0);
    let t = commit_miner::terminal::Terminal {
        color: true,
        width: 40,
    };
    for row in t.record(&record, 0.65).lines() {
        assert!(console::measure_text_width(row) <= 40, "{row}");
    }
}

#[test]
fn security_fix_labels_require_a_supported_cwe() {
    let temp = tempfile::tempdir().unwrap();
    let id = report_fixture(temp.path());
    let mut scan = store::Store::new(temp.path().into())
        .unwrap()
        .get(&id)
        .unwrap();
    let record = &mut scan.results[0];
    record.probabilities.clear();
    record.categories.clear();
    record.probabilities.insert("security_fix".into(), 0.99);
    let t = commit_miner::terminal::Terminal {
        color: false,
        width: 100,
    };
    let card = t.record(record, 0.65);
    assert!(card.contains("SECURITY REVIEW"));
    assert!(card.contains("CWE unresolved"));
    assert!(!card.contains("SECURITY FIX"));
    let table = t.table(&[record], 0.65);
    assert!(table.contains("Security review"));
    assert!(table.contains("CWE"));
    assert!(table.contains("unresolved"));
    record.probabilities.insert("cwe_862".into(), 0.95);
    assert!(t.record(record, 0.65).contains("SECURITY FIX · CWE-862"));
}

#[test]
fn tables_show_one_primary_type_and_minimal_csv() {
    let temp = tempfile::tempdir().unwrap();
    let id = report_fixture(temp.path());
    let mut scan = store::Store::new(temp.path().into())
        .unwrap()
        .get(&id)
        .unwrap();
    scan.results.truncate(1);
    let r = &mut scan.results[0];
    r.categories.clear();
    r.probabilities.clear();
    r.probabilities.insert("security_fix".into(), 0.8);
    r.probabilities.insert("cwe_862".into(), 0.8);
    r.probabilities.insert("bug_fix".into(), 0.99);
    r.probabilities.insert("change_performance".into(), 0.99);
    assert_eq!(r.primary_classification(0.65).0, "Security fix");
    let t = commit_miner::terminal::Terminal {
        color: false,
        width: 100,
    };
    let table = t.table(&[r], 0.65);
    assert!(table.contains("Security fix"));
    assert!(table.contains("CWE-862"));
    assert!(!table.contains("Performance"));
    assert!(!table.contains("Bug fix"));
    let csv = commit_miner::export::render(&scan, "csv").unwrap();
    assert_eq!(
        csv.lines().next().unwrap(),
        "\"Commit\",\"Message\",\"Type\",\"CWE\",\"Date\""
    );
    assert_eq!(csv.lines().count(), 2);
    assert!(csv.contains("\"Security fix\",\"CWE-862\""));
    assert!(!csv.contains("Performance"));
    let r = &mut scan.results[0];
    r.probabilities.remove("cwe_862");
    r.probabilities.remove("security_fix");
    assert_eq!(r.primary_classification(0.65).0, "Bug fix");
    r.probabilities.remove("bug_fix");
    r.probabilities.insert("change_feature".into(), 0.7);
    assert_eq!(r.primary_classification(0.65).0, "Performance");
}

#[test]
fn elapsed_time_stays_on_the_final_progress_line() {
    let t = commit_miner::terminal::Terminal {
        color: false,
        width: 80,
    };
    let progress = Progress {
        percent: 100.,
        elapsed_seconds: 3661.8,
        ..Progress::default()
    };
    let footer = t.progress(&progress);
    assert!(footer.trim_end().ends_with("elapsed 01:01:01"));
    assert!(console::measure_text_width(footer.trim_end()) <= 80);
}
