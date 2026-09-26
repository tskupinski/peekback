//! Built-in color themes. The page holds each theme's colors in
//! `assets/themes/`; the daemon only picks which one applies, from the config.
use serde::Serialize;

use crate::config::Config;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Appearance {
    Light,
    Dark,
}

/// Every built-in theme, as `theme = "..."` names it. A new theme is a
/// stylesheet in `assets/themes/` plus an entry here.
pub const THEMES: [(&str, Appearance); 2] = [("light", Appearance::Light), ("dark", Appearance::Dark)];

/// `auto` follows the system between `light` and `dark`.
pub const AUTO: &str = "auto";

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Theme {
    /// A built-in theme's name, or `auto`.
    pub name: String,
    pub font_family: Option<String>,
    pub font_size: Option<f64>,
}

impl Theme {
    pub fn from_config(config: &Config) -> Self {
        Self { name: config.theme.clone(), font_family: config.font.clone(), font_size: config.font_size }
    }

    /// The appearance a pinned theme gives the window, so its title bar and
    /// controls match; `None` follows the system.
    pub fn appearance(&self) -> Option<Appearance> {
        THEMES.iter().find(|(name, _)| *name == self.name).map(|(_, appearance)| *appearance)
    }
}

pub fn known(name: &str) -> bool {
    name == AUTO || THEMES.iter().any(|(theme, _)| *theme == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pinned_theme_sets_the_window_appearance_and_auto_follows_the_system() {
        let theme = |name: &str| Theme::from_config(&Config { theme: name.into(), ..Default::default() });
        assert_eq!(theme("dark").appearance(), Some(Appearance::Dark));
        assert_eq!(theme("light").appearance(), Some(Appearance::Light));
        assert_eq!(theme(AUTO).appearance(), None);
        assert!(known(AUTO) && known("light") && known("dark") && !known("gruvbox"));
    }

    #[test]
    fn every_built_in_theme_has_a_stylesheet() {
        for (name, _) in THEMES {
            let path = format!("{}/assets/themes/{name}.css", env!("CARGO_MANIFEST_DIR"));
            let css = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("missing {path}"));
            assert!(css.contains(&format!(":root[data-theme=\"{name}\"]")), "{path} must scope its tokens");
        }
    }

    #[test]
    fn font_overrides_come_from_the_config() {
        let config = Config { font: Some("My Font".into()), font_size: Some(17.0), ..Default::default() };
        let theme = Theme::from_config(&config);
        assert_eq!(theme.name, AUTO);
        assert_eq!(theme.font_family.as_deref(), Some("My Font"));
        assert_eq!(theme.font_size, Some(17.0));
    }
}
