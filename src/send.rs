use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};

pub use crate::mux::on_path;
use crate::mux::{Mux, Pane};
use crate::registry::Session;

/// How text reaches the session's prompt. Multiplexers are probed in the
/// order the session's panes were recorded, innermost first.
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
            other => Backend::Mux(Mux::parse(other).ok_or_else(|| anyhow!("unknown backend {other:?}"))?),
        }))
    }
}

pub struct Outcome {
    pub backend: Backend,
    /// What the user should know, in one line.
    pub note: String,
}

/// The backend `send` would use for this session.
pub fn probe(session: &Session, pinned: Option<Backend>) -> Backend {
    // The viewer can hold a session whose agent exited since it was shown. Its
    // terminal may then hold a shell, where every pasted newline would run a
    // command.
    if pinned.is_none() && !session.agent_alive() {
        return Backend::Clipboard;
    }
    if let Some(pinned) = pinned {
        return pinned;
    }
    let reachable = session.terminal.panes.iter().find(|pane| pane.mux.reachable(pane));
    match reachable {
        Some(pane) => Backend::Mux(pane.mux),
        None if keystroke_available(session) => Backend::Keystroke,
        None => Backend::Clipboard,
    }
}

pub fn send(session: &Session, text: &str, pinned: Option<Backend>) -> Result<Outcome> {
    let text = paste_safe(text);
    let text = text.as_str();
    let backend = probe(session, pinned);
    if backend != Backend::Clipboard && !session.agent_alive() {
        bail!("the session's agent is no longer running, so its terminal may now hold something else");
    }
    let note = match backend {
        Backend::Mux(mux) => {
            mux.send(pane_of(session, mux)?, text)?;
            format!("Sent to the prompt through {}", mux.name())
        }
        Backend::Keystroke => {
            send_keystroke(session, text)?;
            "Pasted into the terminal, which is now in front".to_string()
        }
        Backend::Clipboard => {
            copy(text)?;
            if !session.agent_alive() {
                "Copied. The session's agent is no longer running, so nothing was pasted.".to_string()
            } else if session.terminal.bundle_id.is_some() && !keystroke::trusted() {
                "Copied. Paste it into the prompt. Grant peekback Accessibility for direct paste.".to_string()
            } else {
                "Copied. Paste it into the prompt.".to_string()
            }
        }
    };
    Ok(Outcome { backend, note })
}

/// A pinned multiplexer is used wherever it sits among the session's panes,
/// so pinning tmux under another multiplexer still reaches the tmux pane.
fn pane_of(session: &Session, mux: Mux) -> Result<&Pane> {
    session
        .terminal
        .panes
        .iter()
        .find(|pane| pane.mux == mux)
        .ok_or_else(|| anyhow!("no {} pane recorded for this session", mux.name()))
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

fn keystroke_available(session: &Session) -> bool {
    session.terminal.bundle_id.is_some() && keystroke::trusted()
}

/// Puts the text on the clipboard, brings the terminal app forward, and
/// presses Cmd+V. The previous clipboard contents are restored afterwards.
fn send_keystroke(session: &Session, text: &str) -> Result<()> {
    let bundle_id = session.terminal.bundle_id.as_deref().ok_or_else(|| anyhow!("no terminal bundle id"))?;
    let mut clipboard = arboard::Clipboard::new()?;
    let previous = clipboard.get_text().ok();
    clipboard.set_text(text)?;
    keystroke::activate(bundle_id)?;
    thread::sleep(Duration::from_millis(150));
    keystroke::press_cmd_v()?;
    thread::sleep(Duration::from_millis(300));
    if let Some(previous) = previous {
        let _ = clipboard.set_text(previous);
    }
    Ok(())
}

#[cfg(target_os = "macos")]
mod keystroke {
    use anyhow::{Result, anyhow};
    use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
    use objc2_foundation::NSString;

    const KEY_V: u16 = 9;

    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXIsProcessTrusted() -> bool;
    }

    pub fn trusted() -> bool {
        unsafe { AXIsProcessTrusted() }
    }

    pub fn activate(bundle_id: &str) -> Result<()> {
        let apps = NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(bundle_id));
        let app = apps.iter().next().ok_or_else(|| anyhow!("{bundle_id} is not running"))?;
        app.activateWithOptions(NSApplicationActivationOptions::empty());
        Ok(())
    }

    pub fn press_cmd_v() -> Result<()> {
        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState).map_err(|_| anyhow!("event source"))?;
        for down in [true, false] {
            let event =
                CGEvent::new_keyboard_event(source.clone(), KEY_V, down).map_err(|_| anyhow!("keyboard event"))?;
            event.set_flags(CGEventFlags::CGEventFlagCommand);
            event.post(CGEventTapLocation::HID);
        }
        Ok(())
    }
}

#[cfg(not(target_os = "macos"))]
mod keystroke {
    use anyhow::{Result, bail};
    pub fn trusted() -> bool {
        false
    }
    pub fn activate(_: &str) -> Result<()> {
        bail!("keystroke backend is macOS only")
    }
    pub fn press_cmd_v() -> Result<()> {
        bail!("keystroke backend is macOS only")
    }
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
        assert_eq!(probe(&session, None), Backend::Clipboard);
        assert!(send(&session, "text", Some(Backend::Mux(Mux::Tmux))).is_err());
    }

    #[test]
    fn a_pinned_multiplexer_is_found_below_the_innermost_pane() {
        let session = session(serde_json::json!({
            "panes": [
                { "mux": "kitty", "server": "unix:/k", "id": "5" },
                { "mux": "tmux", "server": "/t", "id": "%1" },
            ],
        }));
        assert_eq!(pane_of(&session, Mux::Tmux).unwrap().id, "%1");
        let missing = pane_of(&session, Mux::Wezterm).unwrap_err().to_string();
        assert_eq!(missing, "no wezterm pane recorded for this session");
    }

    #[test]
    fn sessions_recorded_before_panes_still_load() {
        let old = session(serde_json::json!({ "tmux_pane": "%1", "tmux_socket": "/t", "bundle_id": "b" }));
        assert!(old.terminal.panes.is_empty());
        assert_eq!(old.terminal.bundle_id.as_deref(), Some("b"));
    }

    #[test]
    fn config_backend_names_parse_as_before() {
        for name in ["tmux", "wezterm", "kitty", "keystroke", "clipboard"] {
            assert_eq!(Backend::parse(name).unwrap().map(Backend::name), Some(name));
        }
        assert_eq!(Backend::parse("auto").unwrap(), None);
        assert!(Backend::parse("screen").is_err());
    }
}
