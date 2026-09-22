use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::paths;

/// One live Claude Code session, as written by hooks/register.sh.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    pub session_id: String,
    pub transcript_path: PathBuf,
    pub cwd: PathBuf,
    pub started_at: i64,
    pub last_active_at: i64,
    #[serde(default)]
    pub terminal: Terminal,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Terminal {
    pub bundle_id: Option<String>,
    pub term_program: Option<String>,
    pub iterm_profile: Option<String>,
    pub tmux_pane: Option<String>,
    pub tmux_socket: Option<String>,
    pub kitty_window: Option<String>,
    pub kitty_listen_on: Option<String>,
    pub wezterm_pane: Option<String>,
}

impl Session {
    /// `~/.claude/projects/<slug>/`, the directory holding the transcript.
    pub fn project_dir(&self) -> Option<&Path> {
        self.transcript_path.parent()
    }

    pub fn memory_dir(&self) -> Option<PathBuf> {
        self.project_dir().map(|d| d.join("memory"))
    }

    /// The hook resets `started_at` on a resume after exit; the transcript's
    /// creation time is the earlier bound and survives that.
    pub fn started_at(&self) -> i64 {
        fs::metadata(&self.transcript_path)
            .and_then(|m| m.created())
            .ok()
            .map(unix_secs)
            .filter(|&t| t > 0)
            .map_or(self.started_at, |t| t.min(self.started_at))
    }

    pub fn scratchpad_dir(&self) -> Option<PathBuf> {
        let slug = self.project_dir()?.file_name()?;
        let uid = unsafe { libc::getuid() };
        Some(
            PathBuf::from(format!("/private/tmp/claude-{uid}"))
                .join(slug)
                .join(&self.session_id)
                .join("scratchpad"),
        )
    }
}

pub fn dir() -> PathBuf {
    paths::state_dir().join("sessions")
}

/// All registered sessions, most recently active first. Unreadable entries
/// are skipped rather than failing the whole listing.
pub fn load_all() -> Vec<Session> {
    let Ok(entries) = fs::read_dir(dir()) else { return Vec::new() };
    let mut sessions: Vec<Session> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| fs::read_to_string(e.path()).ok())
        .filter_map(|text| serde_json::from_str(&text).ok())
        .collect();
    sessions.sort_by(|a, b| b.last_active_at.cmp(&a.last_active_at));
    sessions
}

/// Ids come from hook input and page messages and become file names.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

pub fn find(session_id: &str) -> Option<Session> {
    if !valid_id(session_id) {
        return None;
    }
    let text = fs::read_to_string(dir().join(format!("{session_id}.json"))).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn find_by_pane(pane: &str) -> Option<Session> {
    load_all().into_iter().find(|s| s.terminal.tmux_pane.as_deref() == Some(pane))
}

pub fn newest() -> Option<Session> {
    load_all().into_iter().next()
}

/// Removes entries whose transcript has not changed for `max_idle_secs`.
/// A session that died without SessionEnd leaves one behind.
pub fn prune(max_idle_secs: i64) -> Vec<Session> {
    let now = now_unix();
    let mut removed = Vec::new();
    for session in load_all() {
        let last_write = fs::metadata(&session.transcript_path)
            .and_then(|m| m.modified())
            .map(unix_secs)
            .unwrap_or(0);
        if now - last_write > max_idle_secs && valid_id(&session.session_id) {
            let _ = fs::remove_file(dir().join(format!("{}.json", session.session_id)));
            removed.push(session);
        }
    }
    removed
}

pub fn now_unix() -> i64 {
    unix_secs(SystemTime::now())
}

pub fn unix_secs(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}
