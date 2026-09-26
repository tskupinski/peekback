//! Agent harnesses: the coding agents whose sessions Peekback records. Each one
//! knows how its agent is started, how it names its session, and how Peekback
//! is installed into it. Every `match` on a harness lives here and delegates to
//! the harness's module.

mod claude;
mod codex;

use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use session_activity::SessionKey;

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

    #[test]
    fn stored_and_command_line_names_are_the_slugs() {
        for harness in Harness::ALL {
            assert_eq!(serde_json::to_value(harness).unwrap(), harness.slug());
            assert_eq!(harness.to_possible_value().unwrap().get_name(), harness.slug());
        }
    }
}
