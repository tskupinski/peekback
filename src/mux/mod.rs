//! Terminal multiplexers: programs with addressable panes and a remote-control
//! interface. Each one records where a session's agent runs, tells whether that
//! pane still exists, and pastes into it.

mod kitty;
mod tmux;
mod wezterm;

use std::io::Write;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

pub use tmux::{active_pane as tmux_active_pane, server_from_env as tmux_server_from_env};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mux {
    Tmux,
    Wezterm,
    Kitty,
}

/// Where a session's agent runs, in one multiplexer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pane {
    pub mux: Mux,
    /// The socket or address of the server the pane belongs to; pane ids are
    /// only unique within one server. WezTerm may not name one, and then
    /// finds its running GUI.
    pub server: Option<String>,
    pub id: String,
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

    /// The terminal device the pane runs on, when the server can tell.
    fn tty(self, pane: &Pane) -> Option<u64> {
        match self {
            Mux::Tmux => tmux::tty(pane),
            Mux::Wezterm | Mux::Kitty => None,
        }
    }

    pub fn reachable(self, pane: &Pane) -> bool {
        match self {
            Mux::Tmux => tmux::reachable(pane),
            Mux::Wezterm => wezterm::reachable(pane),
            Mux::Kitty => kitty::reachable(pane),
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

/// Every pane the environment places the agent in, innermost first.
pub fn capture(env: impl Fn(&str) -> Option<String>, agent_tty: Option<u64>) -> Vec<Pane> {
    let panes = Mux::ALL.into_iter().filter_map(|mux| mux.capture(&env)).collect();
    on_agent_tty(panes, agent_tty, |pane| pane.mux.tty(pane))
}

/// Variables leak through nesting, so they cannot tell which pane the agent
/// draws in: a WezTerm window or herdr started inside tmux hands the outer
/// `TMUX_PANE` to everything it runs. The agent's own tty can. A pane known to
/// sit on another tty is dropped, since pasting into it types into whatever
/// runs there, and the one on the agent's tty comes first. Panes whose tty
/// cannot be read keep the order of `Mux::ALL`.
fn on_agent_tty(panes: Vec<Pane>, agent_tty: Option<u64>, tty: impl Fn(&Pane) -> Option<u64>) -> Vec<Pane> {
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

fn output(cmd: &mut Command) -> Result<String> {
    let output = cmd.output().with_context(|| format!("run {:?}", cmd.get_program()))?;
    if !output.status.success() {
        bail!("{:?} failed: {}", cmd.get_program(), String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn run(cmd: &mut Command) -> Result<()> {
    output(cmd).map(drop)
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
            capture(env, None),
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
        assert_eq!(capture(env, None), [Pane { mux: Mux::Wezterm, server: None, id: "1".into() }]);
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
