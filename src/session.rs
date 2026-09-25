use std::collections::HashMap;

use anyhow::{Result, anyhow, bail};

use crate::mux::{self, Mux, Pane};
use crate::registry::{self, Session};

pub const HOOK_HINT: &str = "no registered sessions; run peekback setup, then start or resume your agent";

/// Explicit id, else the session in a given tmux pane, else the session this
/// process runs inside, else the most recently active one. Flags beat the
/// environment so scripts run from inside a session can target another.
pub fn resolve(explicit: Option<&str>, pane: Option<&str>) -> Result<Session> {
    if let Some(id) = explicit {
        return registry::find(id).ok_or_else(|| anyhow::anyhow!("session {id} is not registered or has ended"));
    }
    if let Some(pane) = pane {
        return in_tmux_pane(registry::load_all(), pane, mux::tmux_server_from_env().as_deref());
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

/// The live session this process runs inside, if any. Never guesses.
pub fn inside() -> Option<Session> {
    current_id(|key| std::env::var(key).ok()).and_then(|id| registry::find(&id))
}

/// Pane ids repeat across tmux servers, so the pane is looked up on the
/// server the caller runs under, as a tmux binding's `run-shell` does. Without
/// one, the id must name a pane on a single server. Sessions come newest
/// first, so a pane still registered to a session that died without ending
/// loses to the one running there now.
fn in_tmux_pane(sessions: Vec<Session>, pane: &str, server: Option<&str>) -> Result<Session> {
    let mut matching = sessions.into_iter().filter_map(|s| {
        let found = tmux_pane(&s).filter(|p| p.id == pane && (server.is_none() || p.server.as_deref() == server));
        let found_on = found.map(|p| p.server.clone());
        found_on.map(|on| (on, s))
    });
    let Some((first_server, session)) = matching.next() else {
        bail!("no registered session in tmux pane {pane}");
    };
    if matching.any(|(other, _)| other != first_server) {
        return Err(anyhow!("tmux pane {pane} exists on several tmux servers; run this from inside tmux"));
    }
    Ok(session)
}

fn tmux_pane(session: &Session) -> Option<&Pane> {
    session.terminal.panes.iter().find(|p| p.mux == Mux::Tmux)
}

/// For callers outside any session, such as the hotkey: the session in the
/// tmux pane the user last worked in, else the most recently active one.
pub fn focused() -> Option<Session> {
    let sessions = registry::load_all();
    in_active_pane(&sessions, mux::tmux_active_pane).or_else(|| sessions.into_iter().next())
}

/// Sessions come newest first, so a pane still registered to a session that
/// died without ending loses to the one running there now.
fn in_active_pane(sessions: &[Session], mut active_pane: impl FnMut(&str) -> Option<String>) -> Option<Session> {
    let mut active = HashMap::new();
    sessions
        .iter()
        .find(|s| match tmux_pane(s) {
            Some(Pane { server: Some(socket), id, .. }) => {
                active.entry(socket.clone()).or_insert_with(|| active_pane(socket)).as_ref() == Some(id)
            }
            _ => false,
        })
        .cloned()
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

    fn session(id: &str, socket: Option<&str>, pane: Option<&str>) -> Session {
        serde_json::from_value(serde_json::json!({
            "session_id": id,
            "cwd": "/project",
            "transcript_path": null,
            "started_at": 0,
            "last_active_at": 0,
            "terminal": { "panes": match (socket, pane) {
                (Some(socket), Some(pane)) => serde_json::json!([{ "mux": "tmux", "server": socket, "id": pane }]),
                _ => serde_json::json!([]),
            } },
        }))
        .unwrap()
    }

    #[test]
    fn prefers_the_newest_session_in_each_servers_active_pane() {
        let sessions = [
            session("outside-tmux", None, None),
            session("other-pane", Some("/a"), Some("%1")),
            session("active", Some("/b"), Some("%7")),
            session("stale-in-active", Some("/b"), Some("%7")),
        ];
        let mut asked = Vec::new();
        let found = in_active_pane(&sessions, |socket| {
            asked.push(socket.to_owned());
            Some(if socket == "/a" { "%2" } else { "%7" }.into())
        });
        assert_eq!(found.map(|s| s.session_id), Some("active".into()));
        assert_eq!(asked, ["/a", "/b"]);
    }

    fn ids(found: Result<Session>) -> String {
        found.map_or_else(|e| e.to_string(), |s| s.session_id)
    }

    #[test]
    fn a_pane_is_looked_up_on_the_callers_tmux_server() {
        let sessions = || {
            vec![
                session("newest-on-a", Some("/a"), Some("%7")),
                session("on-b", Some("/b"), Some("%7")),
                session("stale-on-a", Some("/a"), Some("%7")),
                session("other", Some("/a"), Some("%1")),
            ]
        };
        // A tmux binding's run-shell job sets TMUX to its server while
        // TMUX_PANE is whatever the server inherited; only the server is used.
        assert_eq!(ids(in_tmux_pane(sessions(), "%7", Some("/b"))), "on-b");
        assert_eq!(ids(in_tmux_pane(sessions(), "%7", Some("/a"))), "newest-on-a");
        assert_eq!(ids(in_tmux_pane(sessions(), "%9", Some("/a"))), "no registered session in tmux pane %9");
        assert_eq!(
            ids(in_tmux_pane(sessions(), "%7", None)),
            "tmux pane %7 exists on several tmux servers; run this from inside tmux"
        );
        assert_eq!(ids(in_tmux_pane(sessions(), "%1", None)), "other");
    }

    #[test]
    fn finds_nothing_when_no_tmux_server_answers() {
        let sessions = [session("a", Some("/gone"), Some("%1")), session("b", Some("/gone"), Some("%2"))];
        let mut asked = 0;
        assert!(
            in_active_pane(&sessions, |_| {
                asked += 1;
                None
            })
            .is_none()
        );
        assert_eq!(asked, 1);
    }
}
