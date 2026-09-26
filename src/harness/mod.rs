//! Agent harnesses: the coding agents whose sessions Peekback records. Each one
//! knows how its agent is started, how it names its session, and how Peekback
//! is installed into it. Every `match` on a harness lives here and delegates to
//! the harness's module.

mod claude;
mod codex;
pub mod hook_json;

use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use session_activity::{FileEvent, SessionKey};

use crate::record::Observation;
use crate::registry::Session;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Harness {
    Claude,
    Codex,
}

impl Harness {
    pub const ALL: [Harness; 2] = [Harness::Claude, Harness::Codex];

    /// Session environment variables are read in this order. Codex comes
    /// first: started from Claude Code's Bash tool, it inherits
    /// `CLAUDE_CODE_SESSION_ID`.
    const SESSION_ENV_PRECEDENCE: [Harness; 2] = [Harness::Codex, Harness::Claude];

    pub fn name(self) -> &'static str {
        match self {
            Harness::Claude => claude::NAME,
            Harness::Codex => codex::NAME,
        }
    }

    pub fn slug(self) -> &'static str {
        match self {
            Harness::Claude => claude::SLUG,
            Harness::Codex => codex::SLUG,
        }
    }

    /// File names the agent's executable is started as.
    pub fn executables(self) -> &'static [&'static str] {
        match self {
            Harness::Claude => claude::EXECUTABLES,
            Harness::Codex => codex::EXECUTABLES,
        }
    }

    /// Variables the agent sets to its session id for the processes it runs.
    pub fn session_env(self) -> &'static [&'static str] {
        match self {
            Harness::Claude => claude::SESSION_ENV,
            Harness::Codex => codex::SESSION_ENV,
        }
    }

    pub fn config(self) -> ConfigFile {
        match self {
            Harness::Claude => claude::CONFIG,
            Harness::Codex => codex::CONFIG,
        }
    }

    /// What one invocation of the harness's integration reports.
    pub fn decode(self, input: &Value, now: i64) -> Result<Option<Observation>> {
        match self {
            Harness::Claude | Harness::Codex => hook_json::decode(self, input, now),
        }
    }

    /// File events in a session's transcript, for harnesses that keep one
    /// Peekback can read.
    pub fn transcript_events(self, key: &SessionKey, cwd: &Path, transcript: &Path) -> Vec<FileEvent> {
        match self {
            Harness::Claude => claude::transcript_events(key, cwd, transcript),
            Harness::Codex => Vec::new(),
        }
    }

    /// When the agent last did something, according to its transcript.
    pub fn last_activity(self, transcript: &Path) -> Option<i64> {
        match self {
            Harness::Claude => claude::last_activity(transcript),
            Harness::Codex => None,
        }
    }

    /// A directory only this session writes, scanned without a time window.
    pub fn scratchpad_dir(self, session: &Session) -> Option<PathBuf> {
        match self {
            Harness::Claude => claude::scratchpad_dir(session),
            Harness::Codex => None,
        }
    }

    /// Notes the agent keeps across sessions of a project, scanned per turn.
    pub fn memory_dir(self, session: &Session) -> Option<PathBuf> {
        match self {
            Harness::Claude => claude::memory_dir(session),
            Harness::Codex => None,
        }
    }

    pub fn started_as(executable: &str) -> Option<Harness> {
        Harness::ALL.into_iter().find(|harness| harness.executables().contains(&executable))
    }

    pub fn session_env_by_precedence() -> impl Iterator<Item = &'static str> {
        Harness::SESSION_ENV_PRECEDENCE.into_iter().flat_map(Harness::session_env).copied()
    }

    pub fn key(self, session_id: impl Into<String>) -> SessionKey {
        SessionKey { agent: self.into(), session_id: session_id.into() }
    }

    /// Records written before Codex support name no harness.
    pub fn before_harness_tracking() -> Harness {
        Harness::Claude
    }
}

/// Where setup installs a harness's hooks, and what to tell the user after.
#[derive(Clone, Copy)]
pub struct ConfigFile {
    /// Overrides the configuration directory when set.
    pub dir_env: &'static str,
    /// The configuration directory under the home directory.
    pub default_dir: &'static str,
    pub file: &'static str,
    /// The hook entries to merge, with placeholder commands.
    pub template: &'static str,
    pub after_setup: &'static str,
}

