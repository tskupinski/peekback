use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use session_activity::{SessionKey, Store};

use crate::{capture, lifecycle, registry::Session, turn_history};

static NEXT: AtomicU64 = AtomicU64::new(0);

#[derive(Serialize, Deserialize)]
struct Job {
    session: Session,
    turn: Option<capture::Turn>,
}

// Callers hold the session lifecycle lock. Persist the original context before
// closing or replacing the live session; replay is safe through append_new.
pub fn enqueue(root: &Path, session: &Session, turn: Option<capture::Turn>) -> Result<()> {
    let key = session.activity_key();
    key.validate()?;
    let dir = root.join("pending-captures").join(key.agent.slug()).join(&key.session_id);
    lifecycle::ensure_dir(&dir)?;
    let stem = format!(
        "{}.{}.{}",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos(),
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    let temporary = dir.join(format!("{stem}.tmp"));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&temporary)?;
        file.write_all(&serde_json::to_vec(&Job { session: session.clone(), turn })?)?;
        file.sync_all()?;
        fs::rename(&temporary, dir.join(format!("{stem}.json")))?;
        fs::File::open(&dir)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

pub fn retry(root: &Path, key: &SessionKey) -> Result<()> {
    key.validate()?;
    let dir = root.join("pending-captures").join(key.agent.slug()).join(&key.session_id);
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    let store = Store::new(root.join("activity"));
    let mut first_error = None;
    for entry in entries {
        let result = (|| -> Result<()> {
            let entry = entry?;
            if entry.path().extension().is_none_or(|ext| ext != "json") {
                return Ok(());
            }
            anyhow::ensure!(entry.file_type()?.is_file(), "capture job must be a regular file");
            let job: Job = serde_json::from_slice(&fs::read(entry.path())?).context("reading pending capture")?;
            anyhow::ensure!(job.session.activity_key() == *key, "capture belongs to another session");
            let history = turn_history::record(root, &job.session, job.turn);
            capture::capture(root, &job.session, &store, job.turn)?;
            history?;
            fs::remove_file(entry.path())?;
            fs::File::open(&dir)?.sync_all()?;
            Ok(())
        })();
        if let Err(error) = result {
            if first_error.is_none() {
                first_error = Some(error);
            }
        }
    }
    first_error.map_or(Ok(()), Err)
}
