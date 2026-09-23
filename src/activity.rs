use std::fs;
use std::path::Path;

use anyhow::{Result, bail};
use clap::Subcommand;
use session_activity::{Agent, FileEvent, Operation, ScanRoot, SessionKey, Source, Store};

use crate::{paths, registry};
use registry::Session;

#[derive(Subcommand)]
pub enum Command {
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
        /// Include filesystem candidates for a currently registered session
        #[arg(long)]
        candidates: bool,
    },
}

pub fn store() -> Store {
    Store::new(paths::state_dir().join("activity"))
}

pub fn run(command: Command) -> Result<()> {
    match command {
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
        Command::Files { agent, session, candidates } => {
            let key = SessionKey { agent: parse_agent(&agent), session_id: session };
            let active = registry::find(&key.session_id).filter(|s| s.agent == key.agent);
            let events = match active {
                Some(session) => observations(&session, &store(), candidates)?,
                None if candidates => bail!("filesystem candidates require a registered session"),
                None => read_events(&store(), &key)?,
            };
            println!("{}", serde_json::to_string_pretty(&session_activity::files(events))?);
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

/// Host-specific discovery roots and legacy compatibility. The library owns
/// normalization, storage, scanning, and aggregation; Peekback chooses roots.
pub(crate) fn observations(session: &Session, store: &Store, candidates: bool) -> Result<Vec<FileEvent>> {
    let report = observations_report(session, store, candidates)?;
    for warning in report.warnings {
        eprintln!("{warning}");
    }
    Ok(report.events)
}

pub(crate) struct Observations {
    pub events: Vec<FileEvent>,
    pub warnings: Vec<String>,
}

/// Keep diagnostics separate so terminal consumers can render them without
/// stderr output corrupting their screen.
pub(crate) fn observations_report(session: &Session, store: &Store, candidates: bool) -> Result<Observations> {
    let key = session.activity_key();
    let report = store.read(&key)?;
    let mut events = report.events;
    let mut warnings: Vec<_> = report
        .warnings
        .into_iter()
        .map(|w| format!("incomplete activity history: skipped {}: {}", w.path.display(), w.message))
        .collect();
    if let Some(transcript) = &session.transcript_path {
        events.extend(session_activity::transcript_events(&key, &session.cwd, transcript));
    }
    for path in &session.written_files {
        let at = fs::metadata(path).and_then(|m| m.modified()).map(registry::unix_secs).unwrap_or(0);
        events.push(FileEvent::new(&key, &session.cwd, path, at, Operation::Write, Source::LegacyRegistry));
    }
    if candidates {
        let mut roots = Vec::new();
        if let Some(path) = session.scratchpad_dir() {
            roots.push(ScanRoot { path, since: 0, depth: usize::MAX, source: Source::ScratchpadScan });
        }
        if let Some(path) = session.memory_dir() {
            roots.push(ScanRoot { path, since: 0, depth: 1, source: Source::MemoryScan });
        }
        if session.cwd != Path::new("/") && dirs::home_dir().is_none_or(|h| h != session.cwd) {
            roots.push(ScanRoot {
                path: session.cwd.clone(),
                since: session.started_at(),
                depth: 4,
                source: Source::ProjectScan,
            });
        }
        let report = session_activity::scan_report(&key, &session.cwd, &roots, 20_000);
        if report.budget_exhausted {
            warnings
                .push(format!("incomplete file scan: entry budget exhausted after {} entries", report.entries_visited));
        }
        if report.depth_limited {
            warnings.push("file scan limited: directories below the configured depth were skipped".into());
        }
        for warning in report.warnings {
            warnings.push(format!("incomplete file scan: {}: {}", warning.path.display(), warning.message));
        }
        events.extend(report.events);
    }
    let cutoff = store.retained_from(&key)?;
    Ok(Observations {
        events: session_activity::reconcile(events.into_iter().filter(|e| cutoff.is_none_or(|at| e.timestamp >= at))),
        warnings,
    })
}
