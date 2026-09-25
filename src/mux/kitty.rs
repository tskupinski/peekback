use std::process::Command;

use anyhow::{Result, anyhow};

use super::{Inspection, Mux, Pane, clear_ambient, output, run_with_stdin};

pub const AMBIENT: &[&str] = &["KITTY_WINDOW_ID", "KITTY_LISTEN_ON"];

/// Without a listen address there is nothing to send through.
pub fn capture(env: &impl Fn(&str) -> Option<String>) -> Option<Pane> {
    let server = env("KITTY_LISTEN_ON")?;
    Some(Pane { mux: Mux::Kitty, server: Some(server), id: env("KITTY_WINDOW_ID")? })
}

fn kitten() -> Command {
    let mut cmd = Command::new("kitten");
    clear_ambient(&mut cmd);
    cmd
}

pub fn inspect(pane: &Pane) -> Inspection {
    let Some(server) = &pane.server else { return Inspection::Unknown };
    let Ok(reply) = output(kitten().args(["@", "--to", server, "ls"])) else {
        return Inspection::Unknown;
    };
    inspect_listing(&reply, &pane.id)
}

fn inspect_listing(reply: &str, id: &str) -> Inspection {
    #[derive(serde::Deserialize)]
    struct Window {
        tabs: Vec<Tab>,
    }
    #[derive(serde::Deserialize)]
    struct Tab {
        windows: Vec<PaneId>,
    }
    #[derive(serde::Deserialize)]
    struct PaneId {
        id: u64,
    }
    let Ok(windows) = serde_json::from_str::<Vec<Window>>(reply) else { return Inspection::Unknown };
    if windows.iter().flat_map(|w| &w.tabs).flat_map(|t| &t.windows).any(|p| p.id.to_string() == id) {
        Inspection::Available { tty: None }
    } else {
        Inspection::Unavailable
    }
}

pub fn focused(_server: Option<&str>) -> Option<String> {
    None
}

pub fn send(pane: &Pane, text: &str) -> Result<()> {
    let to = pane.server.as_deref().ok_or_else(|| anyhow!("no kitty socket recorded"))?;
    run_with_stdin(
        kitten().args([
            "@",
            "--to",
            to,
            "send-text",
            // Always bracket, so a newline can never be typed as Enter.
            "--bracketed-paste=enable",
            "--match",
            &format!("id:{}", pane.id),
            "--stdin",
        ]),
        text,
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn listing_distinguishes_absent_panes_from_unknown_ownership() {
        use super::{Inspection, inspect_listing};
        let reply = r#"[{"tabs":[{"windows":[{"id":3}]}]}]"#;
        assert_eq!(inspect_listing(reply, "3"), Inspection::Available { tty: None });
        assert_eq!(inspect_listing(reply, "4"), Inspection::Unavailable);
        assert_eq!(inspect_listing("[]", "3"), Inspection::Unavailable);
        assert_eq!(inspect_listing("not json", "3"), Inspection::Unknown);
        assert_eq!(inspect_listing("[{}]", "3"), Inspection::Unknown);
    }

    #[test]
    fn commands_ignore_the_window_they_were_started_from() {
        super::super::assert_no_ambient(&super::kitten());
    }
}
