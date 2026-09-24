use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Result;
use serde_json::Value;
use session_activity::{Agent, Store};

use crate::registry::{Session, Terminal};

static NEXT_WRITE: AtomicU64 = AtomicU64::new(0);

pub fn terminal() -> Terminal {
    let get = |key| std::env::var(key).ok().filter(|v| !v.is_empty());
    Terminal {
        bundle_id: get("__CFBundleIdentifier"),
        term_program: get("TERM_PROGRAM"),
        iterm_profile: get("ITERM_PROFILE"),
        tmux_pane: get("TMUX_PANE"),
        tmux_socket: get("TMUX").map(|v| v.split(',').next().unwrap_or_default().to_owned()),
        kitty_window: get("KITTY_WINDOW_ID"),
        kitty_listen_on: get("KITTY_LISTEN_ON"),
        wezterm_pane: get("WEZTERM_PANE"),
        agent: crate::process::hook_agent(),
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
    if event == "SessionEnd" {
        // Closing the session must survive an interrupted transcript import.
        crate::lifecycle::mark_ended(root, &key)?;
        if let Some(session) = &previous {
            // Preserve known history before the live registry entry disappears.
            let events = crate::activity::observations(session, &store, false)?;
            let retained = crate::activity::read_events(&store, &session.activity_key())?;
            let imported: Vec<_> = events.into_iter().filter(|e| !retained.contains(e)).collect();
            store.append(&session.activity_key(), &imported)?;
        }
        crate::lifecycle::end(root, &key)?;
        return Ok(());
    }
    let Some(cwd) = input["cwd"].as_str().map(Path::new).filter(|p| p.is_absolute()) else { return Ok(()) };
    if !input["transcript_path"].is_null() && !input["transcript_path"].is_string() {
        return Ok(());
    }
    let session = Session {
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
        written_files: previous.map_or_else(Vec::new, |s| s.written_files),
    };
    store.append(&session.activity_key(), &session_activity::hook_events(agent, input, now)?)?;
    // Late tools may contribute useful history, but only SessionStart can
    // explicitly reopen a session after an end marker has been published.
    if event != "SessionStart" && crate::lifecycle::ended(root, &key) {
        return Ok(());
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
    result
}
