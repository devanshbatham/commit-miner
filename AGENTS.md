# Project instructions

Keep the commit-miner binary and viewer package versions at `0.0.1` until the first GitHub publish. Do not bump versions for local changes before that publish. Keep Cargo.toml, Cargo.lock, package.json, and package-lock.json in sync.

Preserve lightweight Jev coverage of every eligible diff section; optimize deeper review instead of skipping unseen files. Semantic classifications must come from Jev, without regex or source-text matching. Keep API concurrency capped at 8 with adaptive backoff. Optimize cost by batching shared state and reusing valid cache entries; retain category/CWE definitions and classification thresholds.
