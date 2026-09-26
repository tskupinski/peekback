use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Result, anyhow, bail};

use super::{Focused, Inspection, Pane, clear_ambient, output, run, run_with_stdin};

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

pub fn inspect(pane: &Pane) -> Inspection {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    let Ok(reply) = query(pane, "#{pane_id} #{pane_tty}") else { return Inspection::Unknown };
    let Some((id, path)) = reply.split_once(' ') else { return Inspection::Unknown };
    if id != pane.id {
        return Inspection::Unavailable;
    }
    let tty = std::fs::metadata(path).ok().filter(|m| m.file_type().is_char_device()).map(|m| m.rdev());
    Inspection::Available { tty }
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
    run(tmux(socket).args(["paste-buffer", "-p", "-r", "-b", &buffer, "-t", id, "-d"]))
}

/// The pane the most recently used client of this tmux server is on. Asked
/// per client rather than left to tmux's choice of current client, which
/// falls back to a session even when no client is attached.
pub fn focused(server: Option<&str>) -> Option<Focused> {
    let socket = server?;
    let format = "#{client_activity}\t#{pane_id}\t#{pane_pid}";
    let clients = output(tmux(socket).args(["list-clients", "-F", format])).ok()?;
    most_recent_client_pane(&clients)
}

fn most_recent_client_pane(clients: &str) -> Option<Focused> {
    clients
        .lines()
        .filter_map(|line| {
            let mut fields = line.splitn(3, '\t');
            let activity = fields.next()?.parse::<u64>().ok()?;
            let pane = fields.next().filter(|pane| !pane.is_empty())?;
            Some((activity, Focused { pane: pane.to_owned(), pid: fields.next().and_then(|pid| pid.parse().ok()) }))
        })
        .max_by_key(|(activity, _)| *activity)
        .map(|(_, focused)| focused)
}

/// The server a plain `tmux` would use, so a pane can be checked before any
/// session on it has registered.
pub fn default_server() -> Option<String> {
    let dir = std::env::var_os("TMUX_TMPDIR").filter(|d| !d.is_empty()).unwrap_or_else(|| "/tmp".into());
    let socket = std::path::Path::new(&dir).join(format!("tmux-{}", unsafe { libc::getuid() })).join("default");
    socket.canonicalize().ok().map(|path| path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tmux_commands_ignore_the_pane_they_were_started_from() {
        super::super::assert_no_ambient(&tmux("/socket"));
    }

    #[test]
    fn the_active_pane_is_the_most_recently_used_clients() {
        let clients = "1790353100\t%3\t11\n1790353127\t%921\t12\n1790353050\t%887\t13";
        assert_eq!(most_recent_client_pane(clients), Some(Focused { pane: "%921".into(), pid: Some(12) }));
        assert_eq!(most_recent_client_pane(""), None);
        assert_eq!(most_recent_client_pane("garbage\n1790353100\t\t11"), None);
    }

    #[test]
    fn the_server_is_the_socket_part_of_tmux() {
        assert_eq!(server_from("/tmp/tmux-501/default,1170,0".into()).as_deref(), Some("/tmp/tmux-501/default"));
        assert_eq!(server_from(",1,0".into()), None);
    }
}
