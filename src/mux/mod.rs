//! Terminal multiplexers: programs with addressable panes and a remote-control
//! interface. Each one records where a session's agent runs, tells whether that
//! pane still exists, and pastes into it.

pub(crate) mod command;
mod tmux;

use std::process::Command;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use command::{output, run, run_with_stdin};
pub use tmux::{default_server as tmux_default_server, server_from_env as tmux_server_from_env};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mux {
    Tmux,
}

/// A candidate destination observed in a hook environment. Ownership must be verified.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Pane {
    pub mux: Mux,
    /// The socket or address of the server the pane belongs to; pane ids are
    /// only unique within one server. A missing server cannot be verified.
    pub server: Option<String>,
    pub id: String,
}

/// The pane a server's most recently used client shows, and the process the
/// pane was started with, whose terminal tells what runs there now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Focused {
    pub pane: String,
    pub pid: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inspection {
    Available { tty: Option<u64> },
    Unavailable,
    Unknown,
}

impl Inspection {
    pub fn tty(self) -> Option<u64> {
        match self {
            Self::Available { tty } => tty,
            _ => None,
        }
    }

    pub fn matches_tty(self, agent_tty: Option<u64>) -> bool {
        matches!((self.tty(), agent_tty), (Some(pane), Some(agent)) if pane == agent)
    }
}

impl Mux {
    pub const ALL: [Mux; 1] = [Mux::Tmux];

    pub fn name(self) -> &'static str {
        match self {
            Mux::Tmux => "tmux",
        }
    }

    pub fn parse(name: &str) -> Option<Mux> {
        Mux::ALL.into_iter().find(|mux| mux.name() == name)
    }

    fn capture(self, env: &impl Fn(&str) -> Option<String>) -> Option<Pane> {
        match self {
            Mux::Tmux => tmux::capture(env),
        }
    }

    pub fn inspect(self, pane: &Pane) -> Inspection {
        match self {
            Mux::Tmux => tmux::inspect(pane),
        }
    }

    pub fn focused(self, server: Option<&str>) -> Option<Focused> {
        match self {
            Mux::Tmux => tmux::focused(server),
        }
    }

    pub fn send(self, pane: &Pane, text: &str) -> Result<()> {
        match self {
            Mux::Tmux => tmux::send(pane, text),
        }
    }

    /// Variables a process inherits from the pane it was started in. Acting on
    /// them from anywhere but a hook targets that pane instead of the one
    /// asked for.
    pub fn ambient_env(self) -> &'static [&'static str] {
        match self {
            Mux::Tmux => tmux::AMBIENT,
        }
    }
}

impl Pane {
    pub fn label(&self) -> String {
        format!("{} {}", self.mux.name(), self.id)
    }
}

/// Panes of multiplexers this build does not know, such as WezTerm and Kitty
/// from earlier versions, are skipped rather than failing the whole session.
pub fn deserialize_known<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Vec<Pane>, D::Error> {
    let entries = Vec::<serde_json::Value>::deserialize(deserializer)?;
    Ok(entries.into_iter().filter_map(|entry| serde_json::from_value(entry).ok()).collect())
}

/// Capture addresses without contacting terminal servers; sending verifies ownership.
pub fn capture(env: impl Fn(&str) -> Option<String>) -> Vec<Pane> {
    Mux::ALL.into_iter().filter_map(|mux| mux.capture(&env)).collect()
}

pub fn on_agent_tty(panes: Vec<Pane>, agent_tty: Option<u64>, mut tty: impl FnMut(&Pane) -> Option<u64>) -> Vec<Pane> {
    let Some(agent_tty) = agent_tty else { return panes };
    let (mut on_agent, unknown): (Vec<_>, Vec<_>) = panes
        .into_iter()
        .filter_map(|pane| match tty(&pane) {
            Some(device) if device != agent_tty => None,
            device => Some((pane, device.is_some())),
        })
        .partition(|(_, known)| *known);
    on_agent.extend(unknown);
    on_agent.into_iter().map(|(pane, _)| pane).collect()
}

fn clear_ambient(cmd: &mut Command) -> &mut Command {
    for key in Mux::ALL.into_iter().flat_map(|mux| mux.ambient_env()) {
        cmd.env_remove(key);
    }
    cmd
}

pub fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
}

#[cfg(test)]
fn assert_no_ambient(cmd: &Command) {
    let removed: Vec<_> = cmd.get_envs().filter(|(_, value)| value.is_none()).map(|(key, _)| key).collect();
    for key in Mux::ALL.into_iter().flat_map(|mux| mux.ambient_env()) {
        assert!(removed.contains(&std::ffi::OsStr::new(key)), "{key} is inherited by {:?}", cmd.get_program());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(server: &str, id: &str) -> Pane {
        Pane { mux: Mux::Tmux, server: Some(server.into()), id: id.into() }
    }

    #[test]
    fn captures_the_tmux_pane_and_ignores_other_terminals() {
        let env = |key: &str| {
            let value = match key {
                "TMUX" => "/tmp/tmux-501/default,1170,0",
                "TMUX_PANE" => "%7",
                "WEZTERM_PANE" => "3",
                "KITTY_WINDOW_ID" => "5",
                _ => return None,
            };
            Some(value.to_owned())
        };
        assert_eq!(capture(env), [pane("/tmp/tmux-501/default", "%7")]);
    }

    #[test]
    fn a_tmux_pane_without_its_server_is_not_captured() {
        let env = |key: &str| (key == "TMUX_PANE").then(|| "%1".to_owned());
        assert!(capture(env).is_empty());
    }

    #[test]
    fn panes_on_other_ttys_are_dropped_and_unknown_ones_follow_verified_ones() {
        let panes = vec![pane("/a", "%1"), pane("/b", "%2"), pane("/c", "%3")];
        let tty = |p: &Pane| match p.server.as_deref() {
            Some("/a") => Some(10),
            Some("/b") => None,
            _ => Some(20),
        };
        let order = |agent| on_agent_tty(panes.clone(), agent, tty).into_iter().map(|p| p.id).collect::<Vec<_>>();
        assert_eq!(order(Some(20)), ["%3", "%2"]);
        // TMUX_PANE inherited by a program started from another pane.
        assert_eq!(order(Some(30)), ["%2"]);
        assert_eq!(order(None), ["%1", "%2", "%3"]);
    }

    #[test]
    fn a_lone_pane_on_another_tty_is_dropped() {
        // herdr started inside tmux: tmux is the only multiplexer it names.
        let tmux = vec![pane("/a", "%1")];
        assert!(on_agent_tty(tmux.clone(), Some(30), |_| Some(10)).is_empty());
        assert_eq!(on_agent_tty(tmux.clone(), Some(10), |_| Some(10)), tmux);
        assert_eq!(on_agent_tty(tmux.clone(), Some(30), |_| None), tmux);
    }

    #[test]
    fn panes_of_removed_multiplexers_are_skipped_when_reading() {
        #[derive(Deserialize)]
        struct Terminal {
            #[serde(deserialize_with = "deserialize_known")]
            panes: Vec<Pane>,
        }
        let json = r#"{"panes": [
            { "mux": "wezterm", "server": "/w", "id": "3" },
            { "mux": "tmux", "server": "/a", "id": "%1" },
            { "mux": "kitty", "server": "unix:/k", "id": "5" }
        ]}"#;
        assert_eq!(serde_json::from_str::<Terminal>(json).unwrap().panes, [pane("/a", "%1")]);
    }
}
