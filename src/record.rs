//! Recording what a harness reports about a session: turn boundaries, file
//! events, and the live registry entry used for sending. Harnesses decode
//! their own payloads into an `Observation`; nothing here reads them.
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Result;
use serde_json::Value;
use session_activity::{FileEvent, Store};

use crate::capture::{self, Turn};
use crate::capture_jobs::Retry;
use crate::harness::Harness;
use crate::registry::{Session, Terminal};

static NEXT_WRITE: AtomicU64 = AtomicU64::new(0);

pub fn terminal() -> Terminal {
    let get = |key: &str| std::env::var(key).ok().filter(|v| !v.is_empty());
    let agent = crate::process::hook_agent();
    Terminal {
        bundle_id: get("__CFBundleIdentifier"),
        term_program: get("TERM_PROGRAM"),
        iterm_profile: get("ITERM_PROFILE"),
        panes: crate::mux::capture(get),
        agent,
    }
}

/// One lifecycle event of a session, decoded from whatever its harness sends.
#[derive(Debug, PartialEq)]
pub struct Observation {
    pub harness: Harness,
    pub session_id: String,
    pub event: Event,
    /// `None` when the harness sent no usable place: the turn still closes,
    /// but the registry entry is not rewritten.
    pub place: Option<Place>,
}

#[derive(Debug, PartialEq)]
pub struct Place {
    pub cwd: PathBuf,
    pub transcript: Option<PathBuf>,
}

#[derive(Debug, PartialEq)]
pub enum Event {
    SessionStarted(Start),
    PromptSubmitted,
    ToolUsed(Vec<FileEvent>),
    TurnEnded,
    SessionEnded,
}

/// Compaction continues the open turn; any other start means the process
/// that owned it is gone.
#[derive(Debug, PartialEq)]
pub enum Start {
    New,
    Compaction,
}

/// One hook invocation: the harness's own payload, decoded, then recorded.
pub(crate) fn hook(
    root: &Path,
    harness: Harness,
    input: &Value,
    now: i64,
    terminal: impl FnOnce() -> Terminal,
) -> Result<()> {
    match harness.decode(input, now)? {
        Some(observation) => record(root, observation, now, terminal()),
        None => Ok(()),
    }
}

