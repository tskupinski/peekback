use anyhow::{Result, bail};

use crate::registry::{self, Session};

pub const HOOK_HINT: &str = "no registered sessions; run peekback setup, then start or resume your agent";

/// Explicit id, else the session in a given tmux pane, else the session this
/// process runs inside, else the most recently active one. Flags beat the
/// environment so scripts run from inside a session can target another.
pub fn resolve(explicit: Option<&str>, pane: Option<&str>) -> Result<Session> {
    if let Some(id) = explicit {
        return registry::find(id).ok_or_else(|| anyhow::anyhow!("session {id} is not registered"));
    }
    if let Some(pane) = pane {
        return registry::find_by_pane(pane)
            .ok_or_else(|| anyhow::anyhow!("no registered session in tmux pane {pane}"));
    }
    if let Some(id) = current_id(|key| std::env::var(key).ok()) {
        return registry::find(&id).ok_or_else(|| {
            anyhow::anyhow!("this session ({id}) is not registered; run peekback setup and check your agent's hooks")
        });
    }
    match registry::newest() {
        Some(session) => Ok(session),
        None => bail!(HOOK_HINT),
    }
}

fn current_id(mut get: impl FnMut(&str) -> Option<String>) -> Option<String> {
    ["CODEX_THREAD_ID", "CODEX_SESSION_ID", "CLAUDE_CODE_SESSION_ID"]
        .into_iter()
        .find_map(|key| get(key).filter(|id| !id.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_codex_thread_and_legacy_session_variables() {
        for key in ["CODEX_THREAD_ID", "CODEX_SESSION_ID", "CLAUDE_CODE_SESSION_ID"] {
            assert_eq!(current_id(|k| (k == key).then(|| "session-1".into())), Some("session-1".into()));
        }
        assert_eq!(current_id(|_| Some(String::new())), None);
        assert_eq!(current_id(|k| Some(k.into())), Some("CODEX_THREAD_ID".into()));
    }
}
