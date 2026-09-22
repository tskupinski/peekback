use std::path::PathBuf;

pub fn state_dir() -> PathBuf {
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
