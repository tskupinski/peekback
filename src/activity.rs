use anyhow::Result;
use clap::Subcommand;
use session_activity::{Agent, FileEvent, SessionKey, Store};

use crate::{paths, registry};

#[derive(Subcommand)]
pub enum Command {
    /// Retry captures retained after a failed hook or session exit
    Retry {
        #[arg(long, value_parser = ["claude", "codex"])]
        agent: String,
        #[arg(long)]
        session: String,
    },
    /// Preview packing raw history into fewer files without dropping events
    Compact {
        #[arg(long, value_parser = ["claude", "codex"])]
        agent: String,
        #[arg(long)]
        session: String,
        /// Commit the previewed maintenance operation
        #[arg(long)]
        apply: bool,
    },
    /// Preview removing events older than a Unix timestamp (seconds)
    Retain {
        #[arg(long, value_parser = ["claude", "codex"])]
        agent: String,
        #[arg(long)]
        session: String,
        #[arg(long, value_parser = clap::value_parser!(i64).range(0..))]
        before: i64,
        /// Permanently remove older events and compact retained history
        #[arg(long)]
        apply: bool,
    },
    /// Record a Claude Code or Codex hook from stdin (silent on success)
    Record {
        #[arg(long, value_parser = ["claude", "codex"])]
        agent: String,
    },
    /// Print retained file events as JSON, including ended sessions
    Events {
        #[arg(long, value_parser = ["claude", "codex"])]
        agent: String,
        #[arg(long)]
        session: String,
    },
    /// Print files and their evidence as JSON (all file types)
    Files {
        #[arg(long, value_parser = ["claude", "codex"])]
        agent: String,
        #[arg(long)]
        session: String,
    },
}

pub fn store() -> Store {
    Store::new(paths::state_dir().join("activity"))
}

pub fn run(command: Command) -> Result<()> {
    match command {
        Command::Retry { agent, session } => {
            let key = SessionKey { agent: parse_agent(&agent), session_id: session };
            let root = paths::state_dir();
            let _lock = crate::lifecycle::lock(&root, &key.session_id)?;
            crate::capture_jobs::retry(&root, &key)
        }
        Command::Compact { agent, session, apply } => {
            let key = SessionKey { agent: parse_agent(&agent), session_id: session };
            println!("{}", serde_json::to_string_pretty(&store().maintain(&key, None, apply)?)?);
            Ok(())
        }
        Command::Retain { agent, session, before, apply } => {
            let key = SessionKey { agent: parse_agent(&agent), session_id: session };
            println!("{}", serde_json::to_string_pretty(&store().maintain(&key, Some(before), apply)?)?);
            Ok(())
        }
        Command::Record { agent } => {
            let input: serde_json::Value = match serde_json::from_reader(std::io::stdin()) {
                Ok(input) => input,
                Err(_) => return Ok(()),
            };
            crate::hooks::register(
                &paths::state_dir(),
                parse_agent(&agent),
                &input,
                registry::now_unix(),
                crate::hooks::terminal(),
            )
        }
        Command::Events { agent, session } => {
            let key = SessionKey { agent: parse_agent(&agent), session_id: session };
            println!("{}", serde_json::to_string_pretty(&read_events(&store(), &key)?)?);
            Ok(())
        }
        Command::Files { agent, session } => {
            let key = SessionKey { agent: parse_agent(&agent), session_id: session };
            println!("{}", serde_json::to_string_pretty(&session_activity::files(read_events(&store(), &key)?))?);
            Ok(())
        }
    }
}

fn parse_agent(agent: &str) -> Agent {
    match agent {
        "codex" => Agent::Codex,
        _ => Agent::Claude,
    }
}

pub(crate) fn read_events(store: &Store, key: &SessionKey) -> Result<Vec<FileEvent>> {
    let report = store.read(key)?;
    for warning in report.warnings {
        eprintln!("incomplete activity history: skipped {}: {}", warning.path.display(), warning.message);
    }
    Ok(report.events)
}
