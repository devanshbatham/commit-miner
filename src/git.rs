use crate::model::*;
use anyhow::{Context, Result, ensure};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
    process::Command,
};
use tokio_util::sync::CancellationToken;
#[derive(Clone, Copy)]
pub enum GitProgress<'a> {
    Stage(&'a str),
    Transfer(&'a str),
}
impl GitProgress<'_> {
    pub fn text(&self) -> &str {
        match self {
            Self::Stage(s) | Self::Transfer(s) => s,
        }
    }
}
type ProgressSink<'a> = &'a (dyn Fn(GitProgress<'_>) + Sync);
#[derive(Clone, Copy)]
enum Access {
    Local,
    Network,
}

// Reading objects must never contact a repository-controlled remote, even on
// Git versions predating GIT_NO_LAZY_FETCH. Both Git execution paths use this
// builder so streamed diffs have the same protections as metadata reads.
fn command(dir: Option<&Path>, access: Access) -> Command {
    let mut cmd = Command::new("git");
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            cmd.env_remove(key);
        }
    }
    cmd.args(["--no-pager", "--literal-pathspecs"]);
    for setting in [
        "core.hooksPath=/dev/null",
        "core.fsmonitor=false",
        "core.quotePath=true",
        "color.ui=false",
        "core.alternateRefsCommand=:",
        "log.showSignature=false",
        "gc.auto=0",
        "maintenance.auto=false",
        "fetch.recurseSubmodules=false",
        "submodule.recurse=false",
        "protocol.allow=never",
    ] {
        cmd.args(["-c", setting]);
    }
    if let Some(dir) = dir {
        cmd.arg("-C").arg(dir);
    }
    // Unit fixtures use a loopback git-daemon. Shipped binaries only allow HTTPS.
    let protocols = if matches!(access, Access::Network) {
        if cfg!(test) { "https:git" } else { "https" }
    } else {
        ""
    };
    cmd.env("GIT_ALLOW_PROTOCOL", protocols)
        .env_remove("TYPESAFE_API_KEY")
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_LFS_SKIP_SMUDGE", "1")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .kill_on_drop(true);
    cmd
}
pub async fn run(
    dir: Option<&Path>,
    args: &[&str],
    cancel: &CancellationToken,
    max: usize,
) -> Result<String> {
    run_observed(dir, args, cancel, max, None, Access::Local).await
}
async fn run_observed(
    dir: Option<&Path>,
    args: &[&str],
    cancel: &CancellationToken,
    max: usize,
    progress: Option<ProgressSink<'_>>,
    access: Access,
) -> Result<String> {
    ensure!(!cancel.is_cancelled(), "Cancelled");
    let mut cmd = command(dir, access);
    cmd.args(args)
        .stderr(if progress.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .kill_on_drop(true);
    let mut process = crate::process::Process::spawn(&mut cmd)
        .context("Could not start Git; install git and check PATH")?;
    let child = &mut process.child;
    let mut out = child
        .stdout
        .take()
        .unwrap()
        .take((max as u64).saturating_add(1));
    let mut stderr = child.stderr.take();
    let work = async {
        let read_output = async {
            let mut bytes = vec![];
            out.read_to_end(&mut bytes).await?;
            Ok::<_, anyhow::Error>(bytes)
        };
        let read_progress = async {
            let mut last = String::new();
            if let Some(reader) = stderr.as_mut() {
                let mut pending = vec![];
                let mut chunk = [0; 4096];
                loop {
                    let n = reader.read(&mut chunk).await?;
                    for byte in &chunk[..n] {
                        if *byte == b'\r' || *byte == b'\n' {
                            if !pending.is_empty() {
                                last = String::from_utf8_lossy(&pending).trim().to_string();
                                if let Some(sink) = progress {
                                    sink(GitProgress::Transfer(&last));
                                }
                                pending.clear();
                            }
                        } else if pending.len() < 8192 {
                            pending.push(*byte);
                        }
                    }
                    if n == 0 {
                        break;
                    }
                }
                if !pending.is_empty() {
                    last = String::from_utf8_lossy(&pending).trim().to_string();
                    if let Some(sink) = progress {
                        sink(GitProgress::Transfer(&last));
                    }
                }
            }
            Ok::<_, anyhow::Error>(last)
        };
        let (bytes, detail) = tokio::try_join!(read_output, read_progress)?;
        ensure!(
            bytes.len() <= max,
            "Git output exceeds the per-operation size limit"
        );
        let status = child.wait().await?;
        ensure!(
            status.success(),
            "Git operation failed. {}",
            if detail.is_empty() {
                "Check repository access, network, and available history."
            } else {
                &detail
            }
        );
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    };
    let result = tokio::select! {biased; _=cancel.cancelled()=>Err(anyhow::anyhow!("Cancelled")),r=tokio::time::timeout(Duration::from_secs(180),work)=>r.context("Git operation timed out").and_then(|r| r)};
    if result.is_err() {
        process.stop().await;
    }
    result
}
async fn git(dir: &Path, args: &[&str], cancel: &CancellationToken) -> Result<String> {
    run(Some(dir), args, cancel, usize::MAX).await
}
async fn network(
    dir: &Path,
    args: &[&str],
    c: &CancellationToken,
    progress: ProgressSink<'_>,
) -> Result<String> {
    validate_network_repository(dir, c).await?;
    let mut args = args.to_vec();
    args.insert(1, "--progress");
    args.insert(2, "--no-recurse-submodules");
    args.insert(3, "--no-auto-maintenance");
    run_observed(
        Some(dir),
        &args,
        c,
        usize::MAX,
        Some(progress),
        Access::Network,
    )
    .await
}

// Managed clones only need the configuration Git itself writes when cloning.
// Do not execute credentials, transport helpers, includes, or other extensions
// added to their local config. User/system credentials remain available.
async fn validate_network_repository(dir: &Path, c: &CancellationToken) -> Result<()> {
    let config = run(
        Some(dir),
        &["config", "--local", "--null", "--list", "--includes"],
        c,
        1024 * 1024,
    )
    .await?;
    for entry in config.split('\0').filter(|s| !s.is_empty()) {
        let key = entry.split_once('\n').map_or(entry, |(key, _)| key);
        let safe = matches!(
            key,
            "core.repositoryformatversion"
                | "core.filemode"
                | "core.bare"
                | "core.logallrefupdates"
                | "core.ignorecase"
                | "core.precomposeunicode"
                | "extensions.objectformat"
                | "extensions.refstorage"
                | "remote.origin.url"
                | "remote.origin.fetch"
        ) || key.starts_with("branch.")
            && (key.ends_with(".remote") || key.ends_with(".merge"));
        // The existing network tests serve their GitHub fixture over loopback.
        let fixture =
            cfg!(test) && key.starts_with("url.git://127.0.0.1:") && key.ends_with(".insteadof");
        ensure!(
            safe || fixture,
            "Saved repository contains unsupported Git configuration ({key}); use a fresh data directory"
        );
    }
    let original = git(dir, &["config", "--get", "remote.origin.url"], c).await?;
    let effective = git(dir, &["remote", "get-url", "origin"], c).await?;
    if cfg!(test) && effective.trim().starts_with("git://127.0.0.1:") {
        return Ok(());
    }
    let expected = github_url(original.trim())?;
    ensure!(
        github_url(effective.trim())?.eq_ignore_ascii_case(&expected),
        "Git URL rewriting changed the saved repository's origin"
    );
    Ok(())
}
pub fn github_url(input: &str) -> Result<String> {
    let url = url::Url::parse(input).context("Use https://github.com/owner/repository")?;
    let p = url.path().trim_matches('/').split('/').collect::<Vec<_>>();
    ensure!(
        url.scheme() == "https"
            && url.host_str() == Some("github.com")
            && url.username().is_empty()
            && url.password().is_none()
            && url.port().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && p.len() == 2,
        "Use https://github.com/owner/repository"
    );
    let name = p[1].strip_suffix(".git").unwrap_or(p[1]);
    for part in [p[0], name] {
        ensure!(
            !part.is_empty()
                && part != "."
                && part != ".."
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "_- .".contains(c) && c != ' '),
            "Invalid GitHub repository name"
        );
    }
    Ok(format!("https://github.com/{}/{name}.git", p[0]))
}
pub async fn repository(
    source: &str,
    root: &Path,
    depth: usize,
    cancel: &CancellationToken,
) -> Result<PathBuf> {
    repository_with_progress(source, root, depth, cancel, &|_| {}).await
}
pub async fn repository_with_progress(
    source: &str,
    root: &Path,
    depth: usize,
    cancel: &CancellationToken,
    progress: ProgressSink<'_>,
) -> Result<PathBuf> {
    // Local history is read offline, without checkout, hooks or signature helpers.
    let local = Path::new(source);
    if local.is_dir() {
        progress(GitProgress::Stage(&format!(
            "Opening local repository · {}",
            local.display()
        )));
        let bare = git(local, &["rev-parse", "--is-bare-repository"], cancel).await?;
        let flag = if bare.trim() == "true" {
            "--absolute-git-dir"
        } else {
            "--show-toplevel"
        };
        let root = git(local, &["rev-parse", flag], cancel).await?;
        return Ok(PathBuf::from(root.trim()));
    }
    let remote = github_url(source)?;
    let dir = root.join("repos").join(&hash(remote.to_lowercase())[..20]);
    crate::store::private_dir(dir.parent().unwrap())?;
    if dir.exists() {
        progress(GitProgress::Stage(&format!(
            "Reusing saved repository · {}",
            dir.display()
        )));
        ensure!(
            git(&dir, &["config", "--get", "remote.origin.url"], cancel)
                .await?
                .trim()
                .eq_ignore_ascii_case(&remote),
            "Existing repository has an unexpected origin"
        );
        progress(GitProgress::Stage("Fetching latest commits from origin"));
        let before = git(&dir, &["rev-parse", "--verify", "HEAD"], cancel).await?;
        network(&dir, &["fetch", "--no-tags", "origin"], cancel, progress)
            .await
            .context(
                "Could not refresh the saved repository; scan stopped to avoid stale results",
            )?;
        let latest = git(
            &dir,
            &["rev-parse", "--verify", "@{upstream}^{commit}"],
            cancel,
        )
        .await
        .context("Saved repository has no available upstream branch")?;
        if before.trim() == latest.trim() {
            progress(GitProgress::Stage("Repository is up to date"));
        } else {
            // Managed clones use --no-checkout. Advance the tracked history
            // directly: pull/merge would populate files and run merge machinery.
            git(
                &dir,
                &["update-ref", "HEAD", latest.trim(), before.trim()],
                cancel,
            )
            .await
            .context("Could not advance the saved repository to its fetched upstream")?;
            progress(GitProgress::Stage(&format!(
                "Repository updated · {} → {}",
                &before.trim()[..7],
                &latest.trim()[..7]
            )));
        }
    } else {
        progress(GitProgress::Stage(&format!(
            "Cloning repository · {remote}"
        )));
        progress(GitProgress::Stage(&format!(
            "Clone destination · {}",
            dir.display()
        )));
        let staging = dir.with_extension(format!("{}.partial", uuid::Uuid::new_v4()));
        let result = run_observed(
            None,
            &[
                "clone",
                "--progress",
                "--no-checkout",
                "--no-recurse-submodules",
                "--template=",
                "--depth",
                &depth.to_string(),
                "--",
                &remote,
                staging.to_str().context("Non-UTF8 data directory")?,
            ],
            cancel,
            8 * 1024 * 1024,
            Some(progress),
            Access::Network,
        )
        .await;
        if let Err(e) = result {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(e);
        }
        if let Err(e) = std::fs::rename(&staging, &dir) {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(e.into());
        }
    }
    Ok(dir)
}
pub fn date(value: &str) -> Result<chrono::NaiveDate> {
    ensure!(
        value.len() == 10
            && value.bytes().enumerate().all(|(i, b)| if i == 4 || i == 7 {
                b == b'-'
            } else {
                b.is_ascii_digit()
            }),
        "Dates must be YYYY-MM-DD"
    );
    chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").context("Invalid calendar date")
}
async fn shallow(dir: &Path, c: &CancellationToken) -> Result<bool> {
    Ok(git(dir, &["rev-parse", "--is-shallow-repository"], c)
        .await?
        .trim()
        == "true")
}
async fn resolve_commit(dir: &Path, reference: &str, c: &CancellationToken) -> Result<String> {
    let peeled = format!("{reference}^{{commit}}");
    Ok(git(
        dir,
        &["rev-parse", "--verify", "--end-of-options", &peeled],
        c,
    )
    .await?
    .trim()
    .to_string())
}
async fn single_commit(
    dir: &Path,
    reference: &str,
    remote_source: bool,
    c: &CancellationToken,
    progress: ProgressSink<'_>,
) -> Result<String> {
    ensure!(
        !reference.is_empty()
            && !reference.starts_with('-')
            && !reference.chars().any(char::is_control),
        "Pass one commit SHA or Git reference to --commit"
    );
    progress(GitProgress::Stage(&format!(
        "Resolving commit · {reference}"
    )));
    let full_sha =
        matches!(reference.len(), 40 | 64) && reference.bytes().all(|b| b.is_ascii_hexdigit());
    let mut resolved = resolve_commit(dir, reference, c).await;
    // A SHA outside the managed clone's branch may still be available remotely.
    // Only an object ID is passed as a fetch refspec, never arbitrary user syntax.
    if resolved.is_err() && remote_source && full_sha && !c.is_cancelled() {
        progress(GitProgress::Stage("Fetching requested commit from origin"));
        network(
            dir,
            &["fetch", "--depth=2", "--no-tags", "origin", reference],
            c,
            progress,
        )
        .await
        .context("Could not fetch the requested commit from origin")?;
        resolved = resolve_commit(dir, reference, c).await;
    }
    if resolved.is_err() && remote_source && !c.is_cancelled() && shallow(dir, c).await? {
        progress(GitProgress::Stage(
            "Fetching full history to resolve the requested commit",
        ));
        network(
            dir,
            &["fetch", "--unshallow", "--tags", "origin"],
            c,
            progress,
        )
        .await
        .context("Could not fetch history for the requested commit")?;
        resolved = resolve_commit(dir, reference, c).await;
    }
    let sha = resolved.with_context(|| format!("Could not resolve '{reference}' to exactly one commit; use a full SHA or an available Git reference"))?;
    // Git hides parents of a shallow boundary commit even when they exist in
    // its raw object. Restore ancestry before constructing its before/after diff.
    if shallow(dir, c).await? {
        let boundary = git(dir, &["rev-list", "--parents", "-n", "1", &sha, "--"], c).await?;
        if boundary.split_whitespace().count() == 1 {
            let raw = git(dir, &["cat-file", "-p", &sha], c).await?;
            if raw
                .split("\n\n")
                .next()
                .unwrap_or("")
                .lines()
                .any(|line| line.starts_with("parent "))
            {
                ensure!(
                    remote_source,
                    "Selected commit's parent is outside the local shallow history; fetch the required history yourself before scanning"
                );
                progress(GitProgress::Stage(
                    "Fetching parent history for the selected diff",
                ));
                network(
                    dir,
                    &["fetch", "--depth=2", "--no-tags", "origin", &sha],
                    c,
                    progress,
                )
                .await
                .context("Could not fetch the selected commit's parent history")?;
                let restored =
                    git(dir, &["rev-list", "--parents", "-n", "1", &sha, "--"], c).await?;
                ensure!(
                    restored.split_whitespace().count() > 1,
                    "The selected commit's parent history is unavailable"
                );
            }
        }
    }
    progress(GitProgress::Stage(&format!("Selected commit · {sha}")));
    Ok(sha)
}
pub async fn history(dir: &Path, o: &Options, c: &CancellationToken) -> Result<Vec<String>> {
    history_with_progress(dir, o, c, &|_| {}).await
}
pub async fn history_with_progress(
    dir: &Path,
    o: &Options,
    c: &CancellationToken,
    progress: ProgressSink<'_>,
) -> Result<Vec<String>> {
    let remote_source = !Path::new(&o.source).is_dir();
    if let Some(reference) = &o.commit {
        ensure!(
            o.limit.is_none() && o.since.is_none() && o.until.is_none() && !o.first_parent,
            "--commit cannot be combined with history selection options"
        );
        return Ok(vec![
            single_commit(dir, reference, remote_source, c, progress).await?,
        ]);
    }
    progress(GitProgress::Stage("Reading commit history"));
    let traversal = if o.first_parent {
        "--first-parent"
    } else {
        "--topo-order"
    };
    if o.since.is_some() || o.until.is_some() {
        if shallow(dir, c).await? {
            ensure!(
                remote_source,
                "A complete date range requires history outside the local shallow repository; fetch the required history yourself before scanning"
            );
            progress(GitProgress::Stage(
                "Fetching full history for the date range",
            ));
            network(
                dir,
                &["fetch", "--unshallow", "--no-tags", "origin"],
                c,
                progress,
            )
            .await?;
            ensure!(
                !shallow(dir, c).await?,
                "The remote has incomplete history; the full date range cannot be determined"
            );
        }
        let start = o
            .since
            .as_ref()
            .map(|s| date(s).map(|d| d.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp()))
            .transpose()?
            .unwrap_or(i64::MIN);
        let end = o
            .until
            .as_ref()
            .map(|s| date(s).map(|d| d.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp() + 86400))
            .transpose()?
            .unwrap_or(i64::MAX);
        let output = run(
            Some(dir),
            &["rev-list", "--timestamp", traversal, "HEAD", "--"],
            c,
            usize::MAX,
        )
        .await?;
        let mut records = vec![];
        for l in output.lines() {
            if let Some((t, sha)) = l.split_once(' ') {
                let t = t.parse::<i64>()?;
                if t >= start && t < end {
                    records.push((t, sha.to_string()));
                }
            }
        }
        records.sort_by_key(|a| std::cmp::Reverse(a.0));
        if let Some(n) = o.limit {
            records.truncate(n);
        }
        return Ok(records.into_iter().map(|(_, s)| s).collect());
    }
    let n = o.limit.unwrap_or(250);
    let max = format!("--max-count={}", n + 1);
    let mut ids = git(dir, &["rev-list", traversal, &max, "HEAD", "--"], c)
        .await?
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    while ids.len() <= n && shallow(dir, c).await? {
        ensure!(
            remote_source,
            "Requested history reaches the local shallow boundary; fetch the required history yourself before scanning"
        );
        progress(GitProgress::Stage(&format!(
            "Fetching more history · {} commits available",
            ids.len()
        )));
        network(
            dir,
            &[
                "fetch",
                "--deepen",
                &(n + 32).to_string(),
                "--no-tags",
                "origin",
            ],
            c,
            progress,
        )
        .await?;
        let more = git(dir, &["rev-list", traversal, &max, "HEAD", "--"], c)
            .await?
            .lines()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let same = more.len() == ids.len();
        ids = more;
        if same {
            break;
        }
    }
    ids.truncate(n);
    Ok(ids)
}
/// Scheduling only: use Git's numeric diff statistics, never source-text heuristics.
pub async fn prioritize(dir: &Path, ids: &[String], c: &CancellationToken) -> Result<Vec<String>> {
    let mut sizes = std::collections::HashMap::<String, usize>::new();
    for batch in ids.chunks(128) {
        let mut args = vec![
            "show",
            "--format=%H",
            "--numstat",
            "-z",
            "--no-renames",
            "--no-ext-diff",
            "--no-textconv",
            "--diff-merges=first-parent",
        ];
        args.extend(batch.iter().map(String::as_str));
        args.push("--");
        let stats = run(Some(dir), &args, c, usize::MAX).await?;
        let mut current = None;
        for field in stats.split('\0') {
            if batch.iter().any(|sha| sha == field) {
                sizes.insert(field.to_string(), 0);
                current = Some(field);
                continue;
            }
            if let Some(sha) = current {
                let mut columns = field.strip_prefix('\n').unwrap_or(field).splitn(3, '\t');
                if let (Some(added), Some(removed), Some(path)) =
                    (columns.next(), columns.next(), columns.next())
                    && crate::policy::eligible(path)
                {
                    // Include per-file context/metadata cost; unknown binary sizes go last.
                    let cost = added
                        .parse::<usize>()
                        .ok()
                        .zip(removed.parse::<usize>().ok())
                        .map(|(a, b)| a.saturating_add(b).saturating_add(40))
                        .unwrap_or(usize::MAX / 2);
                    let size = sizes.get_mut(sha).expect("known Git commit");
                    *size = size.saturating_add(cost);
                }
            }
        }
    }
    let mut ordered = ids.to_vec();
    // Stable sorting preserves original history order for equal-sized changes.
    ordered.sort_by_key(|sha| sizes.get(sha).copied().unwrap_or(usize::MAX));
    Ok(ordered)
}
pub async fn commit(dir: &Path, sha: &str, c: &CancellationToken) -> Result<Commit> {
    let output = git(
        dir,
        &[
            "show",
            "-s",
            "--format=%H%x00%P%x00%aI%x00%an%x00%cI%x00%B",
            sha,
            "--",
        ],
        c,
    )
    .await?;
    let f = output.split('\0').collect::<Vec<_>>();
    ensure!(f.len() == 6, "Unexpected Git commit metadata");
    let parents = f[1]
        .split_whitespace()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if parents.is_empty() {
        let raw = git(dir, &["cat-file", "-p", sha], c).await?;
        ensure!(
            !raw.split("\n\n")
                .next()
                .unwrap_or("")
                .lines()
                .any(|l| l.starts_with("parent ")),
            "Commit parent is outside available history"
        );
    }
    let mut args = vec![
        "diff-tree",
        "--root",
        "--no-commit-id",
        "--name-only",
        "-r",
        "-z",
        "--no-renames",
    ];
    if let Some(p) = parents.first() {
        args.push(p);
    }
    args.extend([sha, "--"]);
    let files = git(dir, &args, c)
        .await?
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    Ok(Commit {
        sha: f[0].into(),
        merge: parents.len() > 1,
        parents,
        date: f[2].into(),
        author: f[3].into(),
        committed_at: f[4].into(),
        message: f[5].trim_end().into(),
        files,
        excluded_files: 0,
    })
}
// Parses Git framing/coordinates only; does not classify source text.
pub fn parse_diff(patch: &str) -> Vec<(String, Vec<DiffLine>)> {
    let mut hunks: Vec<(String, Vec<DiffLine>)> = vec![];
    let (mut old, mut new) = (0, 0);
    for line in patch.lines() {
        if line.starts_with("@@ ") {
            let mut fields = line.split_whitespace();
            fields.next();
            let a = fields
                .next()
                .unwrap_or("")
                .trim_start_matches('-')
                .split(',')
                .next()
                .unwrap_or("")
                .parse::<usize>();
            let b = fields
                .next()
                .unwrap_or("")
                .trim_start_matches('+')
                .split(',')
                .next()
                .unwrap_or("")
                .parse::<usize>();
            if let (Ok(a), Ok(b)) = (a, b) {
                old = a;
                new = b;
                hunks.push((line.into(), vec![]));
            }
            continue;
        }
        let Some((_, lines)) = hunks.last_mut() else {
            continue;
        };
        let (kind, text, o, n) = match line.as_bytes().first() {
            Some(b'+') => {
                let n = new;
                new += 1;
                ("added", &line[1..], None, Some(n))
            }
            Some(b'-') => {
                let o = old;
                old += 1;
                ("removed", &line[1..], Some(o), None)
            }
            Some(b' ') => {
                let (o, n) = (old, new);
                old += 1;
                new += 1;
                ("context", &line[1..], Some(o), Some(n))
            }
            _ => ("meta", line, None, None),
        };
        lines.push(DiffLine {
            kind: kind.into(),
            text: text.into(),
            old_line: o,
            new_line: n,
        });
    }
    hunks
}
// Read Git output incrementally into request-sized source sections. There is no
// per-file or cumulative diff byte cap. The request budget controls chunking only.
struct Sections {
    sha: String,
    path: String,
    header: String,
    old: usize,
    new: usize,
    in_hunk: bool,
    lines: Vec<DiffLine>,
    bytes: usize,
    overlap: usize,
    items: Vec<Evidence>,
}
impl Sections {
    fn new(sha: &str, path: &str) -> Self {
        Self {
            sha: sha.into(),
            path: path.into(),
            header: "File metadata or binary change".into(),
            old: 0,
            new: 0,
            in_hunk: false,
            lines: vec![],
            bytes: 0,
            overlap: 0,
            items: vec![],
        }
    }
    fn flush(&mut self, overlap: bool) -> Result<()> {
        if self.lines.len() <= self.overlap {
            return Ok(());
        }
        let id = hash(serde_json::to_vec(&(
            &self.sha,
            &self.path,
            self.items.len(),
            &self.lines,
        ))?);
        self.items.push(Evidence {
            id: id[..24].into(),
            sha: self.sha.clone(),
            path: self.path.clone(),
            header: self.header.clone(),
            lines: self.lines.clone(),
            part: 0,
            parts: 0,
        });
        let keep = if overlap { self.lines.len().min(3) } else { 0 };
        self.lines = self.lines[self.lines.len() - keep..].to_vec();
        self.overlap = keep;
        self.bytes = serde_json::to_vec(&self.lines)?.len();
        Ok(())
    }
    fn push(&mut self, line: DiffLine) -> Result<()> {
        if serde_json::to_vec(&line)?.len() > 1800 {
            let mut offset = 0;
            while offset < line.text.len() {
                let rest = &line.text[offset..];
                let end = rest
                    .char_indices()
                    .nth(300)
                    .map(|(i, _)| i)
                    .unwrap_or(rest.len());
                let next = rest
                    .char_indices()
                    .nth(250)
                    .map(|(i, _)| i)
                    .unwrap_or(rest.len());
                let mut part = line.clone();
                part.text = rest[..end].into();
                self.add(part)?;
                if end == rest.len() {
                    break;
                }
                offset += next;
            }
        } else {
            self.add(line)?;
        }
        Ok(())
    }
    fn add(&mut self, line: DiffLine) -> Result<()> {
        let bytes = serde_json::to_vec(&line)?.len();
        if self.bytes + bytes > 5000 {
            self.flush(true)?;
        }
        self.bytes += bytes;
        self.lines.push(line);
        Ok(())
    }
    fn line(&mut self, line: &str) -> Result<()> {
        if line.starts_with("@@ ") {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            let coordinate = |i: usize| {
                fields
                    .get(i)
                    .and_then(|s| s.get(1..))
                    .and_then(|s| s.split(',').next())
                    .and_then(|s| s.parse::<usize>().ok())
            };
            if let (Some(old), Some(new)) = (coordinate(1), coordinate(2)) {
                if self.in_hunk {
                    self.flush(false)?;
                    self.lines.clear();
                    self.bytes = 0;
                    self.overlap = 0;
                }
                self.header = line.into();
                self.old = old;
                self.new = new;
                self.in_hunk = true;
            }
            return Ok(());
        }
        let (kind, text, old, new) = if self.in_hunk {
            match line.as_bytes().first() {
                Some(b'+') => {
                    let n = self.new;
                    self.new += 1;
                    ("added", &line[1..], None, Some(n))
                }
                Some(b'-') => {
                    let o = self.old;
                    self.old += 1;
                    ("removed", &line[1..], Some(o), None)
                }
                Some(b' ') => {
                    let (o, n) = (self.old, self.new);
                    self.old += 1;
                    self.new += 1;
                    ("context", &line[1..], Some(o), Some(n))
                }
                _ => ("meta", line, None, None),
            }
        } else {
            ("meta", line, None, None)
        };
        self.push(DiffLine {
            kind: kind.into(),
            text: text.into(),
            old_line: old,
            new_line: new,
        })
    }
    fn finish(mut self) -> Result<Vec<Evidence>> {
        self.flush(false)?;
        let count = self.items.len();
        for (i, item) in self.items.iter_mut().enumerate() {
            item.part = i + 1;
            item.parts = count;
        }
        Ok(self.items)
    }
}
async fn diff_sections(
    dir: &Path,
    parent: &str,
    sha: &str,
    file: &str,
    c: &CancellationToken,
) -> Result<Vec<Evidence>> {
    ensure!(!c.is_cancelled(), "Cancelled");
    let mut cmd = command(Some(dir), Access::Local);
    cmd.args([
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "--no-renames",
        "--unified=20",
        parent,
        sha,
        "--",
        file,
    ])
    .stderr(Stdio::null())
    .stdout(Stdio::piped())
    .kill_on_drop(true);
    let mut process = crate::process::Process::spawn(&mut cmd).context("Could not start Git")?;
    let child = &mut process.child;
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut sections = Sections::new(sha, file);
    let work = async {
        let mut bytes = vec![];
        loop {
            ensure!(!c.is_cancelled(), "Cancelled");
            bytes.clear();
            let n = reader.read_until(b'\n', &mut bytes).await?;
            if n == 0 {
                break;
            }
            if bytes.last() == Some(&b'\n') {
                bytes.pop();
            }
            if bytes.last() == Some(&b'\r') {
                bytes.pop();
            }
            sections.line(&String::from_utf8_lossy(&bytes))?;
        }
        ensure!(
            child.wait().await?.success(),
            "Git could not read this diff"
        );
        sections.finish()
    };
    let result = tokio::select! {biased;_=c.cancelled()=>Err(anyhow::anyhow!("Cancelled")),r=tokio::time::timeout(Duration::from_secs(180),work)=>r.context("Git diff operation timed out").and_then(|r| r)};
    if result.is_err() {
        process.stop().await;
    }
    result
}
pub fn metadata_sections(sha: &str, path: &str, text: &str) -> Result<Vec<Evidence>> {
    let mut sections = Sections::new(sha, path);
    sections.header = "Commit metadata; not executable code changes".into();
    for line in text.lines() {
        sections.push(DiffLine {
            kind: "meta".into(),
            text: line.into(),
            old_line: None,
            new_line: None,
        })?;
    }
    sections.finish()
}
pub async fn evidence(
    dir: &Path,
    commit: &Commit,
    c: &CancellationToken,
) -> Result<(Vec<Evidence>, Vec<String>)> {
    let parent = if let Some(p) = commit.parents.first() {
        p.clone()
    } else {
        git(dir, &["hash-object", "-w", "-t", "tree", "/dev/null"], c)
            .await?
            .trim()
            .into()
    };
    let mut evidence = vec![];
    let mut warnings = vec![];
    for file in &commit.files {
        match diff_sections(dir, &parent, &commit.sha, file, c).await {
            Ok(items) => evidence.extend(items),
            Err(e) => {
                ensure!(!c.is_cancelled(), "Cancelled");
                let warning = format!("Unavailable diff for {file}: {e}");
                warnings.push(warning.clone());
                evidence.push(Evidence {
                    id: hash(format!("{}:{file}:unavailable", commit.sha))[..24].into(),
                    sha: commit.sha.clone(),
                    path: file.clone(),
                    header: warning,
                    lines: vec![],
                    part: 1,
                    parts: 1,
                });
            }
        }
    }
    if evidence.is_empty() {
        evidence.push(Evidence {
            id: hash(&commit.sha)[..24].into(),
            sha: commit.sha.clone(),
            path: "(no textual changes)".into(),
            header: if commit.excluded_files > 0 {
                "No diff supplied: all changed files excluded by the file policy"
            } else {
                "Empty commit: no changed files"
            }
            .into(),
            lines: vec![],
            part: 1,
            parts: 1,
        });
    }
    Ok((evidence, warnings))
}
