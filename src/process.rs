//! Own Git processes and their helper processes for the lifetime of an operation.
use anyhow::Result;
use std::time::Duration;
use tokio::process::{Child, Command};

#[cfg(unix)]
static GROUPS: std::sync::Mutex<std::collections::BTreeSet<i32>> =
    std::sync::Mutex::new(std::collections::BTreeSet::new());

pub(crate) struct Process {
    pub child: Child,
    #[cfg(unix)]
    group: i32,
}

impl Process {
    pub fn spawn(command: &mut Command) -> Result<Self> {
        #[cfg(unix)]
        {
            // Hold the registry lock across spawn so a forced stop cannot miss a child.
            let mut groups = GROUPS.lock().unwrap_or_else(|e| e.into_inner());
            command.process_group(0);
            let child = command.kill_on_drop(true).spawn()?;
            let group = child.id().expect("newly spawned child") as i32;
            groups.insert(group);
            Ok(Self { child, group })
        }
        #[cfg(not(unix))]
        Ok(Self {
            child: command.kill_on_drop(true).spawn()?,
        })
    }

    pub async fn stop(&mut self) {
        #[cfg(unix)]
        // Let Git remove its lockfiles before escalating to the whole group.
        unsafe {
            libc::kill(-self.group, libc::SIGTERM);
        }
        #[cfg(not(unix))]
        let _ = self.child.start_kill();
        let _ = tokio::time::timeout(Duration::from_millis(500), self.child.wait()).await;
        #[cfg(unix)]
        unsafe {
            libc::kill(-self.group, libc::SIGKILL);
        }
        let _ = self.child.start_kill();
        let _ = tokio::time::timeout(Duration::from_millis(500), self.child.wait()).await;
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            let mut groups = GROUPS.lock().unwrap_or_else(|e| e.into_inner());
            // Also covers aborted tasks and descendants still holding output pipes.
            unsafe {
                libc::kill(-self.group, libc::SIGKILL);
            }
            groups.remove(&self.group);
        }
        let _ = self.child.start_kill();
    }
}

/// Used by the second Ctrl+C before terminating the CLI immediately.
pub fn kill_all() {
    #[cfg(unix)]
    for group in GROUPS.lock().unwrap_or_else(|e| e.into_inner()).iter() {
        unsafe {
            libc::kill(-*group, libc::SIGKILL);
        }
    }
}
