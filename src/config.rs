use std::fs;
use std::path::PathBuf;

use serde::Deserialize;

/// `~/.config/peekback/config.toml`; every key is optional.
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Config {
    pub hotkey: String,
    pub backend: String,
    /// Where the window goes relative to the terminal window: `right`,
    /// `left`, `over`, or `free` for an ordinary window the user places.
    pub placement: Placement,
    /// Fraction of the terminal's width the viewer takes for `right` and
    /// `left`.
    pub split: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Placement {
    Right,
    Left,
    Over,
    Free,
}

impl Default for Config {
    fn default() -> Self {
        Self { hotkey: "Cmd+Shift+M".into(), backend: "auto".into(), placement: Placement::Right, split: 0.5 }
    }
}

pub fn path() -> PathBuf {
    dirs::home_dir().expect("home directory").join(".config/peekback/config.toml")
}

pub fn load() -> Config {
    let Ok(text) = fs::read_to_string(path()) else { return Config::default() };
    match toml::from_str(&text) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("ignoring {}: {e}", path().display());
            Config::default()
        }
    }
}
