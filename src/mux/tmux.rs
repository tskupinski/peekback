use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Result, anyhow, bail};

use super::{Pane, clear_ambient, output, run, run_with_stdin};

pub const AMBIENT: &[&str] = &["TMUX", "TMUX_PANE"];

static NEXT_BUFFER: AtomicU64 = AtomicU64::new(0);

pub fn capture(env: &impl Fn(&str) -> Option<String>) -> Option<Pane> {
    let server = server_from(env("TMUX")?)?;
    Some(Pane { mux: super::Mux::Tmux, server: Some(server), id: env("TMUX_PANE")? })
}

/// The server a tmux client or `run-shell` job was started by. `TMUX_PANE`
/// is not read: a `run-shell` job gets whatever the server itself inherited.
pub fn server_from_env() -> Option<String> {
    std::env::var("TMUX").ok().and_then(server_from)
}

/// `TMUX` is `socket,pid,session`.
fn server_from(value: String) -> Option<String> {
    value.split(',').next().filter(|socket| !socket.is_empty()).map(str::to_owned)
}

/// The daemon inherits the pane of whichever terminal started it, and tmux
/// resolves untargeted commands against `TMUX_PANE`, which would pin "the
/// active pane" to that pane's window.
fn tmux(socket: &str) -> Command {
    let mut cmd = Command::new("tmux");
    clear_ambient(cmd.arg("-S").arg(socket));
    cmd
}

fn target(pane: &Pane) -> Result<(&str, &str)> {
    let socket = pane.server.as_deref().ok_or_else(|| anyhow!("no tmux socket recorded"))?;
    Ok((socket, &pane.id))
}

fn query(pane: &Pane, format: &str) -> Result<String> {
    let (socket, id) = target(pane)?;
    output(tmux(socket).args(["display-message", "-p", "-t", id, format]))
}

pub fn tty(pane: &Pane) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    let path = query(pane, "#{pane_tty}").ok()?;
    std::fs::metadata(path).ok().map(|m| m.rdev())
}

pub fn reachable(pane: &Pane) -> bool {
    query(pane, "#{pane_id}").is_ok()
}

pub fn send(pane: &Pane, text: &str) -> Result<()> {
    let (socket, id) = target(pane)?;
    // tmux brackets the paste only when the program asked for it. Without
    // that, it turns every newline into Enter.
    if text.contains('\n') && query(pane, "#{bracket_paste_flag}")? != "1" {
        bail!("the tmux pane does not accept pasted text, so its newlines would be typed as Enter");
    }
    let buffer = format!("peekback-{}-{}", std::process::id(), NEXT_BUFFER.fetch_add(1, Ordering::Relaxed));
    run_with_stdin(tmux(socket).args(["load-buffer", "-b", &buffer, "-"]), text)?;
    run(tmux(socket).args(["paste-buffer", "-p", "-b", &buffer, "-t", id, "-d"]))
}

/// The pane the most recently used client of this tmux server is on.
pub fn active_pane(socket: &str) -> Option<String> {
    output(tmux(socket).args(["display-message", "-p", "#{pane_id}"])).ok().filter(|pane| !pane.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tmux_commands_ignore_the_pane_they_were_started_from() {
        super::super::assert_no_ambient(&tmux("/socket"));
    }

    #[test]
    fn the_server_is_the_socket_part_of_tmux() {
        assert_eq!(server_from("/tmp/tmux-501/default,1170,0".into()).as_deref(), Some("/tmp/tmux-501/default"));
        assert_eq!(server_from(",1,0".into()), None);
    }
}
