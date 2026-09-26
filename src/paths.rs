use std::path::PathBuf;

pub fn state_dir() -> PathBuf {
    if let Some(dir) = isolated_state_dir() {
        return dir;
    }
    let home = dirs::home_dir().expect("home directory");
    home.join(".local/state/peekback")
}

/// A state directory chosen with `PEEKBACK_STATE_DIR`, for a registry and
/// daemon kept apart from the everyday one.
pub fn isolated_state_dir() -> Option<PathBuf> {
    std::env::var_os("PEEKBACK_STATE_DIR").filter(|d| !d.is_empty()).map(PathBuf::from)
}

pub fn socket_path() -> PathBuf {
    state_dir().join("daemon.sock")
}

pub fn lock_path() -> PathBuf {
    state_dir().join("daemon.lock")
}

pub fn log_path() -> PathBuf {
    state_dir().join("daemon.log")
}
