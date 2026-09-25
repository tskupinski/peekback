use std::process::Command;

use anyhow::Result;

use super::{Inspection, Mux, Pane, clear_ambient, output, run_with_stdin};

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

pub fn inspect(pane: &Pane) -> Inspection {
    if pane.server.is_none() {
        return Inspection::Unknown;
    }
    let Ok(reply) = output(wezterm(pane).args(["cli", "list", "--format", "json"])) else {
        return Inspection::Unknown;
    };
    inspect_listing(&reply, &pane.id)
}

fn inspect_listing(reply: &str, id: &str) -> Inspection {
    #[derive(serde::Deserialize)]
    struct ListedPane {
        pane_id: u64,
    }
    let Ok(panes) = serde_json::from_str::<Vec<ListedPane>>(reply) else { return Inspection::Unknown };
    if panes.iter().any(|pane| pane.pane_id.to_string() == id) {
        // The CLI listing exposes identity, but no controlling terminal.
        Inspection::Available { tty: None }
    } else {
        Inspection::Unavailable
    }
}

pub fn focused(_server: Option<&str>) -> Option<String> {
    None
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
    fn listing_distinguishes_absent_panes_from_unknown_ownership() {
        use super::{Inspection, inspect_listing};
        let reply = r#"[{"pane_id":3}]"#;
        assert_eq!(inspect_listing(reply, "3"), Inspection::Available { tty: None });
        assert_eq!(inspect_listing(reply, "4"), Inspection::Unavailable);
        assert_eq!(inspect_listing("[]", "3"), Inspection::Unavailable);
        assert_eq!(inspect_listing("not json", "3"), Inspection::Unknown);
        assert_eq!(inspect_listing("[{}]", "3"), Inspection::Unknown);
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
