use std::fs;
use std::path::PathBuf;

use serde::Deserialize;

use crate::send::Backend;

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
    /// `auto` takes colors and the monospace font from the terminal the
    /// session runs in; `system` keeps the page's own light and dark
    /// palettes.
    pub theme: String,
    /// Overrides for the monospace font the terminal theme would supply.
    pub font: Option<String>,
    pub font_size: Option<f64>,
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
        Self {
            hotkey: "Cmd+Shift+M".into(),
            backend: "auto".into(),
            placement: Placement::Right,
            split: 0.5,
            theme: "auto".into(),
            font: None,
            font_size: None,
        }
    }
}

pub fn path() -> PathBuf {
    dirs::home_dir().expect("home directory").join(".config/peekback/config.toml")
}

impl Config {
    /// The pinned send backend, or None for `auto`. A bad value was already
    /// reported by `load` and replaced with the default.
    pub fn pinned_backend(&self) -> Option<Backend> {
        Backend::parse(&self.backend).unwrap_or(None)
    }
}

pub fn load() -> Config {
    let Ok(text) = fs::read_to_string(path()) else { return Config::default() };
    let mut config: Config = match toml::from_str(&text) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("ignoring {}: {e}", path().display());
            return Config::default();
        }
    };
    let defaults = Config::default();
    if let Err(e) = Backend::parse(&config.backend) {
        eprintln!("{}: {e}, using backend = \"auto\"", path().display());
        config.backend = defaults.backend;
    }
    if !(0.2..=0.9).contains(&config.split) {
        eprintln!("{}: split must be between 0.2 and 0.9, using 0.5", path().display());
        config.split = defaults.split;
    }
    config
}
