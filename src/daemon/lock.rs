use std::fs::{File, OpenOptions};
use std::os::unix::io::AsRawFd;
use std::path::Path;

use anyhow::{Result, bail};

/// Held for the daemon's lifetime; the OS releases it if the process dies,
/// which is why the lock and not the socket file decides "running".
pub struct Lock {
    _file: File,
}

pub fn acquire(path: &Path) -> Result<Lock> {
    let file = OpenOptions::new().create(true).truncate(false).write(true).open(path)?;
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc != 0 {
        bail!("another peekback daemon is already running");
    }
    Ok(Lock { _file: file })
}
