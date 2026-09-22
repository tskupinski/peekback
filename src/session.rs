use anyhow::{Result, bail};

use crate::registry::{self, Session};

pub const HOOK_HINT: &str =
    "no registered sessions; wire hooks/register.sh into Claude Code settings (see hooks/settings-snippet.json)";

/// Explicit id, else the session this process runs inside, else the session
/// in a given tmux pane, else the most recently active one.
pub fn resolve(explicit: Option<&str>, pane: Option<&str>) -> Result<Session> {
    if let Some(id) = explicit {
        return registry::find(id).ok_or_else(|| anyhow::anyhow!("session {id} is not registered"));
    }
    if let Ok(id) = std::env::var("CLAUDE_CODE_SESSION_ID") {
        return registry::find(&id).ok_or_else(|| {
            anyhow::anyhow!("this session ({id}) is not registered; is hooks/register.sh wired to SessionStart?")
        });
    }
    if let Some(pane) = pane {
        return registry::find_by_pane(pane)
            .ok_or_else(|| anyhow::anyhow!("no registered session in tmux pane {pane}"));
    }
    match registry::newest() {
        Some(session) => Ok(session),
        None => bail!(HOOK_HINT),
    }
}
