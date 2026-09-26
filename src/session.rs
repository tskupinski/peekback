use std::collections::HashMap;

use anyhow::{Result, anyhow, bail};

use crate::mux::{self, Focused, Mux, Pane};
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
        return in_tmux_pane(matching_panes(registry::load_all()), pane, mux::tmux_server_from_env().as_deref());
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

/// What the hotkey should open.
#[derive(Debug)]
pub enum Focus {
    Session(Box<Session>),
    /// An agent runs in the focused pane but has not registered a session:
    /// Codex creates one only at its first prompt.
    Unstarted {
        agent: registry::Agent,
        pane: Pane,
    },
    Nothing,
}

/// For callers outside any session, such as the hotkey: the session in the
/// focused pane reported by an adapter, else an agent there that has not
/// started a session, else the most recently active session.
pub fn focused() -> Focus {
    let sessions = matching_panes(registry::load_all());
    let servers = mux::tmux_default_server().into_iter().map(|server| (Mux::Tmux, Some(server)));
    resolve_focus(sessions, servers, |mux, server| mux.focused(server), crate::process::foreground_agent)
}

fn resolve_focus(
    sessions: Vec<Session>,
    extra_servers: impl IntoIterator<Item = (Mux, Option<String>)>,
    mut focused: impl FnMut(Mux, Option<&str>) -> Option<Focused>,
    agent_in: impl Fn(u32) -> Option<registry::Agent>,
) -> Focus {
    let mut active = HashMap::new();
    let mut ask = |mux: Mux, server: &Option<String>| {
        active.entry((mux, server.clone())).or_insert_with(|| focused(mux, server.as_deref())).clone()
    };
    if let Some(session) = in_active_pane(&sessions, &mut ask) {
        return Focus::Session(Box::new(session));
    }
    let servers = sessions.iter().flat_map(|s| s.terminal.panes.iter().map(|p| (p.mux, p.server.clone())));
    for (mux, server) in servers.chain(extra_servers) {
        if let Some(Focused { pane, pid: Some(pid) }) = ask(mux, &server) {
            if let Some(agent) = agent_in(pid) {
                return Focus::Unstarted { agent, pane: Pane { mux, server, id: pane } };
            }
        }
    }
    sessions.into_iter().next().map_or(Focus::Nothing, |s| Focus::Session(Box::new(s)))
}

fn matching_panes(mut sessions: Vec<Session>) -> Vec<Session> {
    let mut terminals = HashMap::new();
    for session in &mut sessions {
        let tty = session.terminal.agent.and_then(|agent| agent.tty());
        session.terminal.panes = mux::on_agent_tty(std::mem::take(&mut session.terminal.panes), tty, |pane| {
            *terminals.entry(pane.clone()).or_insert_with(|| pane.mux.inspect(pane).tty())
        });
    }
    sessions
}

/// Sessions come newest first, so a pane still registered to a session that
/// died without ending loses to the one running there now.
fn in_active_pane(
    sessions: &[Session],
    mut focused: impl FnMut(Mux, &Option<String>) -> Option<Focused>,
) -> Option<Session> {
    sessions
        .iter()
        .find(|session| {
            session.terminal.panes.iter().any(|pane| focused(pane.mux, &pane.server).is_some_and(|f| f.pane == pane.id))
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

    /// A focused pane whose shell pid stands for what runs there.
    fn on(pane: &str, pid: u32) -> Option<Focused> {
        Some(Focused { pane: pane.into(), pid: Some(pid) })
    }

    const SHELL: u32 = 1;
    const CLAUDE: u32 = 2;
    const CODEX: u32 = 3;

    fn agent_in(pid: u32) -> Option<registry::Agent> {
        match pid {
            CLAUDE => Some(registry::Agent::Claude),
            CODEX => Some(registry::Agent::Codex),
            _ => None,
        }
    }

    fn focus_id(focus: Focus) -> String {
        match focus {
            Focus::Session(s) => s.session_id,
            Focus::Unstarted { agent, pane } => format!("unstarted {} in {}", agent.slug(), pane.id),
            Focus::Nothing => "nothing".into(),
        }
    }

    #[test]
    fn focus_is_asked_once_per_server() {
        let sessions = vec![
            session("first-on-a", Some("/a"), Some("%1")),
            session("second-on-a", Some("/a"), Some("%2")),
            session("on-b", Some("/b"), Some("%3")),
        ];
        let mut calls = Vec::new();
        let found = resolve_focus(
            sessions,
            [(Mux::Tmux, Some("/b".into()))],
            |mux, server| {
                calls.push((mux, server.unwrap().to_owned()));
                if server == Some("/b") { on("%3", CLAUDE) } else { None }
            },
            agent_in,
        );
        assert_eq!(focus_id(found), "on-b");
        assert_eq!(calls, [(Mux::Tmux, "/a".into()), (Mux::Tmux, "/b".into())]);
    }

    #[test]
    fn an_agent_without_a_session_in_the_focused_pane_is_opened_as_unstarted() {
        let sessions = || vec![session("elsewhere", Some("/a"), Some("%1"))];
        // A fresh Codex, which registers only at its first prompt.
        let codex = |_: Mux, _: Option<&str>| on("%9", CODEX);
        assert_eq!(focus_id(resolve_focus(sessions(), [], codex, agent_in)), "unstarted codex in %9");
        // A pane without an agent keeps the most recently active session.
        let shell = |_: Mux, _: Option<&str>| on("%9", SHELL);
        assert_eq!(focus_id(resolve_focus(sessions(), [], shell, agent_in)), "elsewhere");
        // Before any session has registered, the default server is asked.
        let fresh = |_: Mux, server: Option<&str>| if server == Some("/default") { on("%2", CLAUDE) } else { None };
        let default = [(Mux::Tmux, Some("/default".into()))];
        assert_eq!(focus_id(resolve_focus(vec![], default, fresh, agent_in)), "unstarted claude in %2");
        assert_eq!(focus_id(resolve_focus(vec![], [], |_, _| None, agent_in)), "nothing");
    }

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
        let sessions = vec![
            session("outside-tmux", None, None),
            session("other-pane", Some("/a"), Some("%1")),
            session("active", Some("/b"), Some("%7")),
            session("stale-in-active", Some("/b"), Some("%7")),
        ];
        let mut asked = Vec::new();
        let found = resolve_focus(
            sessions,
            [],
            |_, socket| {
                let socket = socket.unwrap();
                asked.push(socket.to_owned());
                on(if socket == "/a" { "%2" } else { "%7" }, CLAUDE)
            },
            agent_in,
        );
        assert_eq!(focus_id(found), "active");
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
        let sessions = vec![session("a", Some("/gone"), Some("%1")), session("b", Some("/gone"), Some("%2"))];
        let mut asked = 0;
        let found = resolve_focus(
            sessions,
            [],
            |_, _| {
                asked += 1;
                None
            },
            agent_in,
        );
        assert_eq!(focus_id(found), "a"); // The most recently active session.
        assert_eq!(asked, 1);
    }
}
