//! Turn capture: decide once, when a turn closes, which files a session
//! touched, and store it. Views only read what was stored.
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
/// An open turn whose agent has been quiet this long was most likely
/// interrupted and left idle; a shorter silence can be one long command.
const QUIET_SECS: i64 = 10 * 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Turn {
    pub started_at: i64,
    pub ended_at: i64,
}

impl Turn {
    pub(crate) fn covers(self, at: i64) -> bool {
        self.started_at - SLACK_SECS <= at && at <= self.ended_at.saturating_add(SLACK_SECS)
    }
}

/// A turn nobody closed with Stop: interrupted with Esc, or its agent died. It
/// ends at the agent's last recorded activity, not when the next hook came,
/// which may be hours later and would cover other sessions' changes.
pub fn abandoned(session: &Session, started_at: i64) -> Turn {
    Turn { started_at, ended_at: session.last_agent_activity().max(started_at) }
}

/// A session's recent turns and its open one, as others see them. The open
/// turn only reaches the present while the agent looks busy.
fn turns<'a>(
    session: &'a Session,
    recorded: Option<&'a crate::turn_history::History>,
    now: i64,
) -> impl Iterator<Item = Turn> + 'a {
    let open = session.turn_started_at.map(|started_at| {
        let turn = abandoned(session, started_at);
        if session.agent_alive() && now - turn.ended_at < QUIET_SECS {
            Turn { ended_at: i64::MAX, ..turn }
        } else {
            turn
        }
    });
    session
        .recent_turns
        .iter()
        .copied()
        .filter(move |turn| !recorded.is_some_and(|history| history.turns.iter().any(|record| record.turn == *turn)))
        .chain(open)
}

/// Bound the registry cache; durable history retains older overlap evidence.
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
        let at = registry::modified_at(path).unwrap_or(0);
        events.push(FileEvent::new(&key, &session.cwd, path, at, Operation::Write, Source::LegacyRegistry));
    }
    let report = session_activity::scan_report(&key, &session.cwd, &scan_roots(session, turn), SCAN_BUDGET);
    if report.budget_exhausted {
        eprintln!("peekback: incomplete capture, scan budget exhausted after {} entries", report.entries_visited);
    }
    for warning in report.warnings {
        eprintln!("peekback: incomplete capture: {}: {}", warning.path.display(), warning.message);
    }
    let others: Vec<_> = registry::registered_in(root).into_iter().filter(|s| s.activity_key() != key).collect();
    // Read live entries first: a concurrently ending session publishes its
    // history before removing the entry, so it appears in at least one read.
    let history = crate::turn_history::load(root).unwrap_or_else(|error| {
        eprintln!("peekback: incomplete overlap history: {error:#}");
        Vec::new()
    });
    let now = registry::now_unix();
    for mut event in report.events {
        if event.source != Source::ScratchpadScan {
            event.concurrent = concurrent(&event, &others, &history, now);
            for other in &history {
                if other.session != key
                    && other.turns.iter().any(|record| {
                        record.turn.covers(event.timestamp)
                            && record.roots.iter().any(|dir| event.path.starts_with(dir))
                    })
                    && !event.concurrent.contains(&other.session)
                {
                    event.concurrent.push(other.session.clone());
                }
            }
            event
                .concurrent
                .sort_by(|a, b| a.agent.slug().cmp(b.agent.slug()).then_with(|| a.session_id.cmp(&b.session_id)));
        }
        events.push(event);
    }
    store.append_new(&key, &events)?;
    Ok(())
}

fn transcripts(session: &Session) -> Vec<FileEvent> {
    let Some(transcript) = &session.transcript_path else { return Vec::new() };
    session.harness.transcript_events(&session.activity_key(), &session.cwd, transcript)
}

fn scan_roots(session: &Session, turn: Option<Turn>) -> Vec<ScanRoot> {
    let mut roots = Vec::new();
    // Only this session writes its scratchpad, so no window is needed there.
    if let Some(path) = session.scratchpad_dir() {
        roots.push(ScanRoot { path, since: 0, until: i64::MAX, depth: usize::MAX, source: Source::ScratchpadScan });
    }
    let Some(turn) = turn else { return roots };
    let (since, until) = (turn.started_at - SLACK_SECS, turn.ended_at.saturating_add(SLACK_SECS));
    if let Some(path) = session.memory_dir() {
        roots.push(ScanRoot { path, since, until, depth: 1, source: Source::MemoryScan });
    }
    if session.cwd != Path::new("/") && dirs::home_dir().is_none_or(|home| home != session.cwd) {
        roots.push(ScanRoot { path: session.cwd.clone(), since, until, depth: 4, source: Source::ProjectScan });
    }
    roots
}

/// Sessions whose roots contain the file and that were working when it last
/// changed; the filesystem cannot tell which of them changed it.
fn concurrent(
    event: &FileEvent,
    others: &[Session],
    history: &[crate::turn_history::History],
    now: i64,
) -> Vec<SessionKey> {
    others
        .iter()
        .filter(|other| {
            event.path.starts_with(&other.cwd) || other.memory_dir().is_some_and(|dir| event.path.starts_with(dir))
        })
        .filter(|other| {
            let recorded = history.iter().find(|history| history.session == other.activity_key());
            turns(other, recorded, now).any(|turn| turn.covers(event.timestamp))
        })
        .map(Session::activity_key)
        .collect()
}
