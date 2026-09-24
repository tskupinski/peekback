mod activity;
mod assets;
mod browse;
mod client;
mod config;
mod daemon;
mod discovery;
mod hooks;
mod lifecycle;
mod paths;
mod process;
mod protocol;
mod registry;
mod send;
mod session;
mod setup;
#[cfg(test)]
mod tests;
mod theme;

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use crate::protocol::{Request, Response};

const PRUNE_AFTER_SECS: i64 = 24 * 60 * 60;

#[derive(Parser)]
#[command(
    name = "peekback",
    version,
    about = "Browse Claude Code and Codex session files, with a native Markdown preview"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Configure hooks for detected agents, preserving existing settings
    Setup(setup::Args),
    /// Browse session files in the terminal; open Markdown in Peekback with p
    Browse(browse::Args),
    /// Collect and query session file activity independently of the viewer
    Activity {
        #[command(subcommand)]
        command: activity::Command,
    },
    /// Show a session's newest Markdown file, or FILE, starting the daemon if needed
    Show {
        file: Option<PathBuf>,
        /// Agent session id (default: the current session, else the most recent)
        #[arg(long)]
        session: Option<String>,
        /// tmux pane id whose session to show, e.g. %3
        #[arg(long)]
        pane: Option<String>,
    },
    /// Paste stdin into a session's prompt
    Send {
        #[arg(long)]
        session: Option<String>,
        #[arg(long)]
        pane: Option<String>,
    },
    /// Run the viewer daemon in the foreground
    Daemon,
    /// Print registered sessions and daemon state
    Status {
        /// Remove sessions with no hook or transcript activity in 24 hours
        #[arg(long)]
        prune: bool,
    },
    /// Hide the viewer window
    Hide,
    /// Stop the daemon
    Quit,
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Setup(args) => setup::run(args),
        Command::Browse(args) => browse::run(args),
        Command::Activity { command } => activity::run(command),
        Command::Show { file, session, pane } => show(file, session, pane),
        Command::Send { session, pane } => send(session, pane),
        Command::Daemon => daemon::run(),
        Command::Status { prune } => status(prune),
        Command::Hide => {
            let _ = client::request(&Request::Hide);
            Ok(())
        }
        Command::Quit => {
            let _ = client::request(&Request::Quit);
            Ok(())
        }
    }
}

fn show(file: Option<PathBuf>, session: Option<String>, pane: Option<String>) -> Result<()> {
    let path = file.map(|f| f.canonicalize().with_context(|| format!("cannot read {}", f.display()))).transpose()?;
    let resolved = session::resolve(session.as_deref(), pane.as_deref());
    let session_id = match (resolved, &path, session.is_some() || pane.is_some()) {
        (Ok(s), _, _) => Some(s.session_id),
        (Err(_), Some(_), false) => None,
        (Err(e), _, _) => return Err(e),
    };
    match client::request_starting_daemon(&Request::Show { session_id, path })? {
        Response::Ok => Ok(()),
        other => bail_on(other),
    }
}

fn send(session: Option<String>, pane: Option<String>) -> Result<()> {
    let target = session::resolve(session.as_deref(), pane.as_deref())?;
    let mut text = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)?;
    let outcome = send::send(&target, &text, config::load().pinned_backend())?;
    eprintln!("{} ({})", outcome.note, outcome.backend.name());
    Ok(())
}

fn status(prune: bool) -> Result<()> {
    // Only here: `peekback status | head` should end quietly. The daemon
    // must keep SIGPIPE ignored, since a client vanishing mid-reply would
    // otherwise kill it.
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
    if prune {
        for session in registry::prune(PRUNE_AFTER_SECS) {
            println!("pruned {} ({})", session.session_id, session.cwd.display());
        }
    }
    match client::request(&Request::Status) {
        Ok(Response::Status { document, session_id }) => {
            println!("daemon: running");
            println!("session: {}", session_id.unwrap_or_else(|| "none".into()));
            println!("document: {}", document.map(|p| p.display().to_string()).unwrap_or_else(|| "none".into()));
        }
        Ok(other) => bail_on(other)?,
        Err(error) => println!("daemon: unavailable ({error:#})"),
    }
    let sessions = registry::load_all();
    if sessions.is_empty() {
        println!("sessions: none ({})", session::HOOK_HINT);
    } else {
        println!("sessions:");
        let now = registry::now_unix();
        let pinned = config::load().pinned_backend();
        for s in sessions {
            let terminal = match (&s.terminal.tmux_pane, &s.terminal.term_program) {
                (Some(pane), _) => format!("tmux {pane}"),
                (None, Some(program)) => program.clone(),
                (None, None) => "unknown terminal".into(),
            };
            println!(
                "  {}  {}  {}  active {}  {}  send via {}",
                s.session_id,
                s.agent.name(),
                s.cwd.display(),
                ago(now - s.last_active_at),
                terminal,
                send::probe(&s, pinned).name()
            );
        }
    }
    let tools: Vec<&str> = ["tmux", "wezterm", "kitten"].into_iter().filter(|t| send::on_path(t)).collect();
    println!("backend tools on PATH: {}", if tools.is_empty() { "none".to_string() } else { tools.join(", ") });
    Ok(())
}

fn ago(secs: i64) -> String {
    match secs {
        s if s < 60 => "just now".into(),
        s if s < 3600 => format!("{}m ago", s / 60),
        s if s < 86400 => format!("{}h ago", s / 3600),
        s => format!("{}d ago", s / 86400),
    }
}

fn bail_on(response: Response) -> Result<()> {
    match response {
        Response::Error { message } => anyhow::bail!("{message}"),
        other => anyhow::bail!("unexpected response from daemon: {other:?}"),
    }
}
