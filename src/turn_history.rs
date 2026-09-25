//! Completed turn evidence outlives the registry entries used for sending.
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use session_activity::SessionKey;

use crate::capture::Turn;
use crate::registry::Session;

static NEXT_WRITE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedTurn {
    pub turn: Turn,
    pub roots: Vec<PathBuf>,
}

#[derive(Serialize, Deserialize)]
pub struct History {
    pub session: SessionKey,
    pub turns: Vec<RecordedTurn>,
}

/// The caller holds this session's lifecycle lock. Publish before removing
/// its live record, so another capture never sees a gap in overlap evidence.
pub fn record(root: &Path, session: &Session, closed: Option<Turn>) -> Result<()> {
    let key = session.activity_key();
    key.validate()?;
    let dir = root.join("turns");
    let path = dir.join(format!("{}.{}.json", key.agent.slug(), key.session_id));
    let mut history = match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice::<History>(&bytes).context("reading turn history")?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => History { session: key.clone(), turns: Vec::new() },
        Err(e) => return Err(e.into()),
    };
    anyhow::ensure!(history.session == key, "turn history belongs to another session");
    let mut roots = vec![session.cwd.clone()];
    roots.extend(session.memory_dir());
    let mut changed = false;
    for turn in session.recent_turns.iter().copied() {
        let record = RecordedTurn { turn, roots: roots.clone() };
        // Legacy recent turns lack roots; never reinterpret one already saved.
        if !history.turns.iter().any(|old| old.turn == turn) {
            history.turns.push(record);
            changed = true;
        }
    }
    if let Some(turn) = closed {
        let record = RecordedTurn { turn, roots };
        if !history.turns.contains(&record) {
            history.turns.push(record);
            changed = true;
        }
    }
    if !changed {
        return Ok(());
    }
    crate::lifecycle::ensure_dir(&dir)?;
    let temporary = dir.join(format!("{}.{}.tmp", std::process::id(), NEXT_WRITE.fetch_add(1, Ordering::Relaxed)));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&temporary)?;
        file.write_all(&serde_json::to_vec(&history)?)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        fs::File::open(&dir)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

pub fn load(root: &Path) -> Result<Vec<History>> {
    let entries = match fs::read_dir(root.join("turns")) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut histories = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                eprintln!("peekback: incomplete overlap history: {error}");
                continue;
            }
        };
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let read = (|| -> Result<History> {
            anyhow::ensure!(entry.file_type()?.is_file(), "turn history must be a regular file");
            let history: History = serde_json::from_slice(&fs::read(&path)?)?;
            history.session.validate()?;
            anyhow::ensure!(
                history.turns.iter().all(|record| record.turn.ended_at >= record.turn.started_at
                    && record.roots.iter().all(|root| root.is_absolute())),
                "invalid turn history"
            );
            Ok(history)
        })();
        match read {
            Ok(history) => histories.push(history),
            Err(error) => eprintln!("peekback: incomplete overlap history: skipped {}: {error:#}", path.display()),
        }
    }
    Ok(histories)
}
