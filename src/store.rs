use crate::model::*;
use anyhow::{Context, Result, ensure};
use serde::Serialize;
use std::{
    fs,
    path::{Path, PathBuf},
};
#[derive(Clone)]
pub struct Store {
    pub root: PathBuf,
}
pub fn private_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)?;
    }
    #[cfg(not(unix))]
    fs::create_dir_all(path)?;
    Ok(())
}
pub fn atomic_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let temp = path.with_file_name(format!(".{}.tmp", uuid::Uuid::new_v4()));
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let write = (|| -> Result<()> {
        let mut f = opts.open(&temp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        fs::rename(&temp, path)?;
        Ok(())
    })();
    if write.is_err() {
        let _ = fs::remove_file(&temp);
    }
    write
}
pub fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    atomic_bytes(path, &serde_json::to_vec(value)?)
}
fn valid(id: &str) -> Result<&str> {
    ensure!(
        !id.is_empty() && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'),
        "Invalid saved result identifier"
    );
    Ok(id)
}
impl Store {
    pub fn new(root: PathBuf) -> Result<Self> {
        private_dir(&root.join("scans"))?;
        Ok(Self { root })
    }
    pub fn save(&self, scan: &Scan) -> Result<()> {
        atomic_json(
            &self
                .root
                .join("scans")
                .join(format!("{}.json", valid(&scan.summary.id)?)),
            scan,
        )
    }
    pub fn create(&self, options: Options) -> Result<Scan> {
        let scan = Scan {
            selection: None,
            owner_pid: std::process::id(),
            summary: Summary {
                id: uuid::Uuid::new_v4().to_string(),
                status: "running".into(),
                options,
                started: now(),
                progress: Progress::default(),
            },
            results: vec![],
            events: vec![],
            error: None,
        };
        self.save(&scan)?;
        Ok(scan)
    }
    pub fn get(&self, id: &str) -> Result<Scan> {
        let mut scan: Scan = serde_json::from_slice(
            &fs::read(self.root.join("scans").join(format!("{}.json", valid(id)?)))
                .context("Saved scan not found")?,
        )?;
        #[cfg(target_os = "linux")]
        if scan.summary.status == "running"
            && !Path::new(&format!("/proc/{}", scan.owner_pid)).exists()
        {
            scan.summary.status = "failed".into();
            scan.summary.progress.phase = "Interrupted".into();
            scan.summary.progress.active = 0;
            scan.error = Some(
                "The process stopped before the scan finished. Completed results are saved.".into(),
            );
            self.save(&scan)?;
        }
        Ok(scan)
    }
    pub fn list(&self) -> Result<Vec<Scan>> {
        let mut scans = vec![];
        for entry in fs::read_dir(self.root.join("scans"))? {
            let path = entry?.path();
            if path.extension().is_some_and(|e| e == "json")
                && let Some(id) = path.file_stem().and_then(|s| s.to_str())
                && let Ok(scan) = self.get(id)
            {
                scans.push(scan);
            }
        }
        scans.sort_by(|a, b| b.summary.started.cmp(&a.summary.started));
        Ok(scans)
    }
    pub fn save_commit(&self, id: &str, r: &ResultRecord) -> Result<()> {
        let dir = self
            .root
            .join("scans")
            .join(format!("{}-commits", valid(id)?));
        private_dir(&dir)?;
        atomic_json(&dir.join(format!("{}.json", valid(&r.commit.sha)?)), r)
    }
    pub fn complete(&self, mut scan: Scan) -> Result<Scan> {
        for r in &mut scan.results {
            let p = self
                .root
                .join("scans")
                .join(format!("{}-commits", valid(&scan.summary.id)?))
                .join(format!("{}.json", valid(&r.commit.sha)?));
            if p.exists() {
                *r = serde_json::from_slice(&fs::read(p)?)?;
            }
        }
        Ok(scan)
    }
}
