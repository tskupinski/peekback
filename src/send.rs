use anyhow::{Context, Result, anyhow, bail};

pub use crate::mux::on_path;
use crate::mux::{Inspection, Mux, Pane};
use crate::registry::Session;

/// Delivery preference. Automatic delivery requires a verified pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    Mux(Mux),
    Keystroke,
    Clipboard,
}

impl Backend {
    pub fn name(self) -> &'static str {
        match self {
            Backend::Mux(mux) => mux.name(),
            Backend::Keystroke => "keystroke",
            Backend::Clipboard => "clipboard",
        }
    }

    /// `auto` means no pin.
    pub fn parse(name: &str) -> Result<Option<Backend>> {
        Ok(Some(match name {
            "auto" => return Ok(None),
            "keystroke" => Backend::Keystroke,
            "clipboard" => Backend::Clipboard,
            "wezterm" | "kitty" => bail!("the {name} backend was removed; use auto, keystroke or clipboard"),
            other => Backend::Mux(Mux::parse(other).ok_or_else(|| anyhow!("unknown backend {other:?}"))?),
        }))
    }
}

pub struct Outcome {
    pub backend: Backend,
    /// What the user should know, in one line.
    pub note: String,
}

#[derive(Debug, PartialEq, Eq)]
enum Destination<'a> {
    Pane { pane: &'a Pane, tty: u64 },
    Keystroke,
    Clipboard,
}

impl Destination<'_> {
    fn backend(&self) -> Backend {
        match self {
            Self::Pane { pane, .. } => Backend::Mux(pane.mux),
            Self::Keystroke => Backend::Keystroke,
            Self::Clipboard => Backend::Clipboard,
        }
    }
}

fn resolve(session: &Session, pinned: Option<Backend>) -> Result<Destination<'_>> {
    select(session, pinned, session.terminal.agent.and_then(|agent| agent.foreground_tty()), |pane| {
        pane.mux.inspect(pane)
    })
}

fn select(
    session: &Session,
    pinned: Option<Backend>,
    agent_tty: Option<u64>,
    mut inspect: impl FnMut(&Pane) -> Inspection,
) -> Result<Destination<'_>> {
    match pinned {
        Some(Backend::Clipboard) => return Ok(Destination::Clipboard),
        Some(Backend::Keystroke) => return Ok(Destination::Keystroke),
        _ => {}
    }
    let candidate = session.terminal.panes.iter().find(|pane| {
        agent_tty.is_some()
            && pinned.is_none_or(|backend| backend == Backend::Mux(pane.mux))
            && inspect(pane).matches_tty(agent_tty)
    });
    if let Some(pane) = candidate {
        return Ok(Destination::Pane { pane, tty: agent_tty.expect("verified tty") });
    }
    if let Some(backend) = pinned {
        bail!("cannot verify that a {} pane belongs to this agent; use clipboard instead", backend.name());
    }
    Ok(Destination::Clipboard)
}

/// Status inspection never sends text or activates a terminal application.
pub fn probe(session: &Session, pinned: Option<Backend>) -> Result<Backend> {
    resolve(session, pinned).map(|destination| destination.backend())
}

pub fn send(session: &Session, text: &str, pinned: Option<Backend>) -> Result<Outcome> {
    let text = paste_safe(text);
    let text = text.as_str();
    let destination = resolve(session, pinned)?;
    let backend = destination.backend();
    if backend != Backend::Clipboard && !session.agent_alive() {
        bail!("the session's agent is no longer running, so its terminal may now hold something else");
    }
    let note = match destination {
        Destination::Pane { pane, tty } => {
            deliver(
                pane,
                tty,
                text,
                || session.terminal.agent.and_then(|agent| agent.foreground_tty()),
                |pane| pane.mux.inspect(pane),
                |pane, text| pane.mux.send(pane, text),
            )?;
            format!("Sent to the prompt through {}", pane.mux.name())
        }
        Destination::Keystroke => {
            let bundle_id = session.terminal.bundle_id.as_deref().ok_or_else(|| anyhow!("no terminal bundle id"))?;
            crate::terminal::paste_into_frontmost(bundle_id, text)?;
            "Pasted into the terminal, which is now in front".to_string()
        }
        Destination::Clipboard => {
            copy(text)?;
            if !session.agent_alive() {
                "Copied. The session's agent is no longer running, so nothing was pasted.".to_string()
            } else if pinned == Some(Backend::Clipboard) {
                "Copied. Paste it into the prompt.".to_string()
            } else {
                "Copied. No verified pane is available; paste it into the prompt.".to_string()
            }
        }
    };
    Ok(Outcome { backend, note })
}

fn deliver(
    pane: &Pane,
    tty: u64,
    text: &str,
    agent_tty: impl FnOnce() -> Option<u64>,
    inspect: impl FnOnce(&Pane) -> Inspection,
    send: impl FnOnce(&Pane, &str) -> Result<()>,
) -> Result<()> {
    let inspection = inspect(pane);
    let current_tty = agent_tty();
    if current_tty != Some(tty) || !inspection.matches_tty(current_tty) {
        bail!("the selected pane can no longer be verified; nothing was sent");
    }
    send(pane, text)
}

