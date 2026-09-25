use std::process::Command;

use anyhow::{Result, anyhow};

use super::{Mux, Pane, clear_ambient, on_path, run_with_stdin};

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

pub fn reachable(pane: &Pane) -> bool {
    pane.server.is_some() && on_path("kitten")
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
    fn commands_ignore_the_window_they_were_started_from() {
        super::super::assert_no_ambient(&super::kitten());
    }
}
