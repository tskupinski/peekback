use std::process::Command;

use anyhow::Result;

use super::{Mux, Pane, clear_ambient, on_path, run_with_stdin};

pub const AMBIENT: &[&str] = &["WEZTERM_PANE", "WEZTERM_UNIX_SOCKET"];

pub fn capture(env: &impl Fn(&str) -> Option<String>) -> Option<Pane> {
    Some(Pane { mux: Mux::Wezterm, server: env("WEZTERM_UNIX_SOCKET"), id: env("WEZTERM_PANE")? })
}

/// `wezterm cli` has no socket flag: it talks to `WEZTERM_UNIX_SOCKET`, else
/// whichever GUI it finds, so the recorded socket goes back into that
/// variable rather than letting the caller's own leak in.
fn wezterm(pane: &Pane) -> Command {
    let mut cmd = Command::new("wezterm");
    clear_ambient(&mut cmd);
    if let Some(socket) = &pane.server {
        cmd.env("WEZTERM_UNIX_SOCKET", socket);
    }
    cmd
}

pub fn reachable(_: &Pane) -> bool {
    on_path("wezterm")
}

pub fn send(pane: &Pane, text: &str) -> Result<()> {
    run_with_stdin(wezterm(pane).args(["cli", "send-text", "--pane-id", &pane.id]), text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of(cmd: &Command, key: &str) -> Option<Option<String>> {
        cmd.get_envs().find(|(k, _)| *k == key).map(|(_, value)| value.map(|v| v.to_string_lossy().into_owned()))
    }

    #[test]
    fn commands_address_the_recorded_server_and_nothing_inherited() {
        let recorded = Pane { mux: Mux::Wezterm, server: Some("/wez.sock".into()), id: "3".into() };
        let cmd = wezterm(&recorded);
        assert_eq!(env_of(&cmd, "WEZTERM_UNIX_SOCKET"), Some(Some("/wez.sock".into())));
        assert_eq!(env_of(&cmd, "WEZTERM_PANE"), Some(None));
        assert_eq!(env_of(&cmd, "TMUX_PANE"), Some(None));

        let unnamed = wezterm(&Pane { server: None, ..recorded });
        super::super::assert_no_ambient(&unnamed);
    }
}