/// Pasted text must stay text. Escape sequences can end a bracketed paste
/// early, after which a newline submits the prompt, and a carriage return
/// submits even inside one.
fn paste_safe(text: &str) -> String {
    text.replace("\r\n", "\n")
        .chars()
        .filter(|&c| c == '\n' || c == '\t' || !c.is_control())
        .collect::<String>()
        .trim_end_matches('\n')
        .to_owned()
}

pub fn copy(text: &str) -> Result<()> {
    arboard::Clipboard::new()?.set_text(text).context("clipboard")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::ProcessId;

    fn session(terminal: serde_json::Value) -> Session {
        serde_json::from_value(serde_json::json!({
            "session_id": "s", "cwd": "/tmp", "started_at": 0, "last_active_at": 0, "terminal": terminal,
        }))
        .unwrap()
    }

    #[test]
    fn delivery_rechecks_ownership_and_propagates_partial_failure_without_retry() {
        let pane = Pane { mux: Mux::Tmux, server: Some("/s".into()), id: "%1".into() };
        for (tty, state) in [
            (None, Inspection::Available { tty: Some(10) }),
            (Some(10), Inspection::Unavailable),
            (Some(10), Inspection::Available { tty: Some(20) }),
        ] {
            assert!(deliver(&pane, 10, "text", || tty, |_| state, |_, _| panic!("must not send")).is_err());
        }
        let mut sends = 0;
        let error = deliver(
            &pane,
            10,
            "text",
            || Some(10),
            |_| Inspection::Available { tty: Some(10) },
            |target, text| {
                assert_eq!(target, &pane);
                assert_eq!(text, "text");
                sends += 1;
                bail!("partial write")
            },
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "partial write");
        assert_eq!(sends, 1);
    }

    #[test]
    fn pasted_text_cannot_carry_control_sequences() {
        assert_eq!(paste_safe("a\r\nb\rc\t\u{1b}[201~\nd\u{7f}\u{9b}\n\n"), "a\nbc\t[201~\nd");
        assert_eq!(paste_safe("zażółć ✓"), "zażółć ✓");
    }

    #[test]
    fn a_session_whose_agent_exited_falls_back_to_the_clipboard() {
        let mut session = session(serde_json::json!({
            "panes": [{ "mux": "tmux", "server": "/nonexistent", "id": "%1" }],
        }));
        assert!(session.agent_alive()); // Entries from before agent tracking.
        session.terminal.agent = Some(ProcessId { pid: u32::MAX, started_at_us: 0 });
        assert_eq!(probe(&session, None).unwrap(), Backend::Clipboard);
        assert!(send(&session, "text", Some(Backend::Mux(Mux::Tmux))).is_err());
    }

    #[test]
    fn selection_keeps_the_exact_verified_pane_and_never_guesses() {
        let session = session(serde_json::json!({
            "bundle_id": "terminal", "panes": [
                { "mux": "tmux", "server": "/outer", "id": "%9" },
                { "mux": "tmux", "server": "/wrong", "id": "%1" },
                { "mux": "tmux", "server": "/right", "id": "%2" }
            ]
        }));
        let inspect = |p: &Pane| match p.server.as_deref() {
            Some("/outer") => Inspection::Available { tty: None },
            Some("/wrong") => Inspection::Available { tty: Some(9) },
            _ => Inspection::Available { tty: Some(10) },
        };
        for pinned in [None, Some(Backend::Mux(Mux::Tmux))] {
            assert_eq!(
                select(&session, pinned, Some(10), inspect).unwrap(),
                Destination::Pane { pane: &session.terminal.panes[2], tty: 10 }
            );
        }
        for state in [Inspection::Unknown, Inspection::Unavailable, Inspection::Available { tty: None }] {
            assert_eq!(select(&session, None, Some(10), |_| state).unwrap(), Destination::Clipboard);
            assert!(select(&session, Some(Backend::Mux(Mux::Tmux)), Some(10), |_| state).is_err());
        }
        assert_eq!(select(&session, None, None, inspect).unwrap(), Destination::Clipboard);
        assert_eq!(select(&session, Some(Backend::Keystroke), None, inspect).unwrap(), Destination::Keystroke);
    }

    #[test]
    fn sessions_recorded_before_panes_still_load() {
        let old = session(serde_json::json!({ "tmux_pane": "%1", "tmux_socket": "/t", "bundle_id": "b" }));
        assert!(old.terminal.panes.is_empty());
        assert_eq!(old.terminal.bundle_id.as_deref(), Some("b"));
    }

    #[test]
    fn config_backend_names_parse_as_before() {
        for name in ["tmux", "keystroke", "clipboard"] {
            assert_eq!(Backend::parse(name).unwrap().map(Backend::name), Some(name));
        }
        assert_eq!(Backend::parse("auto").unwrap(), None);
        assert!(Backend::parse("screen").is_err());
        let removed = Backend::parse("wezterm").unwrap_err().to_string();
        assert_eq!(removed, "the wezterm backend was removed; use auto, keystroke or clipboard");
    }
}
