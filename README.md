# commit-miner

Classify Git commit diffs and messages with Jev. Bug fixes, security fixes/CWEs, and change types.

## Install

Requires Rust/Cargo and Git.

```bash
git clone https://github.com/devanshbatham/commit-miner.git
cd commit-miner
cargo install --path . --locked
export PATH="$HOME/.cargo/bin:$PATH"
export TYPESAFE_API_KEY='your-key'
```

## Use

```bash
commit-miner scan . -n 500
commit-miner scan https://github.com/owner/repo -n 500
commit-miner scan . --commit SHA_OR_REF
commit-miner scan . --since 2026-01-01 --until 2026-03-31 -o report.html
commit-miner scan . -n 500 -o report.csv
```

| Argument | Meaning |
| --- | --- |
| `-n, --commits N` | Latest N; default 250, or all matches with dates |
| `--commit SHA_OR_REF` | Exactly one commit; cannot combine with count/dates |
| `--since / --until YYYY-MM-DD` | Inclusive UTC committer dates |
| `-w, --workers N` | Default and maximum 8 |
| `-f, --format html\|csv` | Report format; stdout if no output path |
| `-o, --output PATH` | Save report; infer format from extension |
| `--only security,change_performance` | Filter displayed classifications |
| `--cwe 79,89` | Filter by CWE |
| `--plain` | Disable color and animation |

`commit-miner scan --help` lists all arguments. `commit-miner categories` lists classifications/CWEs. Filters preserve the full saved scan. Managed GitHub clones refresh automatically; local directories use their current HEAD.

New scans show estimated Jev cost from reported token usage and known model pricing. [Calculation details](docs/JEV_PROTOCOL.md#cost).

## Saved scans

```bash
commit-miner list
commit-miner show SCAN_ID --only security
commit-miner export SCAN_ID -o report.html
```

Saved locally (`~/.local/share/commit-miner` on Linux); override with `--data-dir PATH`. Ctrl+C retains completed results.

## Agent skill

```bash
commit-miner install-skill all
# Or: commit-miner install-skill codex|claude|opencode (choose one)
```

Restart your harness. The bundled [SKILL.md](skills/commit-miner/SKILL.md) includes binary installation, arguments, defaults, examples, and saved-result commands. For manual installation, copy `skills/commit-miner/` into your harness’s skills directory.
