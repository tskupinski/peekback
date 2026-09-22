use std::io::Write;
use std::process::{Command, Stdio};
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

/// The backend `send` would use for this session.
pub fn probe(session: &Session, pinned: Option<Backend>) -> Backend {
    pinned.unwrap_or_else(|| {
        PROBE_ORDER.into_iter().find(|b| available(*b, session)).unwrap_or(Backend::Clipboard)
    })
}

pub fn send(session: &Session, text: &str, pinned: Option<Backend>) -> Result<Outcome> {
    let text = text.trim_end_matches('\n');
    let backend = probe(session, pinned);
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
            if session.terminal.bundle_id.is_some() && !keystroke::trusted() {
                "Copied. Paste it into the prompt. Grant peekback Accessibility for direct paste.".to_string()
            } else {
                "Copied. Paste it into the prompt.".to_string()
            }
        }
    };
    Ok(Outcome { backend, note })
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
    run_with_stdin(tmux(socket).args(["load-buffer", "-b", "peekback", "-"]), text)?;
    run(tmux(socket).args(["paste-buffer", "-p", "-b", "peekback", "-t", pane, "-d"]))
}

fn send_wezterm(session: &Session, text: &str) -> Result<()> {
    let pane = session.terminal.wezterm_pane.as_deref().ok_or_else(|| anyhow!("no wezterm pane"))?;
    run_with_stdin(Command::new("wezterm").args(["cli", "send-text", "--pane-id", pane]), text)
}

fn send_kitty(session: &Session, text: &str) -> Result<()> {
    let window = session.terminal.kitty_window.as_deref().ok_or_else(|| anyhow!("no kitty window"))?;
    let to = session.terminal.kitty_listen_on.as_deref().ok_or_else(|| anyhow!("no kitty socket"))?;
    run_with_stdin(
        Command::new("kitten").args(["@", "--to", to, "send-text", "--bracketed-paste", "--match", &format!("id:{window}"), "--stdin"]),
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
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|dir| dir.join(program).is_file())
    })
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
        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
            .map_err(|_| anyhow!("event source"))?;
        for down in [true, false] {
            let event = CGEvent::new_keyboard_event(source.clone(), KEY_V, down)
                .map_err(|_| anyhow!("keyboard event"))?;
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
