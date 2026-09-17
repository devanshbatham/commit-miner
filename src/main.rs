use anyhow::{Context, Result, bail, ensure};
use clap::{Args, Parser, Subcommand};
use commit_miner::{
    export,
    filter::Filters,
    git,
    jev::{Event, Jev, MAX_WORKERS},
    miner,
    model::*,
    store::{self, Store},
    terminal::{ColorMode, Terminal, clean},
};
use futures::{StreamExt, stream};
use indicatif::{ProgressBar, ProgressStyle};
use std::{
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;
#[derive(Parser)]
#[command(
    name = "commit-miner",
    version,
    about = "Classify Git commits with Jev. Export offline diff reports."
)]
struct Cli {
    #[arg(
        long,
        global = true,
        env = "JEV_DATA_DIR",
        help = "Directory for scans, clones and cache"
    )]
    data_dir: Option<PathBuf>,
    #[arg(
        long,
        global = true,
        value_enum,
        default_value = "auto",
        help = "Terminal colors: auto, always or never; auto respects NO_COLOR"
    )]
    color: ColorMode,
    #[arg(long, global = true, help = "Disable terminal color and animation")]
    plain: bool,
    #[command(subcommand)]
    command: Commands,
}
#[derive(Subcommand)]
enum Commands {
    /// Scan a local Git directory or a GitHub repository URL
    Scan(ScanArgs),
    /// List all filterable categories and CWE IDs
    Categories,
    /// List locally saved scans
    List,
    /// Inspect a saved scan or commit
    Show {
        id: String,
        #[arg(long)]
        commit: Option<String>,
        #[command(flatten)]
        filters: Filters,
    },
    /// Export a saved scan without calling Jev
    Export {
        id: String,
        #[command(flatten)]
        output: OutputArgs,
        #[command(flatten)]
        filters: Filters,
    },
    /// Install the embedded skill for an agent harness
    InstallSkill {
        #[arg(default_value="all",value_parser=["all","codex","claude","opencode"])]
        harness: String,
    },
}
#[derive(Args)]
struct OutputArgs {
    #[arg(short='f',long,value_parser=export::format)]
    format: Option<String>,
    #[arg(short = 'o', long, help = "Output path, or - for stdout")]
    output: Option<PathBuf>,
}
impl OutputArgs {
    fn format(&self) -> String {
        self.format
            .clone()
            .or_else(|| {
                self.output
                    .as_ref()
                    .and_then(|p| p.extension())
                    .and_then(|s| s.to_str())
                    .and_then(|s| export::format(s).ok())
            })
            .unwrap_or_else(|| "html".into())
    }
    fn report_stdout(&self) -> bool {
        self.output.as_ref().is_some_and(|p| p == Path::new("-"))
            || (self.format.is_some() && self.output.is_none())
    }
    fn wanted(&self) -> bool {
        self.format.is_some() || self.output.is_some()
    }
}
#[derive(Args)]
struct ScanArgs {
    #[arg(
        help = "Local Git directory (including bare repos/worktrees) or https://github.com/owner/repo"
    )]
    source: String,
    #[arg(short='n',long="commits",value_parser=positive_count,help="Most recent N commits; default 250 without dates, all matches with dates")]
    limit: Option<usize>,
    #[arg(long, value_name="SHA_OR_REF", conflicts_with_all=["limit", "since", "until", "first_parent"], help="Classify exactly one commit by full/short SHA, tag or reference (e.g. HEAD~3)")]
    commit: Option<String>,
    #[arg(long,value_parser=valid_date,help="Inclusive UTC committer date (YYYY-MM-DD)")]
    since: Option<String>,
    #[arg(long,value_parser=valid_date,help="Inclusive UTC committer date (YYYY-MM-DD)")]
    until: Option<String>,
    #[arg(short='w',long,default_value="8",value_parser=workers,help="Parallel workers (maximum 8)")]
    workers: usize,
    #[arg(long,default_value="0.65",value_parser=threshold)]
    threshold: f64,
    #[arg(long)]
    first_parent: bool,
    #[arg(long)]
    no_cache: bool,
    #[command(flatten)]
    filters: Filters,
    #[command(flatten)]
    output: OutputArgs,
}
fn positive_count(s: &str) -> Result<usize, String> {
    let n = s
        .parse::<usize>()
        .map_err(|_| "Enter a positive commit count")?;
    if n == 0 || n > 1_000_000 {
        Err("Commit count must be 1–1000000".into())
    } else {
        Ok(n)
    }
}
fn workers(s: &str) -> Result<usize, String> {
    let n = s
        .parse::<usize>()
        .map_err(|_| "Workers must be a positive integer")?;
    if n > 0 {
        Ok(n)
    } else {
        Err("Workers must be a positive integer".into())
    }
}
fn threshold(s: &str) -> Result<f64, String> {
    let n = s
        .parse::<f64>()
        .map_err(|_| "Threshold must be 0.01–0.99")?;
    if (0.01..=0.99).contains(&n) {
        Ok(n)
    } else {
        Err("Threshold must be 0.01–0.99".into())
    }
}
fn valid_date(s: &str) -> Result<String, String> {
    git::date(s).map(|_| s.into()).map_err(|e| e.to_string())
}
fn line(text: &str) -> Result<()> {
    let mut out = io::stdout().lock();
    writeln!(out, "{text}")?;
    out.flush()?;
    Ok(())
}
struct Interrupt {
    task: tokio::task::JoinHandle<()>,
    bar: ProgressBar,
}
impl Interrupt {
    fn listen(cancel: CancellationToken, bar: &ProgressBar) -> Result<Self> {
        #[cfg(unix)]
        let mut signals =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
        let task = tokio::spawn(async move {
            let mut interrupted = false;
            loop {
                #[cfg(unix)]
                if signals.recv().await.is_none() {
                    return;
                }
                #[cfg(not(unix))]
                if tokio::signal::ctrl_c().await.is_err() {
                    return;
                }
                if interrupted {
                    commit_miner::process::kill_all();
                    std::process::exit(130);
                }
                interrupted = true;
                cancel.cancel();
            }
        });
        Ok(Self {
            task,
            bar: bar.clone(),
        })
    }
}
impl Drop for Interrupt {
    fn drop(&mut self) {
        self.task.abort();
        self.bar.finish_and_clear();
    }
}
fn export_output(
    store: &Store,
    scan: &Scan,
    o: &OutputArgs,
    filters: &Filters,
    diagnostic: &Terminal,
) -> Result<()> {
    eprintln!(
        "{}",
        diagnostic.activity(&format!("Exporting {} report", o.format().to_uppercase()))
    );
    let format = o.format();
    // CSV only needs summary records, so do not read potentially large diff sidecars.
    let scan = if format == "html" {
        filters.apply(store.complete(scan.clone())?)
    } else {
        filters.apply(scan.clone())
    };
    let rendered = export::render(&scan, &format)?;
    if let Some(path) = &o.output
        && path != Path::new("-")
    {
        store::atomic_bytes(path, rendered.as_bytes())
            .with_context(|| format!("Could not write {}", path.display()))?;
        eprintln!(
            "{}",
            diagnostic.activity(&format!("Report saved · {}", path.display()))
        );
        return Ok(());
    }
    let mut stdout = io::stdout().lock();
    stdout.write_all(rendered.as_bytes())?;
    stdout.flush()?;
    Ok(())
}
fn data_dir() -> Result<PathBuf> {
    Ok(directories::ProjectDirs::from("", "", "commit-miner")
        .context("Could not locate a user data directory; pass --data-dir")?
        .data_local_dir()
        .to_path_buf())
}
fn install_skill(harness: &str) -> Result<()> {
    let home = directories::BaseDirs::new()
        .context("Could not locate home directory")?
        .home_dir()
        .to_path_buf();
    let codex = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".codex"));
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    for (name, root) in [
        ("codex", codex.join("skills")),
        ("claude", home.join(".claude/skills")),
        ("opencode", config.join("opencode/skills")),
    ] {
        if harness != "all" && harness != name {
            continue;
        }
        let dir = root.join("commit-miner");
        let path = dir.join("SKILL.md");
        let skill = include_str!("../skills/commit-miner/SKILL.md");
        if path.exists() {
            ensure!(
                std::fs::read_to_string(&path)? == skill,
                "Existing {name} skill differs; preserve it or remove it explicitly before installing"
            );
        } else {
            store::private_dir(&dir)?;
            store::atomic_bytes(&path, skill.as_bytes())?;
        }
        line(&format!("{name}  {}", path.display()))?;
    }
    Ok(())
}
async fn scan(store: Store, args: ScanArgs, color: ColorMode, plain: bool) -> Result<i32> {
    if let (Some(a), Some(b)) = (&args.since, &args.until) {
        ensure!(a <= b, "--since must not be after --until");
    }
    let key = std::env::var("TYPESAFE_API_KEY").context("Set TYPESAFE_API_KEY before scanning")?;
    ensure!(!key.trim().is_empty(), "TYPESAFE_API_KEY is empty");
    let model = std::env::var("JEV_MODEL").unwrap_or_else(|_| "jev-latest".into());
    let source = if Path::new(&args.source).is_dir() {
        Path::new(&args.source)
            .canonicalize()?
            .to_string_lossy()
            .into_owned()
    } else {
        git::github_url(&args.source)?
    };
    let options = Options {
        source,
        commit: args.commit.clone(),
        limit: args.limit.or(
            if args.commit.is_none() && args.since.is_none() && args.until.is_none() {
                Some(250)
            } else {
                None
            },
        ),
        since: args.since,
        until: args.until,
        first_parent: args.first_parent,
        threshold: args.threshold,
        concurrency: args.workers.min(MAX_WORKERS),
        cache: !args.no_cache,
    };
    let mut scan = store.create(options.clone())?;
    let cancel = CancellationToken::new();
    let terminal = Terminal::new(color, false, plain);
    let diagnostic = Terminal::new(color, true, plain);
    let mut shown = 0;
    if !args.output.report_stdout() {
        line(&terminal.heading("commit-miner", &options.source))?;
        if args.filters.active() {
            line(&terminal.heading("Filters", &args.filters.description(options.threshold)))?;
        }
    }
    let interactive = !plain && io::stderr().is_terminal() && io::stdout().is_terminal();
    let bar = if interactive {
        let b = ProgressBar::new_spinner();
        b.set_style(ProgressStyle::with_template(if diagnostic.color {
            "  {spinner:.magenta} {msg}\n  elapsed {elapsed_precise}"
        } else {
            "  {spinner} {msg}\n  elapsed {elapsed_precise}"
        })?);
        b.set_message("Preparing repository");
        b.enable_steady_tick(Duration::from_millis(80));
        b
    } else {
        ProgressBar::hidden()
    };
    let _interrupt = Interrupt::listen(cancel.clone(), &bar)?;
    let began = Instant::now();
    let mut first_error: Option<anyhow::Error> = None;
    let activity = |message: &str| {
        bar.suspend(|| eprintln!("{}", diagnostic.activity(message)));
        bar.set_message(clean(message));
    };
    if args.workers > MAX_WORKERS {
        activity(&format!(
            "Workers capped at {MAX_WORKERS} (requested {})",
            args.workers
        ));
    }
    activity(&format!(
        "Scan {} · {} workers · {} · cache {}",
        scan.summary.id,
        options.concurrency,
        model,
        if options.cache { "on" } else { "off" }
    ));
    let transfer_log = std::sync::Mutex::new(Instant::now() - Duration::from_secs(2));
    let git_progress = |event: git::GitProgress<'_>| match event {
        git::GitProgress::Stage(message) => activity(message),
        git::GitProgress::Transfer(message) => {
            bar.set_message(clean(message));
            if !interactive {
                let mut last = transfer_log.lock().unwrap();
                if last.elapsed() >= Duration::from_secs(1) {
                    eprintln!("{}", diagnostic.activity(message));
                    *last = Instant::now();
                }
            }
        }
    };
    let prepared = async {
        let repo = git::repository_with_progress(
            &options.source,
            &store.root,
            if options.commit.is_some() {
                32
            } else {
                options.limit.unwrap_or(250) + 32
            },
            &cancel,
            &git_progress,
        )
        .await?;
        let ids = git::history_with_progress(&repo, &options, &cancel, &git_progress).await?;
        let queued = if ids.len() > 1 {
            activity("Prioritizing smaller commits");
            git::prioritize(&repo, &ids, &cancel).await?
        } else {
            ids.clone()
        };
        Ok::<_, anyhow::Error>((repo, ids, queued))
    }
    .await;
    match prepared {
        Err(e) => first_error = Some(e),
        Ok((repo, ids, queued)) => {
            let count = ids.len();
            activity(&format!(
                "Analyzing {count} {} · messages and eligible diffs",
                if count == 1 { "commit" } else { "commits" }
            ));
            activity("Saving results as commits finish");
            scan.summary.progress.total = count;
            scan.summary.progress.commits = count;
            scan.summary.progress.phase = "Classifying commits".into();
            scan.results = ids.iter().map(|sha| ResultRecord::pending(sha)).collect();
            let positions = ids
                .iter()
                .enumerate()
                .map(|(i, sha)| (sha.clone(), i))
                .collect::<std::collections::HashMap<_, _>>();
            store.save(&scan)?;
            bar.set_length(count as u64 * 1000);
            bar.set_prefix(format!("0/{count} commits"));
            bar.set_style(
                ProgressStyle::with_template(
                    if diagnostic.width<60 {"  {spinner} {prefix}\n  {wide_bar} {percent:>3}% · {elapsed_precise}"}else if diagnostic.color {"  {spinner:.magenta} {msg}\n  {wide_bar:.magenta/black} {percent:>3}% · {prefix} · elapsed {elapsed_precise}"}else{"  {spinner} {msg}\n  {wide_bar} {percent:>3}% · {prefix} · elapsed {elapsed_precise}"},
                )?
                .progress_chars("━╸─"),
            );
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let jev = Jev::new(
                key,
                model,
                store.root.clone(),
                options.cache,
                options.concurrency,
                tx.clone(),
            )?;
            let work_cancel = cancel.child_token();
            let worker_cancel = work_cancel.clone();
            let work_options = options.clone();
            let errors = Arc::new(std::sync::Mutex::new(None));
            let work_errors = errors.clone();
            let producer = tokio::spawn(async move {
                stream::iter(queued)
                    .take_while(|_| futures::future::ready(!worker_cancel.is_cancelled()))
                    .map(|sha| {
                        let (repo, o, j, c, t, e) = (
                            repo.clone(),
                            work_options.clone(),
                            jev.clone(),
                            worker_cancel.clone(),
                            tx.clone(),
                            work_errors.clone(),
                        );
                        async move {
                            if c.is_cancelled() {
                                return;
                            }
                            if let Err(err) = miner::mine(&repo, &sha, &o, &j, &c, &t).await {
                                let mut first = e.lock().unwrap();
                                if first.is_none() {
                                    *first = Some(err);
                                }
                                c.cancel();
                            }
                        }
                    })
                    .buffer_unordered(work_options.concurrency)
                    .for_each(|_| async {})
                    .await;
            });
            let mut last_save = Instant::now();
            let mut last_log = Instant::now();
            let mut heartbeat = tokio::time::interval(Duration::from_secs(1));
            heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut retry_until = Instant::now();
            let mut stopping_announced = false;
            let mut sections_by_commit = std::collections::HashMap::<String, (usize, usize)>::new();
            loop {
                let event = tokio::select! {
                    biased;
                    _ = cancel.cancelled(), if !stopping_announced => {
                        stopping_announced = true;
                        activity("Stopping · saving completed results · Ctrl+C again to force exit");
                        None
                    },
                    event = rx.recv() => match event { Some(event) => Some(event), None => break },
                    _ = heartbeat.tick() => None,
                };
                let p = &mut scan.summary.progress;
                let mut finished = false;
                if let Some(event) = event {
                    match event {
                        Event::SectionProgress { sha, done, total } => {
                            let (previous_done, previous_total) = sections_by_commit
                                .insert(sha, (done, total))
                                .unwrap_or_default();
                            p.sections += total.saturating_sub(previous_total);
                            p.reviewed_sections += done.saturating_sub(previous_done);
                        }
                        Event::CallStarted => {
                            p.calls += 1;
                            p.active += 1;
                        }
                        Event::CallFinished => p.active = p.active.saturating_sub(1),
                        Event::Cache => p.cached += 1,
                        Event::Usage {
                            model,
                            input_tokens,
                            output_tokens,
                        } => {
                            p.input_tokens += input_tokens;
                            p.output_tokens += output_tokens;
                            p.cost.get_or_insert_with(Default::default).record(
                                &model,
                                input_tokens,
                                output_tokens,
                            );
                        }
                        Event::Retry {
                            attempt,
                            status,
                            delay,
                        } => {
                            retry_until = retry_until.max(Instant::now() + delay);
                            let msg = format!(
                                "Jev retry {attempt} · {} · {:.1}s",
                                if status == 0 {
                                    "connection".into()
                                } else {
                                    format!("HTTP {status}")
                                },
                                delay.as_secs_f64()
                            );
                            if interactive {
                                bar.println(&msg);
                            } else {
                                eprintln!("{msg}");
                            }
                            scan.events.push(serde_json::json!({"id":scan.events.len()+1,"type":"call_retry","time":now(),"data":{"attempt":attempt,"status":status,"delayMs":delay.as_millis()}}));
                        }
                        Event::Commit(record) => {
                            if record.reviewed() || record.status == "failed" {
                                sections_by_commit.remove(&record.commit.sha);
                            }
                            if record.reviewed() {
                                p.done += 1;
                                p.classified += 1;
                            } else if record.status == "failed" {
                                p.done += 1;
                                p.failed += 1;
                            }
                            if record.review_coverage == "metadata" {
                                p.metadata_only += 1;
                            }
                            p.excluded_changes += record.commit.excluded_files;
                            if record
                                .probability("bug_fix")
                                .is_some_and(|p| p >= options.threshold)
                            {
                                p.bugs += 1;
                            }
                            if record
                                .probability("security_fix")
                                .is_some_and(|p| p >= options.threshold)
                            {
                                p.security += 1;
                            }
                            finished = true;
                            if let Err(e) = store.save_commit(&scan.summary.id, &record) {
                                first_error = Some(e);
                                work_cancel.cancel();
                                break;
                            }
                            let matched = args.filters.matches(&record, options.threshold);
                            if matched {
                                shown += 1;
                            }
                            if matched && !args.output.report_stdout() && !cancel.is_cancelled() {
                                let text = terminal
                                    .record(&record, args.filters.cutoff(options.threshold));
                                let write = if interactive {
                                    bar.suspend(|| line(&text))
                                } else {
                                    line(&text)
                                };
                                if let Err(e) = write {
                                    first_error = Some(e);
                                    work_cancel.cancel();
                                }
                            }
                            let mut light = *record;
                            light.evidence.clear();
                            if let Some(index) = positions.get(&light.commit.sha) {
                                scan.results[*index] = light;
                            }
                        }
                    }
                }
                // Reserve the last 10% of each commit for final classification.
                let partial: f64 = sections_by_commit
                    .values()
                    .map(|(done, total)| 0.9 * *done as f64 / (*total).max(1) as f64)
                    .sum();
                p.percent = 100. * (p.done as f64 + partial) / p.total.max(1) as f64;
                p.elapsed_seconds = began.elapsed().as_secs_f64();
                p.requests_per_second = p.calls as f64 / p.elapsed_seconds.max(1.);
                bar.set_position(((p.done as f64 + partial) * 1000.) as u64);
                bar.set_prefix(format!("{}/{} commits", p.done, p.total));
                let retry = retry_until.saturating_duration_since(Instant::now());
                let state = if cancel.is_cancelled() {
                    "Stopping · saving completed results".to_string()
                } else if !retry.is_zero() {
                    format!("Retrying Jev in {:.0}s", retry.as_secs_f64().ceil())
                } else {
                    format!("{} active · {} calls", p.active, p.calls)
                };
                bar.set_message(format!(
                    "{}/{} sections · {state} · {} cached{}",
                    p.reviewed_sections,
                    p.sections,
                    p.cached,
                    p.cost
                        .as_ref()
                        .map(|c| format!(" · {}", c.label()))
                        .unwrap_or_default()
                ));
                if !interactive && (finished || last_log.elapsed() >= Duration::from_secs(3)) {
                    eprintln!(
                        "[{}/{} commits] {}/{} sections · {state} · {} cached · elapsed {:.0}s",
                        p.done,
                        p.total,
                        p.reviewed_sections,
                        p.sections,
                        p.cached,
                        p.elapsed_seconds
                    );
                    last_log = Instant::now();
                }
                if scan.events.len() > 80 {
                    scan.events.drain(..scan.events.len() - 80);
                }
                if finished || last_save.elapsed() > Duration::from_millis(500) {
                    if let Err(e) = store.save(&scan) {
                        first_error = Some(e);
                        work_cancel.cancel();
                        break;
                    }
                    last_save = Instant::now();
                }
            }
            if let Err(e) = producer.await {
                first_error = Some(e.into());
            }
            if first_error.is_none() {
                first_error = errors.lock().unwrap().take();
            }
        }
    }
    bar.finish_and_clear();
    scan.summary.progress.active = 0;
    scan.summary.progress.elapsed_seconds = began.elapsed().as_secs_f64();
    let incomplete =
        scan.summary.progress.failed > 0 || scan.results.iter().any(|r| r.status != "complete");
    let code = if cancel.is_cancelled() {
        scan.summary.status = "cancelled".into();
        scan.error = Some("Stopped. Completed results are saved.".into());
        130
    } else if let Some(e) = first_error {
        scan.summary.status = "failed".into();
        scan.error = Some(e.to_string());
        1
    } else if incomplete {
        scan.summary.status = "incomplete".into();
        scan.error = Some(
            "Some commits or diff sections could not be analyzed. Completed results are saved."
                .into(),
        );
        1
    } else {
        scan.summary.status = "completed".into();
        scan.summary.progress.percent = 100.;
        0
    };
    scan.summary.progress.phase = if code == 0 {
        "Completed"
    } else if code == 130 {
        "Stopped"
    } else {
        "Incomplete"
    }
    .into();
    store.save(&scan)?;
    if !args.output.report_stdout() && !cancel.is_cancelled() {
        let visible = scan
            .results
            .iter()
            .filter(|r| args.filters.matches(r, options.threshold))
            .collect::<Vec<_>>();
        line(&terminal.table(&visible, args.filters.cutoff(options.threshold)))?;
    }
    eprintln!(
        "{}",
        diagnostic.summary(&scan, shown, args.filters.active())
    );

    if shown == 0 && !args.output.report_stdout() {
        line(&terminal.heading(
            if args.filters.active() {
                "No matching commits"
            } else {
                "No classified commits"
            },
            "",
        ))?;
    }
    if let Some(e) = &scan.error {
        eprintln!("{}", clean(e));
    }
    let final_bar = diagnostic.progress(&scan.summary.progress);
    eprintln!(
        "{}",
        diagnostic.activity(&format!(
            "Scan saved · {}",
            store
                .root
                .join("scans")
                .join(format!("{}.json", scan.summary.id))
                .display()
        ))
    );
    if args.output.wanted() && !cancel.is_cancelled() {
        export_output(&store, &scan, &args.output, &args.filters, &diagnostic)?;
    }
    eprintln!("{final_bar}");
    if cancel.is_cancelled() && code != 130 {
        scan.summary.status = "cancelled".into();
        scan.summary.progress.phase = "Stopped".into();
        scan.error = Some("Stopped. Completed results are saved.".into());
        store.save(&scan)?;
        return Ok(130);
    }
    Ok(code)
}
#[tokio::main]
async fn main() {
    let code = match run().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{}", clean(&format!("{e:#}")));
            1
        }
    };
    std::process::exit(code);
}
async fn run() -> Result<i32> {
    let cli = Cli::parse();
    if let Commands::InstallSkill { harness } = &cli.command {
        install_skill(harness)?;
        return Ok(0);
    }
    let terminal = Terminal::new(cli.color, false, cli.plain);
    if matches!(cli.command, Commands::Categories) {
        line(&terminal.categories())?;
        return Ok(0);
    }
    let store = Store::new(cli.data_dir.map(Ok).unwrap_or_else(data_dir)?)?;
    match cli.command {
        Commands::Scan(args) => return scan(store, args, cli.color, cli.plain).await,
        Commands::List => {
            let scans = store.list()?;
            line(&terminal.heading("Saved scans", &format!("{} scans", scans.len())))?;
            for scan in scans {
                line(&terminal.saved(&scan))?;
            }
        }
        Commands::Show {
            id,
            commit,
            filters,
        } => {
            let scan = filters.apply(store.complete(store.get(&id)?)?);
            let threshold = filters.cutoff(scan.summary.options.threshold);
            if let Some(sha) = commit {
                ensure!(
                    !sha.is_empty() && sha.chars().all(|c| c.is_ascii_hexdigit()),
                    "Invalid commit SHA"
                );
                let found = scan
                    .results
                    .iter()
                    .filter(|r| r.commit.sha.starts_with(&sha))
                    .collect::<Vec<_>>();
                ensure!(
                    found.len() == 1,
                    "Commit SHA must match exactly one commit in the selected results"
                );
                line(&terminal.record(found[0], threshold))?;
                line(&terminal.diff(found[0]))?;
            } else {
                line(&terminal.heading("commit-miner", &scan.summary.options.source))?;
                if filters.active() {
                    line(&terminal.heading(
                        "Filters",
                        &filters.description(scan.summary.options.threshold),
                    ))?;
                }
                for r in &scan.results {
                    line(&terminal.record(r, threshold))?;
                }
                if scan.results.is_empty() {
                    line(&terminal.heading("No matching commits", ""))?;
                }
                line(&terminal.summary(&scan, scan.results.len(), filters.active()))?;
            }
        }
        Commands::Export {
            id,
            output,
            filters,
        } => export_output(
            &store,
            &store.get(&id)?,
            &output,
            &filters,
            &Terminal::new(cli.color, true, cli.plain),
        )?,
        Commands::InstallSkill { .. } | Commands::Categories => bail!("Unreachable command"),
    }
    Ok(0)
}
