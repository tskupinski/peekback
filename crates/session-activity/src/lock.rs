use std::fs::{File, OpenOptions};
use std::path::Path;

use anyhow::Result;

/// All cooperating readers/writers take a shared lock; maintenance takes an
/// exclusive lock. OS ownership releases it on process exit, including a crash.
pub(crate) struct Lock {
    _file: File,
}

impl Lock {
    pub fn acquire(dir: &Path, exclusive: bool) -> Result<Self> {
        let mut options = OpenOptions::new();
        options.create(true).truncate(false).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(dir.join(".lock"))?;
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            loop {
                let mode = if exclusive { libc::LOCK_EX } else { libc::LOCK_SH };
                if unsafe { libc::flock(file.as_raw_fd(), mode) } == 0 {
                    break;
                }
                let error = std::io::Error::last_os_error();
                if error.kind() != std::io::ErrorKind::Interrupted {
                    return Err(error.into());
                }
            }
        }
        #[cfg(not(unix))]
        anyhow::ensure!(!exclusive, "history maintenance currently requires Unix file locks");
        Ok(Self { _file: file })
    }
}
