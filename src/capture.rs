//! Turn capture: decide once, when a turn closes, which files a session
//! touched, and store it. Views only read what was stored.
use std::fs;
use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use session_activity::{FileEvent, Operation, ScanRoot, SessionKey, Source, Store};

use crate::registry::{self, Session};

/// Modification times are whole seconds and hooks start a moment after the
/// agent acts, so turn bounds get this much slack on both sides.
const SLACK_SECS: i64 = 2;
const SCAN_BUDGET: usize = 20_000;
/// Turns are kept for telling concurrent sessions apart as long as a session
/// that is still working could overlap them.
const RECENT_TURNS_SECS: i64 = 24 * 60 * 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Turn {
    pub started_at: i64,
    pub ended_at: i64,
}

impl Turn {
    fn covers(self, at: i64) -> bool {
        self.started_at - SLACK_SECS <= at && at <= self.ended_at.saturating_add(SLACK_SECS)
    }
}

/// Turns ended long ago cannot overlap anything a live session still captures.
pub fn remember(turns: &mut Vec<Turn>, closed: Turn) {
    turns.push(closed);
    turns.retain(|turn| turn.ended_at >= closed.ended_at - RECENT_TURNS_SECS);
}

/// Store the evidence of a closed turn, or, without one, the evidence that
/// needs no time window: transcripts, legacy paths and the scratchpad.
pub fn capture(root: &Path, session: &Session, store: &Store, turn: Option<Turn>) -> Result<()> {
    let key = session.activity_key();
    let mut events = transcripts(session);
    for path in &session.written_files {
        let at = fs::metadata(path).and_then(|m| m.modified()).map(registry::unix_secs).unwrap_or(0);
        events.push(FileEvent::new(&key, &session.cwd, path, at, Operation::Write, Source::LegacyRegistry));
    }
    let report = session_activity::scan_report(&key, &session.cwd, &scan_roots(session, turn), SCAN_BUDGET);
    if report.budget_exhausted {
        eprintln!("peekback: incomplete capture, scan budget exhausted after {} entries", report.entries_visited);
    }
    for warning in report.warnings {
        eprintln!("peekback: incomplete capture: {}: {}", warning.path.display(), warning.message);
    }
    let others: Vec<_> = registry::load_all_in(root).into_iter().filter(|s| s.activity_key() != key).collect();
    for mut event in report.events {
        if event.source != Source::ScratchpadScan {
            event.concurrent = concurrent(&event, &others);
        }
        events.push(event);
    }
    store.append_new(&key, &events)?;
    Ok(())
}

fn transcripts(session: &Session) -> Vec<FileEvent> {
    let Some(main) = &session.transcript_path else { return Vec::new() };
    let key = session.activity_key();
    std::iter::once(main.clone())
        .chain(session_activity::subagent_transcripts(main))
        .flat_map(|transcript| session_activity::transcript_events(&key, &session.cwd, &transcript))
        .collect()
}

fn scan_roots(session: &Session, turn: Option<Turn>) -> Vec<ScanRoot> {
    let mut roots = Vec::new();
    // Only this session writes its scratchpad, so no window is needed there.
    if let Some(path) = session.scratchpad_dir() {
        roots.push(ScanRoot { path, since: 0, depth: usize::MAX, source: Source::ScratchpadScan });
    }
    let Some(turn) = turn else { return roots };
    let since = turn.started_at - SLACK_SECS;
    if let Some(path) = session.memory_dir() {
        roots.push(ScanRoot { path, since, depth: 1, source: Source::MemoryScan });
    }
    if session.cwd != Path::new("/") && dirs::home_dir().is_none_or(|home| home != session.cwd) {
        roots.push(ScanRoot { path: session.cwd.clone(), since, depth: 4, source: Source::ProjectScan });
    }
    roots
}

/// Sessions whose roots contain the file and that were working when it last
/// changed; the filesystem cannot tell which of them changed it.
fn concurrent(event: &FileEvent, others: &[Session]) -> Vec<SessionKey> {
    others
        .iter()
        .filter(|other| {
            event.path.starts_with(&other.cwd) || other.memory_dir().is_some_and(|dir| event.path.starts_with(dir))
        })
        .filter(|other| other.turns().any(|turn| turn.covers(event.timestamp)))
        .map(Session::activity_key)
        .collect()
}
