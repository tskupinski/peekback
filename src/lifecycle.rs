//! Cross-process serialization and durable end markers for the live registry.
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::{
    fs::{DirBuilderExt, OpenOptionsExt},
    io::AsRawFd,
};
use std::path::{Path, PathBuf};

use anyhow::Result;
use session_activity::SessionKey;

pub struct Lock {
    _file: File,
}

pub fn lock(root: &Path, id: &str) -> Result<Lock> {
    anyhow::ensure!(session_activity::valid_id(id), "invalid session id");
    let dir = root.join("lifecycle");
    ensure_dir(&dir)?;
    // The registry still uses native IDs; serialize both agents for that ID.
    // Never unlink lock files: another process may already have one open.
    let file =
        OpenOptions::new().create(true).truncate(false).write(true).mode(0o600).open(dir.join(format!("{id}.lock")))?;
    loop {
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
            break;
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error.into());
        }
    }
    Ok(Lock { _file: file })
}

fn marker(root: &Path, key: &SessionKey) -> PathBuf {
    root.join("lifecycle").join(format!("{}.{}.ended", key.agent.slug(), key.session_id))
}

pub fn ended(root: &Path, key: &SessionKey) -> bool {
    marker(root, key).exists()
}

pub fn ensure_dir(dir: &Path) -> Result<()> {
    let missing: Vec<_> =
        dir.ancestors().take_while(|p| !p.as_os_str().is_empty() && !p.exists()).map(Path::to_path_buf).collect();
    fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    for path in missing {
        File::open(&path)?.sync_all()?;
        if let Some(parent) = path.parent() {
            File::open(if parent.as_os_str().is_empty() { Path::new(".") } else { parent })?.sync_all()?;
        }
    }
    Ok(())
}

/// Caller holds the lifecycle lock. Publish the marker before removing the
/// registry record; readers also check it if the process dies between steps.
pub fn mark_ended(root: &Path, key: &SessionKey) -> Result<()> {
    key.validate()?;
    let path = marker(root, key);
    let mut file = OpenOptions::new().create(true).truncate(true).write(true).mode(0o600).open(&path)?;
    file.write_all(b"ended\n")?;
    file.sync_all()?;
    File::open(root.join("lifecycle"))?.sync_all()?;
    Ok(())
}

pub fn end(root: &Path, key: &SessionKey) -> Result<()> {
    mark_ended(root, key)?;
    let registry = root.join("sessions");
    match fs::remove_file(registry.join(format!("{}.json", key.session_id))) {
        Ok(()) => File::open(registry)?.sync_all()?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    Ok(())
}

/// Called only after publishing a SessionStart record, under the same lock.
pub fn resume(root: &Path, key: &SessionKey) -> Result<()> {
    match fs::remove_file(marker(root, key)) {
        Ok(()) => File::open(root.join("lifecycle"))?.sync_all()?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    Ok(())
}
