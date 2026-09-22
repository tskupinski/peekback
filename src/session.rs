use anyhow::{Result, bail};

use crate::registry::{self, Session};

pub const HOOK_HINT: &str =
    "no registered sessions; wire hooks/register.sh into Claude Code settings (see hooks/settings-snippet.json)";

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
    if let Ok(id) = std::env::var("CLAUDE_CODE_SESSION_ID") {
        return registry::find(&id).ok_or_else(|| {
            anyhow::anyhow!("this session ({id}) is not registered; is hooks/register.sh wired to SessionStart?")
        });
    }
    match registry::newest() {
        Some(session) => Ok(session),
        None => bail!(HOOK_HINT),
    }
}
