//! Codex CLI.

use super::ConfigFile;

pub const NAME: &str = "Codex";
pub const SLUG: &str = "codex";
pub const EXECUTABLES: &[&str] = &["codex"];
pub const SESSION_ENV: &[&str] = &["CODEX_THREAD_ID", "CODEX_SESSION_ID"];
pub const CONFIG: ConfigFile = ConfigFile {
    dir_env: "CODEX_HOME",
    default_dir: ".codex",
    file: "hooks.json",
    template: include_str!("../../hooks/codex-snippet.json"),
    after_setup: "Start or resume Codex, then use /hooks to review and trust the Peekback hooks.",
};
