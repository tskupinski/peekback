//! Terminal multiplexers: programs with addressable panes and a remote-control
//! interface. Each one records where a session's agent runs, tells whether that
//! pane still exists, and pastes into it.

pub(crate) mod command;
mod kitty;
mod tmux;
mod wezterm;

use std::process::Command;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use command::{output, run, run_with_stdin};
pub use tmux::server_from_env as tmux_server_from_env;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mux {
    Tmux,
    Wezterm,
    Kitty,
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
    pub const ALL: [Mux; 3] = [Mux::Tmux, Mux::Wezterm, Mux::Kitty];

    pub fn name(self) -> &'static str {
        match self {
            Mux::Tmux => "tmux",
            Mux::Wezterm => "wezterm",
            Mux::Kitty => "kitty",
        }
    }

    pub fn parse(name: &str) -> Option<Mux> {
        Mux::ALL.into_iter().find(|mux| mux.name() == name)
    }

    fn capture(self, env: &impl Fn(&str) -> Option<String>) -> Option<Pane> {
        match self {
            Mux::Tmux => tmux::capture(env),
            Mux::Wezterm => wezterm::capture(env),
            Mux::Kitty => kitty::capture(env),
        }
    }

    pub fn inspect(self, pane: &Pane) -> Inspection {
        match self {
            Mux::Tmux => tmux::inspect(pane),
            Mux::Wezterm => wezterm::inspect(pane),
            Mux::Kitty => kitty::inspect(pane),
        }
    }

    pub fn focused(self, server: Option<&str>) -> Option<String> {
        match self {
            Mux::Tmux => tmux::focused(server),
            Mux::Wezterm => wezterm::focused(server),
            Mux::Kitty => kitty::focused(server),
        }
    }

    pub fn send(self, pane: &Pane, text: &str) -> Result<()> {
        match self {
            Mux::Tmux => tmux::send(pane, text),
            Mux::Wezterm => wezterm::send(pane, text),
            Mux::Kitty => kitty::send(pane, text),
        }
    }

    /// Variables a process inherits from the pane it was started in. Acting on
    /// them from anywhere but a hook targets that pane instead of the one
    /// asked for.
    pub fn ambient_env(self) -> &'static [&'static str] {
        match self {
            Mux::Tmux => tmux::AMBIENT,
            Mux::Wezterm => wezterm::AMBIENT,
            Mux::Kitty => kitty::AMBIENT,
        }
    }
}

impl Pane {
    pub fn label(&self) -> String {
        format!("{} {}", self.mux.name(), self.id)
    }
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

    fn pane(mux: Mux, id: &str) -> Pane {
        Pane { mux, server: Some("/s".into()), id: id.into() }
    }

    #[test]
    fn captures_every_multiplexer_the_environment_names() {
        let env = |key: &str| {
            let value = match key {
                "TMUX" => "/tmp/tmux-501/default,1170,0",
                "TMUX_PANE" => "%7",
                "WEZTERM_PANE" => "3",
                "WEZTERM_UNIX_SOCKET" => "/wez.sock",
                "KITTY_WINDOW_ID" => "5",
                "KITTY_LISTEN_ON" => "unix:/kitty",
                _ => return None,
            };
            Some(value.to_owned())
        };
        assert_eq!(
            capture(env),
            [
                Pane { mux: Mux::Tmux, server: Some("/tmp/tmux-501/default".into()), id: "%7".into() },
                Pane { mux: Mux::Wezterm, server: Some("/wez.sock".into()), id: "3".into() },
                Pane { mux: Mux::Kitty, server: Some("unix:/kitty".into()), id: "5".into() },
            ]
        );
    }

    #[test]
    fn a_pane_without_its_server_is_not_captured_where_the_server_is_required() {
        let env = |key: &str| matches!(key, "TMUX_PANE" | "KITTY_WINDOW_ID" | "WEZTERM_PANE").then(|| "1".to_owned());
        assert_eq!(capture(env), [Pane { mux: Mux::Wezterm, server: None, id: "1".into() }]);
    }

    #[test]
    fn the_pane_on_the_agents_tty_comes_first_and_panes_on_other_ttys_are_dropped() {
        let panes = vec![pane(Mux::Tmux, "%1"), pane(Mux::Wezterm, "2"), pane(Mux::Kitty, "3")];
        let tty = |p: &Pane| match p.mux {
            Mux::Tmux => Some(10),
            Mux::Wezterm => None,
            Mux::Kitty => Some(20),
        };
        let order = |agent| on_agent_tty(panes.clone(), agent, tty).into_iter().map(|p| p.mux).collect::<Vec<_>>();
        assert_eq!(order(Some(20)), [Mux::Kitty, Mux::Wezterm]);
        assert_eq!(order(Some(10)), [Mux::Tmux, Mux::Wezterm]);
        // TMUX_PANE inherited by a WezTerm window opened from tmux.
        assert_eq!(order(Some(30)), [Mux::Wezterm]);
        assert_eq!(order(None), [Mux::Tmux, Mux::Wezterm, Mux::Kitty]);
    }

    #[test]
    fn a_lone_pane_on_another_tty_is_dropped() {
        // herdr started inside tmux: tmux is the only multiplexer it names.
        let tmux = vec![pane(Mux::Tmux, "%1")];
        assert!(on_agent_tty(tmux.clone(), Some(30), |_| Some(10)).is_empty());
        assert_eq!(on_agent_tty(tmux.clone(), Some(10), |_| Some(10)), tmux);
        assert_eq!(on_agent_tty(tmux.clone(), Some(30), |_| None), tmux);
    }

    #[test]
    fn backend_names_stay_the_config_names() {
        for name in ["tmux", "wezterm", "kitty"] {
            assert_eq!(Mux::parse(name).map(Mux::name), Some(name));
        }
        assert_eq!(Mux::parse("herdr"), None);
    }
}
