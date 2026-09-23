use std::path::PathBuf;

pub fn state_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("PEEKBACK_STATE_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    let home = dirs::home_dir().expect("home directory");
    home.join(".local/state/peekback")
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
