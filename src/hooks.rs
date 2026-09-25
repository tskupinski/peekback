use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Result;
use serde_json::Value;
use session_activity::{Agent, Store};

use crate::capture::{self, Turn};
use crate::registry::{Session, Terminal};

static NEXT_WRITE: AtomicU64 = AtomicU64::new(0);

pub fn terminal() -> Terminal {
    let get = |key: &str| std::env::var(key).ok().filter(|v| !v.is_empty());
    let agent = crate::process::hook_agent();
    Terminal {
        bundle_id: get("__CFBundleIdentifier"),
        term_program: get("TERM_PROGRAM"),
        iterm_profile: get("ITERM_PROFILE"),
        panes: crate::mux::capture(get, agent.and_then(crate::process::ProcessId::tty)),
        agent,
    }
}

pub(crate) fn register(root: &Path, agent: Agent, input: &Value, now: i64, terminal: Terminal) -> Result<()> {
    let Some(id) = input["session_id"].as_str().filter(|id| session_activity::valid_id(id)) else { return Ok(()) };
    let Some(event) = input["hook_event_name"].as_str() else { return Ok(()) };
    let supported = matches!(event, "SessionStart" | "UserPromptSubmit" | "PostToolUse" | "Stop" | "SessionEnd")
        || (agent == Agent::Claude && event == "PostToolUseFailure");
    if !supported {
        return Ok(());
    }
    let _lock = crate::lifecycle::lock(root, id)?;
    let key = session_activity::SessionKey { agent, session_id: id.into() };
    let dir = root.join("sessions");
    let file = dir.join(format!("{id}.json"));
    let previous: Option<Session> = fs::read(&file).ok().and_then(|b| serde_json::from_slice(&b).ok());
    anyhow::ensure!(
        previous.as_ref().is_none_or(|s| s.agent == agent),
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
    if event == "SessionEnd" {
        // Closing the session must survive an interrupted capture.
        crate::lifecycle::mark_ended(root, &key)?;
        let captured = previous.as_ref().map_or(Ok(()), |s| capture::capture(root, s, &store, closed));
        crate::lifecycle::end(root, &key)?;
        return captured;
    }
    let Some(cwd) = input["cwd"].as_str().map(Path::new).filter(|p| p.is_absolute()) else { return Ok(()) };
    if !input["transcript_path"].is_null() && !input["transcript_path"].is_string() {
        return Ok(());
    }
    let mut session = Session {
        session_id: id.into(),
        agent,
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
        recent_turns: previous.map_or_else(Vec::new, |s| s.recent_turns),
    };
    if let Some(turn) = closed {
        capture::remember(&mut session.recent_turns, turn);
    }
    store.append(&session.activity_key(), &session_activity::hook_events(agent, input, now)?)?;
    // Late tools may contribute useful history, but only SessionStart can
    // explicitly reopen a session after an end marker has been published.
    if event != "SessionStart" && crate::lifecycle::ended(root, &key) {
        return Ok(());
    }
    // A failed capture must still record the new turn state, so its error is
    // reported after the registry write.
    let captured =
        if event == "Stop" || closed.is_some() { capture::capture(root, &session, &store, closed) } else { Ok(()) };
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
    result.and(captured)
}
