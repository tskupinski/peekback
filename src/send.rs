use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};

use crate::registry::Session;

/// How text reaches the session's prompt, in probe order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    Tmux,
    Wezterm,
    Kitty,
    Keystroke,
    Clipboard,
}

const PROBE_ORDER: [Backend; 4] = [Backend::Tmux, Backend::Wezterm, Backend::Kitty, Backend::Keystroke];

impl Backend {
    pub fn name(self) -> &'static str {
        match self {
            Backend::Tmux => "tmux",
            Backend::Wezterm => "wezterm",
            Backend::Kitty => "kitty",
            Backend::Keystroke => "keystroke",
            Backend::Clipboard => "clipboard",
        }
    }

    /// `auto` means no pin.
    pub fn parse(name: &str) -> Result<Option<Backend>> {
        Ok(Some(match name {
            "auto" => return Ok(None),
            "tmux" => Backend::Tmux,
            "wezterm" => Backend::Wezterm,
            "kitty" => Backend::Kitty,
            "keystroke" => Backend::Keystroke,
            "clipboard" => Backend::Clipboard,
            other => bail!("unknown backend {other:?}"),
        }))
    }
}

pub struct Outcome {
    pub backend: Backend,
    /// What the user should know, in one line.
    pub note: String,
}

static NEXT_BUFFER: AtomicU64 = AtomicU64::new(0);

/// The backend `send` would use for this session.
pub fn probe(session: &Session, pinned: Option<Backend>) -> Backend {
    // The viewer can hold a session whose agent exited since it was shown. Its
    // terminal may then hold a shell, where every pasted newline would run a
    // command.
    if pinned.is_none() && !session.agent_alive() {
        return Backend::Clipboard;
    }
    pinned.unwrap_or_else(|| PROBE_ORDER.into_iter().find(|b| available(*b, session)).unwrap_or(Backend::Clipboard))
}

pub fn send(session: &Session, text: &str, pinned: Option<Backend>) -> Result<Outcome> {
    let text = paste_safe(text);
    let text = text.as_str();
    let backend = probe(session, pinned);
    if backend != Backend::Clipboard && !session.agent_alive() {
        bail!("the session's agent is no longer running, so its terminal may now hold something else");
    }
    let note = match backend {
        Backend::Tmux => {
            send_tmux(session, text)?;
            "Sent to the prompt through tmux".to_string()
        }
        Backend::Wezterm => {
            send_wezterm(session, text)?;
            "Sent to the prompt through wezterm".to_string()
        }
        Backend::Kitty => {
            send_kitty(session, text)?;
            "Sent to the prompt through kitty".to_string()
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

fn available(backend: Backend, session: &Session) -> bool {
    let t = &session.terminal;
    match backend {
        Backend::Tmux => match (&t.tmux_socket, &t.tmux_pane) {
            (Some(socket), Some(pane)) => tmux(socket)
                .args(["display-message", "-p", "-t", pane, "#{pane_id}"])
                .output()
                .is_ok_and(|o| o.status.success()),
            _ => false,
        },
        Backend::Wezterm => t.wezterm_pane.is_some() && on_path("wezterm"),
        Backend::Kitty => t.kitty_window.is_some() && t.kitty_listen_on.is_some() && on_path("kitten"),
        Backend::Keystroke => t.bundle_id.is_some() && keystroke::trusted(),
        Backend::Clipboard => true,
    }
}

fn tmux(socket: &str) -> Command {
    let mut cmd = Command::new("tmux");
    cmd.arg("-S").arg(socket);
    cmd
}

fn send_tmux(session: &Session, text: &str) -> Result<()> {
    let socket = session.terminal.tmux_socket.as_deref().ok_or_else(|| anyhow!("no tmux socket"))?;
    let pane = session.terminal.tmux_pane.as_deref().ok_or_else(|| anyhow!("no tmux pane"))?;
    // tmux brackets the paste only when the program asked for it. Without
    // that, it turns every newline into Enter.
    if text.contains('\n')
        && tmux_output(socket, &["display-message", "-p", "-t", pane, "#{bracket_paste_flag}"])? != "1"
    {
        bail!("the tmux pane does not accept pasted text, so its newlines would be typed as Enter");
    }
    let buffer = format!("peekback-{}-{}", std::process::id(), NEXT_BUFFER.fetch_add(1, Ordering::Relaxed));
    run_with_stdin(tmux(socket).args(["load-buffer", "-b", &buffer, "-"]), text)?;
    run(tmux(socket).args(["paste-buffer", "-p", "-b", &buffer, "-t", pane, "-d"]))
}

/// The pane the most recently used client of this tmux server is on.
pub fn tmux_active_pane(socket: &str) -> Option<String> {
    tmux_output(socket, &["display-message", "-p", "#{pane_id}"]).ok().filter(|pane| !pane.is_empty())
}

fn tmux_output(socket: &str, args: &[&str]) -> Result<String> {
    let output = tmux(socket).args(args).output().context("run tmux")?;
    if !output.status.success() {
        bail!("tmux failed: {}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn send_wezterm(session: &Session, text: &str) -> Result<()> {
    let pane = session.terminal.wezterm_pane.as_deref().ok_or_else(|| anyhow!("no wezterm pane"))?;
    run_with_stdin(Command::new("wezterm").args(["cli", "send-text", "--pane-id", pane]), text)
}

fn send_kitty(session: &Session, text: &str) -> Result<()> {
    let window = session.terminal.kitty_window.as_deref().ok_or_else(|| anyhow!("no kitty window"))?;
    let to = session.terminal.kitty_listen_on.as_deref().ok_or_else(|| anyhow!("no kitty socket"))?;
    run_with_stdin(
        Command::new("kitten").args([
            "@",
            "--to",
            to,
            "send-text",
            // Always bracket, so a newline can never be typed as Enter.
            "--bracketed-paste=enable",
            "--match",
            &format!("id:{window}"),
            "--stdin",
        ]),
        text,
    )
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

fn run(cmd: &mut Command) -> Result<()> {
    let output = cmd.output().with_context(|| format!("run {:?}", cmd.get_program()))?;
    if !output.status.success() {
        bail!("{:?} failed: {}", cmd.get_program(), String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(())
}

fn run_with_stdin(cmd: &mut Command, input: &str) -> Result<()> {
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("run {:?}", cmd.get_program()))?;
    child.stdin.take().expect("piped stdin").write_all(input.as_bytes())?;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        bail!("{:?} failed: {}", cmd.get_program(), String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(())
}

pub fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
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

    #[test]
    fn pasted_text_cannot_carry_control_sequences() {
        assert_eq!(paste_safe("a\r\nb\rc\t\u{1b}[201~\nd\u{7f}\u{9b}\n\n"), "a\nbc\t[201~\nd");
        assert_eq!(paste_safe("zażółć ✓"), "zażółć ✓");
    }

    #[test]
    fn a_session_whose_agent_exited_falls_back_to_the_clipboard() {
        let mut session: Session = serde_json::from_value(serde_json::json!({
            "session_id": "s", "cwd": "/tmp", "started_at": 0, "last_active_at": 0,
            "terminal": { "tmux_pane": "%1", "tmux_socket": "/nonexistent" },
        }))
        .unwrap();
        assert!(session.agent_alive()); // Entries from before agent tracking.
        session.terminal.agent = Some(ProcessId { pid: u32::MAX, started_at_us: 0 });
        assert_eq!(probe(&session, None), Backend::Clipboard);
        assert!(send(&session, "text", Some(Backend::Tmux)).is_err());
    }
}