impl From<Harness> for session_activity::Agent {
    fn from(harness: Harness) -> Self {
        match harness {
            Harness::Claude => session_activity::Agent::Claude,
            Harness::Codex => session_activity::Agent::Codex,
        }
    }
}

impl From<session_activity::Agent> for Harness {
    fn from(agent: session_activity::Agent) -> Self {
        match agent {
            session_activity::Agent::Claude => Harness::Claude,
            session_activity::Agent::Codex => Harness::Codex,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_harness_has_a_place_in_the_session_env_precedence() {
        for harness in Harness::ALL {
            assert_eq!(Harness::SESSION_ENV_PRECEDENCE.iter().filter(|h| **h == harness).count(), 1, "{harness:?}");
        }
    }

    #[test]
    fn codex_session_ids_win_over_an_inherited_claude_one() {
        let order: Vec<_> = Harness::session_env_by_precedence().collect();
        assert_eq!(order, ["CODEX_THREAD_ID", "CODEX_SESSION_ID", "CLAUDE_CODE_SESSION_ID"]);
    }

    #[test]
    fn harnesses_are_recognized_by_their_executable_only() {
        assert_eq!(Harness::started_as("claude"), Some(Harness::Claude));
        assert_eq!(Harness::started_as("codex"), Some(Harness::Codex));
        assert_eq!(Harness::started_as("2.1.282"), None);
        assert_eq!(Harness::started_as("node"), None);
    }

    fn session(harness: Harness, transcript: &str) -> Session {
        serde_json::from_value(serde_json::json!({
            "session_id": "s-1",
            "agent": harness.slug(),
            "cwd": "/project",
            "transcript_path": transcript,
            "started_at": 0,
            "last_active_at": 0,
        }))
        .unwrap()
    }

    #[test]
    fn only_claude_has_private_directories() {
        let claude = session(Harness::Claude, "/home/.claude/projects/-project/s-1.jsonl");
        assert_eq!(claude.memory_dir(), Some("/home/.claude/projects/-project/memory".into()));
        let scratchpad = claude.scratchpad_dir().unwrap();
        assert!(scratchpad.ends_with("-project/s-1/scratchpad"), "{}", scratchpad.display());
        let codex = session(Harness::Codex, "/home/.codex/sessions/rollout.jsonl");
        assert_eq!((codex.memory_dir(), codex.scratchpad_dir()), (None, None));
    }

    #[test]
    fn codex_transcripts_are_not_read_as_claude_ones() {
        let dir = std::env::temp_dir().join(format!("peekback-harness-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let transcript = dir.join("t.jsonl");
        let record = serde_json::json!({
            "type": "assistant",
            "timestamp": "2026-09-26T10:00:00Z",
            "cwd": "/project",
            "message": {"content": [{"type": "tool_use", "id": "t1", "name": "Write", "input": {"file_path": "a.md"}}]},
        });
        std::fs::write(&transcript, format!("{record}\n")).unwrap();
        let events =
            |harness: Harness| harness.transcript_events(&harness.key("s-1"), Path::new("/project"), &transcript);
        assert_eq!(events(Harness::Claude).len(), 1);
        assert!(events(Harness::Codex).is_empty());
        assert!(Harness::Claude.last_activity(&transcript).is_some());
        assert_eq!(Harness::Codex.last_activity(&transcript), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn setup_merges_each_harness_template_and_replaces_only_its_own_entries() {
        for harness in Harness::ALL {
            let theirs = serde_json::json!({"type": "command", "command": "other-tool"});
            let settings = serde_json::json!({"model": "x", "hooks": {"Stop": [{"hooks": [theirs]}]}});
            let once = hook_json::merge(settings, harness.config().template, ": ours; run", ": ours; ").unwrap();
            let twice = hook_json::merge(once.clone(), harness.config().template, ": ours; run", ": ours; ").unwrap();
            assert_eq!(once, twice, "{harness:?}");
            assert_eq!(once["model"], "x");
            let stop = once["hooks"]["Stop"].as_array().unwrap();
            assert_eq!(stop[0]["hooks"][0]["command"], "other-tool");
            assert_eq!(stop.last().unwrap()["hooks"][0]["command"], ": ours; run");
        }
    }

    #[test]
    fn stored_and_command_line_names_are_the_slugs() {
        for harness in Harness::ALL {
            assert_eq!(serde_json::to_value(harness).unwrap(), harness.slug());
            assert_eq!(harness.to_possible_value().unwrap().get_name(), harness.slug());
        }
    }
}
