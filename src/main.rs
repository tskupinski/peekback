mod assets;
mod client;
mod config;
mod daemon;
mod discovery;
mod paths;
mod protocol;
mod registry;
mod session;

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use crate::protocol::{Request, Response};

const PRUNE_AFTER_SECS: i64 = 24 * 60 * 60;

#[derive(Parser)]
#[command(name = "peekback", about = "Rendered Markdown viewer for Claude Code sessions")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show a session's newest Markdown file, or FILE, starting the daemon if needed
    Show {
        file: Option<PathBuf>,
        /// Claude Code session id (default: the current session, else the most recent)
        #[arg(long)]
        session: Option<String>,
        /// tmux pane id whose session to show, e.g. %3
        #[arg(long)]
        pane: Option<String>,
    },
    /// Run the viewer daemon in the foreground
    Daemon,
    /// Print registered sessions and daemon state
    Status {
        /// Remove sessions whose transcript has not changed in 24 hours
        #[arg(long)]
        prune: bool,
    },
    /// Stop the daemon
    Quit,
}

fn main() -> Result<()> {
    // Let `peekback status | head` end quietly instead of panicking on EPIPE.
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
    match Cli::parse().command {
        Command::Show { file, session, pane } => show(file, session, pane),
        Command::Daemon => daemon::run(),
        Command::Status { prune } => status(prune),
        Command::Quit => {
            let _ = client::request(&Request::Quit);
            Ok(())
        }
    }
}

fn show(file: Option<PathBuf>, session: Option<String>, pane: Option<String>) -> Result<()> {
    let path = file
        .map(|f| f.canonicalize().with_context(|| format!("cannot read {}", f.display())))
        .transpose()?;
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

fn status(prune: bool) -> Result<()> {
    if prune {
        for session in registry::prune(PRUNE_AFTER_SECS) {
            println!("pruned {} ({})", session.session_id, session.cwd.display());
        }
    }
    match client::request(&Request::Status) {
        Ok(Response::Status { document, session_id }) => {
            println!("daemon: running");
            println!("session: {}", session_id.unwrap_or_else(|| "none".into()));
            println!(
                "document: {}",
                document.map(|p| p.display().to_string()).unwrap_or_else(|| "none".into())
            );
        }
        Ok(other) => bail_on(other)?,
        Err(_) => println!("daemon: not running"),
    }
    let sessions = registry::load_all();
    if sessions.is_empty() {
        println!("sessions: none ({})", session::HOOK_HINT);
    } else {
        println!("sessions:");
        let now = registry::now_unix();
        for s in sessions {
            let terminal = match (&s.terminal.tmux_pane, &s.terminal.term_program) {
                (Some(pane), _) => format!("tmux {pane}"),
                (None, Some(program)) => program.clone(),
                (None, None) => "unknown terminal".into(),
            };
            println!(
                "  {}  {}  active {}  {}",
                s.session_id,
                s.cwd.display(),
                ago(now - s.last_active_at),
                terminal
            );
        }
    }
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
