//! Colors and font read from the terminal the session runs in, so the viewer
//! reads as an extension of it rather than a separate app.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use serde::Serialize;

use crate::config::Config;
use crate::registry::Terminal;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Theme {
    pub source: String,
    pub font_family: Option<String>,
    pub font_size: Option<f64>,
    pub background: String,
    pub foreground: String,
    /// The sixteen ANSI colors as `#rrggbb`.
    pub palette: Vec<String>,
}

pub fn detect(terminal: &Terminal, config: &Config) -> Option<Theme> {
    let detected = if config.theme != "auto" {
        None
    } else {
        match terminal.term_program.as_deref() {
            Some("ghostty") => ghostty(),
            _ => match terminal.bundle_id.as_deref() {
                Some("com.googlecode.iterm2") => iterm(terminal.iterm_profile.as_deref()),
                Some("com.mitchellh.ghostty") => ghostty(),
                _ => None,
            },
        }
    };
    let mut theme = if config.theme == "auto" { detected } else { None }.or_else(|| {
        (config.font.is_some() || config.font_size.is_some()).then(|| Theme {
            source: "system".into(),
            font_family: None,
            font_size: None,
            background: String::new(),
            foreground: String::new(),
            palette: Vec::new(),
        })
    })?;
    if config.font.is_some() {
        theme.font_family = config.font.clone();
    }
    if config.font_size.is_some() {
        theme.font_size = config.font_size;
    }
    Some(theme)
}

fn iterm(profile_name: Option<&str>) -> Option<Theme> {
    let output =
        crate::mux::command::output(Command::new("defaults").args(["export", "com.googlecode.iterm2", "-"])).ok()?;
    let root: plist::Value = plist::from_bytes(output.as_bytes()).ok()?;
    let profiles = root.as_dictionary()?.get("New Bookmarks")?.as_array()?;
    fn name(p: &plist::Value) -> Option<&str> {
        p.as_dictionary()?.get("Name")?.as_string()
    }
    let profile = profiles
        .iter()
        .find(|p| profile_name.is_some_and(|wanted| name(p) == Some(wanted)))
        .or_else(|| profiles.iter().find(|p| name(p) == Some("Default")))
        .or_else(|| profiles.first())?
        .as_dictionary()?;

    let color = |key: &str| -> Option<String> {
        let c = profile.get(key)?.as_dictionary()?;
        let channel = |k: &str| c.get(k).and_then(|v| v.as_real().or_else(|| v.as_signed_integer().map(|i| i as f64)));
        Some(hex(channel("Red Component")?, channel("Green Component")?, channel("Blue Component")?))
    };
    let palette: Option<Vec<String>> = (0..16).map(|i| color(&format!("Ansi {i} Color"))).collect();
    let (font_family, font_size) =
        profile.get("Normal Font").and_then(|f| f.as_string()).map(split_font).unwrap_or((None, None));
    Some(Theme {
        source: "iterm2".into(),
        font_family,
        font_size,
        background: color("Background Color")?,
        foreground: color("Foreground Color")?,
        palette: palette?,
    })
}

/// iTerm stores fonts as "PostScriptName size"; the name is what CSS needs.
fn split_font(spec: &str) -> (Option<String>, Option<f64>) {
    let mut parts = spec.rsplitn(2, ' ');
    let size = parts.next().and_then(|s| s.parse::<f64>().ok());
    let family = parts.next().map(str::to_string);
    match (family, size) {
        (Some(f), Some(s)) => (Some(f), Some(s)),
        _ => (Some(spec.to_string()), None),
    }
}

/// Reads the Ghostty config and the theme file it names. Untested on a
/// real Ghostty setup so far.
fn ghostty() -> Option<Theme> {
    let home = dirs::home_dir()?;
    let config_paths =
        [home.join(".config/ghostty/config"), home.join("Library/Application Support/com.mitchellh.ghostty/config")];
    let mut settings: Vec<(String, String)> = Vec::new();
    for path in &config_paths {
        if let Ok(text) = fs::read_to_string(path) {
            settings.extend(parse_ghostty(&text));
        }
    }
    let get = |key: &str| settings.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v.clone());

    let mut theme = Theme {
        source: "ghostty".into(),
        font_family: get("font-family"),
        font_size: get("font-size").and_then(|s| s.parse().ok()),
        background: "#000000".into(),
        foreground: "#ffffff".into(),
        palette: default_palette(),
    };
    let mut any_color = false;
    if let Some(name) = get("theme") {
        let name = name.split(',').next().unwrap_or(&name).trim();
        let name = name.rsplit(':').next().unwrap_or(name).trim();
        for dir in [
            home.join(".config/ghostty/themes"),
            PathBuf::from("/Applications/Ghostty.app/Contents/Resources/ghostty/themes"),
        ] {
            if let Ok(text) = fs::read_to_string(dir.join(name)) {
                any_color |= apply_ghostty(&mut theme, &parse_ghostty(&text));
                break;
            }
        }
    }
    any_color |= apply_ghostty(&mut theme, &settings);
    any_color.then_some(theme)
}

fn parse_ghostty(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.starts_with('#') {
                return None;
            }
            let (k, v) = line.split_once('=')?;
            Some((k.trim().to_string(), v.trim().trim_matches('"').to_string()))
        })
        .collect()
}

fn apply_ghostty(theme: &mut Theme, settings: &[(String, String)]) -> bool {
    let mut any = false;
    for (key, value) in settings {
        match key.as_str() {
            "background" => {
                theme.background = normalize_hex(value);
                any = true;
            }
            "foreground" => {
                theme.foreground = normalize_hex(value);
                any = true;
            }
            "palette" => {
                if let Some((index, color)) = value.split_once('=') {
                    if let Ok(i) = index.trim().parse::<usize>() {
                        if i < 16 {
                            theme.palette[i] = normalize_hex(color.trim());
                            any = true;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    any
}

fn normalize_hex(value: &str) -> String {
    let v = value.trim_start_matches('#');
    format!("#{v}")
}

fn default_palette() -> Vec<String> {
    [
        "#000000", "#cc0000", "#4e9a06", "#c4a000", "#3465a4", "#75507b", "#06989a", "#d3d7cf", "#555753", "#ef2929",
        "#8ae234", "#fce94f", "#729fcf", "#ad7fa8", "#34e2e2", "#eeeeec",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

fn hex(r: f64, g: f64, b: f64) -> String {
    let c = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", c(r), c(g), c(b))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn font_overrides_do_not_require_a_supported_terminal_or_terminal_colors() {
        for mode in ["auto", "system"] {
            let config = Config {
                theme: mode.into(),
                font: Some("My Font".into()),
                font_size: Some(17.0),
                ..Default::default()
            };
            let theme = detect(&Terminal::default(), &config).unwrap();
            assert_eq!(theme.font_family.as_deref(), Some("My Font"));
            assert_eq!(theme.font_size, Some(17.0));
            assert!(theme.palette.is_empty());
        }
    }
}
