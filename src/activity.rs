use anyhow::Result;
use clap::Subcommand;
use session_activity::{FileEvent, SessionKey, Store};

use crate::harness::Harness;
use crate::{paths, registry};

/// What the hooks run. Installed hook commands name these, so they keep
/// their names; they are hidden from help since nobody types them.
#[derive(Subcommand)]
pub enum Command {
    /// Record a Claude Code or Codex hook from stdin (silent on success)
    Record {
        #[arg(long)]
        agent: Harness,
    },
    /// Retry captures retained after a failed hook or session exit
    Retry {
        #[arg(long)]
        agent: Harness,
        #[arg(long)]
        session: String,
    },
}

/// For finding out why a file is or is not listed: the recorded history
/// behind the viewer, and its upkeep.
#[derive(Subcommand)]
pub enum Debug {
    /// Browse a session's observed files in the terminal; p opens Markdown in Peekback
    Browse(crate::browse::Args),
    /// Print a session's files and their evidence as JSON (all file types)
    Files {
        #[arg(long)]
        agent: Harness,
        #[arg(long)]
        session: String,
    },
    /// Print a session's retained file events as JSON, including ended sessions
    Events {
        #[arg(long)]
        agent: Harness,
        #[arg(long)]
        session: String,
    },
    /// Preview packing raw history into fewer files without dropping events
    Compact {
        #[arg(long)]
        agent: Harness,
        #[arg(long)]
        session: String,
        /// Commit the previewed maintenance operation
        #[arg(long)]
        apply: bool,
    },
    /// Preview removing events older than a Unix timestamp (seconds)
    Retain {
        #[arg(long)]
        agent: Harness,
        #[arg(long)]
        session: String,
        #[arg(long, value_parser = clap::value_parser!(i64).range(0..))]
        before: i64,
        /// Permanently remove older events and compact retained history
        #[arg(long)]
        apply: bool,
    },
}

pub fn store() -> Store {
    Store::new(paths::state_dir().join("activity"))
}

pub fn run(command: Command) -> Result<()> {
    match command {
        Command::Record { agent } => {
            let input: serde_json::Value = match serde_json::from_reader(std::io::stdin()) {
                Ok(input) => input,
                Err(_) => return Ok(()),
            };
            crate::record::hook(&paths::state_dir(), agent, &input, registry::now_unix(), crate::record::terminal)
        }
        Command::Retry { agent, session } => {
            let key = agent.key(session);
            let root = paths::state_dir();
            let _lock = crate::lifecycle::lock(&root, &key.session_id)?;
            crate::capture_jobs::retry(&root, &key, crate::capture_jobs::Retry::Explicit)
        }
    }
}

pub fn debug(command: Debug) -> Result<()> {
    match command {
        Debug::Browse(args) => crate::browse::run(args),
        Debug::Compact { agent, session, apply } => {
            let key = agent.key(session);
            println!("{}", serde_json::to_string_pretty(&store().maintain(&key, None, apply)?)?);
            Ok(())
        }
        Debug::Retain { agent, session, before, apply } => {
            let key = agent.key(session);
            println!("{}", serde_json::to_string_pretty(&store().maintain(&key, Some(before), apply)?)?);
            Ok(())
        }
        Debug::Events { agent, session } => {
            let key = agent.key(session);
            println!("{}", serde_json::to_string_pretty(&read_events(&store(), &key)?)?);
            Ok(())
        }
        Debug::Files { agent, session } => {
            let key = agent.key(session);
            println!("{}", serde_json::to_string_pretty(&session_activity::files(read_events(&store(), &key)?))?);
            Ok(())
        }
    }
}

pub(crate) fn read_events(store: &Store, key: &SessionKey) -> Result<Vec<FileEvent>> {
    let report = store.read(key)?;
    for warning in report.warnings {
        eprintln!("incomplete activity history: skipped {}: {}", warning.path.display(), warning.message);
    }
    Ok(report.events)
}
