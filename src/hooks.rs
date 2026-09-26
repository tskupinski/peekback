use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Result;
use serde_json::Value;
use session_activity::Store;

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

pub(crate) fn register(root: &Path, harness: Harness, input: &Value, now: i64, terminal: Terminal) -> Result<()> {
    let Some(id) = input["session_id"].as_str().filter(|id| session_activity::valid_id(id)) else { return Ok(()) };
    let Some(event) = input["hook_event_name"].as_str() else { return Ok(()) };
    let supported = matches!(event, "SessionStart" | "UserPromptSubmit" | "PostToolUse" | "Stop" | "SessionEnd")
        || (harness == Harness::Claude && event == "PostToolUseFailure");
    if !supported {
        return Ok(());
    }
    let _lock = crate::lifecycle::lock(root, id)?;
    let key = harness.key(id);
    let dir = root.join("sessions");
    let file = dir.join(format!("{id}.json"));
    let previous: Option<Session> = fs::read(&file).ok().and_then(|b| serde_json::from_slice(&b).ok());
    anyhow::ensure!(
        previous.as_ref().is_none_or(|s| s.harness == harness),
        "session ID is already registered to a different agent"
    );
    let store = Store::new(root.join("activity"));
    let open_turn = previous.as_ref().and_then(|s| s.turn_started_at);
    // Compaction can happen inside a turn; any other start means the process
    // that owned the open turn is gone.
    let compacting = event == "SessionStart" && input["source"] == "compact";
    let closed = match (event, open_turn, previous.as_ref()) {
        ("Stop", Some(started_at), _) => Some(Turn { started_at, ended_at: now.max(started_at) }),
        // No Stop came: interrupted with Esc, quit mid-turn, or the agent died.
        ("UserPromptSubmit" | "SessionStart" | "SessionEnd", Some(started_at), Some(previous)) if !compacting => {
            Some(capture::abandoned(previous, started_at))
        }
        _ => None,
    };
    let closing = closed.is_some() || event == "SessionEnd" || event == "Stop";
    let capture_context = previous.as_ref().filter(|_| closing).map(|s| {
        let mut context = s.clone();
        if event == "Stop" && context.transcript_path.is_none() {
            context.transcript_path = input["transcript_path"].as_str().filter(|p| !p.is_empty()).map(Into::into);
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
    let retained = if closed.is_some() || event == "SessionEnd" {
        previous.as_ref().map_or(Ok(()), |s| crate::turn_history::record(root, s, closed))
    } else {
        Ok(())
    };
    if event == "SessionEnd" {
        // Closing the session must survive an interrupted capture.
        crate::lifecycle::mark_ended(root, &key)?;
        let captured = crate::capture_jobs::retry(root, &key, Retry::Automatic);
        crate::lifecycle::end(root, &key)?;
        without_job();
        return queued.and(retained).and(captured);
    }
    let Some(cwd) = input["cwd"].as_str().map(Path::new).filter(|p| p.is_absolute()) else {
        without_job();
        return queued;
    };
    if !input["transcript_path"].is_null() && !input["transcript_path"].is_string() {
        without_job();
        return queued;
    }
    let mut session = Session {
        session_id: id.into(),
        incarnation: if event == "SessionStart" && !compacting || previous.is_none() {
            format!(
                "{}-{}",
                std::process::id(),
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos()
            )
        } else {
            previous.as_ref().map(|s| s.incarnation.clone()).unwrap_or_default()
        },
        harness,
        cwd: cwd.to_owned(),
        terminal,
        transcript_path: input["transcript_path"]
            .as_str()
            .filter(|p| !p.is_empty())
            .map(Into::into)
            .or_else(|| previous.as_ref().and_then(|s| s.transcript_path.clone())),
        started_at: previous.as_ref().map_or(now, |s| s.started_at),
        last_active_at: previous.as_ref().map_or(now, |s| now.max(s.last_active_at)),
        written_files: previous.as_ref().map_or_else(Vec::new, |s| s.written_files.clone()),
        turn_started_at: match event {
            "UserPromptSubmit" => Some(now),
            "Stop" => None,
            "SessionStart" if !compacting => None,
            _ => open_turn,
        },
        recent_turns: previous.as_ref().map_or_else(Vec::new, |s| s.recent_turns.clone()),
    };
    if let Some(turn) = closed {
        capture::remember(&mut session.recent_turns, turn);
    }
    store.append(&session.activity_key(), &session_activity::hook_events(harness.into(), input, now)?)?;
    // Late tools may contribute useful history, but only SessionStart can
    // explicitly reopen a session after an end marker has been published.
    if event != "SessionStart" && crate::lifecycle::ended(root, &key) {
        without_job();
        return queued;
    }
    // A failed capture must still record the new turn state, so its error is
    // reported after the registry write.
    if event == "Stop" && previous.is_none() {
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
        if event == "SessionStart" {
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
    if matches!(event, "PostToolUse" | "PostToolUseFailure") {
        return queued.and(retained);
    }
    let captured = crate::capture_jobs::retry(root, &key, Retry::Automatic);
    queued.and(retained).and(captured)
}