pub(crate) fn record(root: &Path, observation: Observation, now: i64, terminal: Terminal) -> Result<()> {
    let Observation { harness, session_id: id, event, place } = observation;
    let _lock = crate::lifecycle::lock(root, &id)?;
    let key = harness.key(id.as_str());
    let dir = root.join("sessions");
    let file = dir.join(format!("{id}.json"));
    let previous: Option<Session> = fs::read(&file).ok().and_then(|b| serde_json::from_slice(&b).ok());
    anyhow::ensure!(
        previous.as_ref().is_none_or(|s| s.harness == harness),
        "session ID is already registered to a different agent"
    );
    let store = Store::new(root.join("activity"));
    let open_turn = previous.as_ref().and_then(|s| s.turn_started_at);
    let closed = match (&event, open_turn, previous.as_ref()) {
        (Event::TurnEnded, Some(started_at), _) => Some(Turn { started_at, ended_at: now.max(started_at) }),
        // No turn end came: interrupted with Esc, quit mid-turn, or the agent died.
        (
            Event::PromptSubmitted | Event::SessionStarted(Start::New) | Event::SessionEnded,
            Some(started_at),
            Some(previous),
        ) => Some(capture::abandoned(previous, started_at)),
        _ => None,
    };
    let closing = closed.is_some() || matches!(event, Event::SessionEnded | Event::TurnEnded);
    let capture_context = previous.as_ref().filter(|_| closing).map(|s| {
        let mut context = s.clone();
        if event == Event::TurnEnded && context.transcript_path.is_none() {
            context.transcript_path = place.as_ref().and_then(|place| place.transcript.clone());
        }
        context
    });
    let queued = capture_context.as_ref().map_or(Ok(()), |context| crate::capture_jobs::enqueue(root, context, closed));
    // A job that cannot be saved must not keep the session open or its turn
    // unclosed. The lifecycle change is persisted first; then the capture runs
    // from the same context, since it can wait on the store's lock past the
    // hook's timeout. The save error is what the hook reports.
    let without_job = || {
        if let (Err(_), Some(context)) = (&queued, &capture_context) {
            if let Err(error) = capture::capture(root, context, &store, closed) {
                eprintln!("capture without a pending job: {error:#}");
            }
        }
    };
    let retained = if closed.is_some() || event == Event::SessionEnded {
        previous.as_ref().map_or(Ok(()), |s| crate::turn_history::record(root, s, closed))
    } else {
        Ok(())
    };
    if event == Event::SessionEnded {
        // Closing the session must survive an interrupted capture.
        crate::lifecycle::mark_ended(root, &key)?;
        let captured = crate::capture_jobs::retry(root, &key, Retry::Automatic);
        crate::lifecycle::end(root, &key)?;
        without_job();
        return queued.and(retained).and(captured);
    }
    let Some(place) = place else {
        without_job();
        return queued;
    };
    let mut session = Session {
        session_id: id.clone(),
        incarnation: if event == Event::SessionStarted(Start::New) || previous.is_none() {
            format!(
                "{}-{}",
                std::process::id(),
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos()
            )
        } else {
            previous.as_ref().map(|s| s.incarnation.clone()).unwrap_or_default()
        },
        harness,
        cwd: place.cwd,
        terminal,
        transcript_path: place.transcript.or_else(|| previous.as_ref().and_then(|s| s.transcript_path.clone())),
        started_at: previous.as_ref().map_or(now, |s| s.started_at),
        last_active_at: previous.as_ref().map_or(now, |s| now.max(s.last_active_at)),
        written_files: previous.as_ref().map_or_else(Vec::new, |s| s.written_files.clone()),
        turn_started_at: match event {
            Event::PromptSubmitted => Some(now),
            Event::TurnEnded | Event::SessionStarted(Start::New) => None,
            _ => open_turn,
        },
        recent_turns: previous.as_ref().map_or_else(Vec::new, |s| s.recent_turns.clone()),
    };
    if let Some(turn) = closed {
        capture::remember(&mut session.recent_turns, turn);
    }
    if let Event::ToolUsed(events) = &event {
        store.append(&session.activity_key(), events)?;
    }
    // Late tools may contribute useful history, but only a session start can
    // explicitly reopen a session after an end marker has been published.
    let starting = matches!(event, Event::SessionStarted(_));
    if !starting && crate::lifecycle::ended(root, &key) {
        without_job();
        return queued;
    }
    // A failed capture must still record the new turn state, so its error is
    // reported after the registry write.
    if event == Event::TurnEnded && previous.is_none() {
        crate::capture_jobs::enqueue(root, &session, None)?;
    }
    crate::lifecycle::ensure_dir(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
    }
    let tmp = dir.join(format!("{id}.{}.{}.tmp", std::process::id(), NEXT_WRITE.fetch_add(1, Ordering::Relaxed)));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| -> Result<()> {
        let mut output = options.open(&tmp)?;
        output.write_all(&serde_json::to_vec(&session)?)?;
        output.sync_all()?;
        fs::rename(&tmp, &file)?;
        #[cfg(unix)]
        fs::File::open(&dir)?.sync_all()?;
        if starting {
            crate::lifecycle::resume(root, &key)?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(tmp);
    }
    result?;
    without_job();
    // Tool hooks come many times a turn; boundaries are enough to retry at.
    if matches!(event, Event::ToolUsed(_)) {
        return queued.and(retained);
    }
    let captured = crate::capture_jobs::retry(root, &key, Retry::Automatic);
    queued.and(retained).and(captured)
}
