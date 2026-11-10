//! Running the external renderers (lualatex, soffice) safely: a concurrency cap, a timeout,
//! and on timeout the whole process group is killed (LibreOffice forks helper processes).

use std::{path::Path, process::Stdio, sync::OnceLock, time::Duration};
use tokio::{process::Command, sync::Semaphore};

pub enum RunError {
    Spawn(String),
    Failed(String),
    Timeout,
}

static SLOTS: OnceLock<Semaphore> = OnceLock::new();

pub fn init(concurrency: usize) {
    let _ = SLOTS.set(Semaphore::new(concurrency.max(1)));
}

pub async fn run(bin: &str, args: &[&str], cwd: &Path, env: &[(&str, &str)], timeout_secs: u64) -> Result<String, RunError> {
    let slots = SLOTS.get_or_init(|| Semaphore::new(2));
    let _permit = slots.acquire().await.map_err(|e| RunError::Spawn(e.to_string()))?;
    let mut cmd = Command::new(bin);
    cmd.args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("HOME", cwd)
        .process_group(0)
        .kill_on_drop(true);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let child = cmd.spawn().map_err(|e| RunError::Spawn(e.to_string()))?;
    let pid = child.id();
    match tokio::time::timeout(Duration::from_secs(timeout_secs), child.wait_with_output()).await {
        Ok(Ok(out)) => {
            let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
            if out.status.success() { Ok(text) } else { Err(RunError::Failed(text)) }
        }
        Ok(Err(e)) => Err(RunError::Failed(e.to_string())),
        Err(_) => {
            if let Some(pid) = pid {
                // SAFETY: plain kill(2) on the child's own process group.
                unsafe {
                    libc::kill(-(pid as i32), libc::SIGKILL);
                }
            }
            Err(RunError::Timeout)
        }
    }
}

/// `<bin> --version`, or None if the program is missing or broken.
pub async fn version(bin: &str) -> Option<String> {
    let dir = std::env::temp_dir();
    match run(bin, &["--version"], &dir, &[], 30).await {
        Ok(out) => out.lines().find(|l| !l.trim().is_empty()).map(|l| l.trim().to_string()),
        Err(_) => None,
    }
}
