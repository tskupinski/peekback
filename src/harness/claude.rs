//! Claude Code.

use std::path::{Path, PathBuf};

use session_activity::{FileEvent, SessionKey};

use super::ConfigFile;
use crate::registry::Session;

pub const NAME: &str = "Claude Code";
pub const SLUG: &str = "claude";
pub const EXECUTABLES: &[&str] = &["claude"];
pub const SESSION_ENV: &[&str] = &["CLAUDE_CODE_SESSION_ID"];
pub const CONFIG: ConfigFile = ConfigFile {
    dir_env: "CLAUDE_CONFIG_DIR",
    default_dir: ".claude",
    file: "settings.json",
    template: include_str!("../../hooks/settings-snippet.json"),
    after_setup: "Start or resume Claude Code; review /hooks if prompted.",
};

/// `~/.claude/projects/<slug>/`, the directory holding the transcript.
fn project_dir(session: &Session) -> Option<&Path> {
    session.transcript_path.as_deref()?.parent()
}

pub fn memory_dir(session: &Session) -> Option<PathBuf> {
    project_dir(session).map(|dir| dir.join("memory"))
}

pub fn scratchpad_dir(session: &Session) -> Option<PathBuf> {
    let slug = project_dir(session)?.file_name()?;
    let uid = unsafe { libc::getuid() };
    Some(PathBuf::from(format!("/private/tmp/claude-{uid}")).join(slug).join(&session.session_id).join("scratchpad"))
}

/// The main transcript and those of its subagents, which run without hooks.
pub fn transcript_events(key: &SessionKey, cwd: &Path, transcript: &Path) -> Vec<FileEvent> {
    std::iter::once(transcript.to_path_buf())
        .chain(session_activity::subagent_transcripts(transcript))
        .flat_map(|transcript| session_activity::transcript_events(key, cwd, &transcript))
        .collect()
}

pub fn last_activity(transcript: &Path) -> Option<i64> {
    session_activity::transcript_last_activity(transcript)
}
