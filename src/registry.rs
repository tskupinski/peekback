use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::harness::Harness;
use crate::paths;

pub use session_activity::SessionKey;

/// One live agent session, as written by `peekback activity record`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    pub session_id: String,
    #[serde(default)]
    pub incarnation: String,
    #[serde(rename = "agent", default = "Harness::before_harness_tracking")]
    pub harness: Harness,
    pub transcript_path: Option<PathBuf>,
    /// Paths from older registry versions; new observations live in the activity store.
    #[serde(default)]
    pub written_files: Vec<PathBuf>,
    pub cwd: PathBuf,
    pub started_at: i64,
    pub last_active_at: i64,
    #[serde(default)]
    pub terminal: Terminal,
    /// Start of the turn in progress. It closes at Stop, the next prompt, or
    /// the end of the session.
    #[serde(default)]
    pub turn_started_at: Option<i64>,
    #[serde(default)]
    pub recent_turns: Vec<crate::capture::Turn>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Terminal {
    pub bundle_id: Option<String>,
    pub term_program: Option<String>,
    pub iterm_profile: Option<String>,
    /// Environment addresses are candidates only. Sending verifies ownership.
    #[serde(default, deserialize_with = "crate::mux::deserialize_known")]
    pub panes: Vec<crate::mux::Pane>,
    /// The agent process running in this terminal, when the hook could see it.
    pub agent: Option<crate::process::ProcessId>,
}

impl Session {
    pub fn activity_key(&self) -> SessionKey {
        self.harness.key(self.session_id.clone())
    }

    pub fn memory_dir(&self) -> Option<PathBuf> {
        self.harness.memory_dir(self)
    }

    /// An agent killed without SessionEnd leaves its entry behind; the process
    /// identity tells. Entries from before process tracking count as alive.
    pub fn agent_alive(&self) -> bool {
        self.terminal.agent.is_none_or(|agent| agent.is_running())
    }

    /// When the agent last did something: its newest transcript record, else
    /// its last hook.
    pub fn last_agent_activity(&self) -> i64 {
        let transcript = self.transcript_path.as_deref().and_then(|path| self.harness.last_activity(path));
        transcript.map_or(self.last_active_at, |at| at.max(self.last_active_at))
    }

    pub fn scratchpad_dir(&self) -> Option<PathBuf> {
        self.harness.scratchpad_dir(self)
    }
}

pub fn dir() -> PathBuf {
    paths::state_dir().join("sessions")
}

/// Live sessions, most recently active first. Unreadable entries are skipped
/// rather than failing the whole listing.
pub fn load_all() -> Vec<Session> {
    load_all_in(&paths::state_dir())
}

pub fn load_all_in(root: &Path) -> Vec<Session> {
    registered_in(root).into_iter().filter(Session::agent_alive).collect()
}

/// Entries without an end marker, including ones whose agent has exited.
pub(crate) fn registered_in(root: &Path) -> Vec<Session> {
    let Ok(entries) = fs::read_dir(root.join("sessions")) else { return Vec::new() };
    let mut sessions: Vec<Session> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| fs::read_to_string(e.path()).ok())
        .filter_map(|text| serde_json::from_str(&text).ok())
        .filter(|s: &Session| !crate::lifecycle::ended(root, &s.activity_key()))
        .collect();
    sessions.sort_by(|a, b| b.last_active_at.cmp(&a.last_active_at));
    sessions
}

/// Ids come from hook input and page messages and become file names.
pub fn valid_id(id: &str) -> bool {
    session_activity::valid_id(id)
}

/// A live session: registered, not ended, and its agent still running.
pub fn find(session_id: &str) -> Option<Session> {
    registered(session_id).filter(Session::agent_alive)
}

fn registered(session_id: &str) -> Option<Session> {
    registered_at(&paths::state_dir(), session_id)
}

fn registered_at(root: &Path, session_id: &str) -> Option<Session> {
    if !valid_id(session_id) {
        return None;
    }
    let text = fs::read_to_string(root.join("sessions").join(format!("{session_id}.json"))).ok()?;
    let session: Session = serde_json::from_str(&text).ok()?;
    (!crate::lifecycle::ended(root, &session.activity_key())).then_some(session)
}

pub fn newest() -> Option<Session> {
    load_all().into_iter().next()
}

/// Removes entries whose agent has exited or that had no hook or transcript
/// activity for `max_idle_secs`. A session that died without SessionEnd
/// leaves one behind.
pub fn prune(max_idle_secs: i64) -> Vec<Session> {
    prune_in(&paths::state_dir(), max_idle_secs)
}

pub(crate) fn prune_in(root: &Path, max_idle_secs: i64) -> Vec<Session> {
    let now = now_unix();
    let mut removed = Vec::new();
    for session in registered_in(root) {
        let Ok(_lock) = crate::lifecycle::lock(root, &session.session_id) else { continue };
        // Re-read under the lock: a hook could have refreshed this session,
        // or resumed it in a new process, after the listing was taken.
        let Some(session) = registered_at(root, &session.session_id) else { continue };
        let last_write =
            session.transcript_path.as_deref().and_then(modified_at).unwrap_or(0).max(session.last_active_at);
        let gone = now - last_write > max_idle_secs || !session.agent_alive();
        if !gone || !valid_id(&session.session_id) {
            continue;
        }
        // A session that died mid-turn gets that turn captured, as SessionEnd
        // would have done.
        let turn = session.turn_started_at.map(|started_at| crate::capture::abandoned(&session, started_at));
        // Without a job, the session is still pruned and captured directly
        // afterwards, as a hook does.
        let queued = crate::capture_jobs::enqueue(root, &session, turn);
        if let Err(error) = &queued {
            eprintln!("retain capture before pruning {}: {error:#}", session.session_id);
        }
        if let Err(error) = crate::turn_history::record(root, &session, turn) {
            eprintln!("retain turns before pruning {}: {error:#}", session.session_id);
        }
        if let Err(error) =
            crate::capture_jobs::retry(root, &session.activity_key(), crate::capture_jobs::Retry::Automatic)
        {
            eprintln!("capture before pruning {}: {error:#}", session.session_id);
        }
        let ended = crate::lifecycle::end(root, &session.activity_key()).is_ok();
        if queued.is_err() {
            let store = session_activity::Store::new(root.join("activity"));
            if let Err(error) = crate::capture::capture(root, &session, &store, turn) {
                eprintln!("capture after pruning {}: {error:#}", session.session_id);
            }
        }
        if ended {
            removed.push(session);
        }
    }
    removed
}

pub fn now_unix() -> i64 {
    unix_secs(SystemTime::now())
}

pub fn modified_at(path: &Path) -> Option<i64> {
    fs::metadata(path).and_then(|m| m.modified()).ok().map(unix_secs)
}

pub fn unix_secs(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}
