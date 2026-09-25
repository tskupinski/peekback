use std::path::{Path, PathBuf};

use serde::Serialize;
use session_activity::{FileActivity, Operation, Outcome, Store};

use crate::registry::Session;

#[derive(Clone, Debug, Serialize)]
pub struct Document {
    pub path: PathBuf,
    pub touched_at: i64,
    pub label: String,
    /// Only a turn scan saw it: no tool reported writing it.
    pub scanned: bool,
    /// Scanned while another session was working in the same place.
    pub shared: bool,
}

pub fn documents(session: &Session) -> Vec<Document> {
    documents_in(session, &crate::activity::store())
}

/// Stored history only: what the session's turns captured, never a scan now.
pub(crate) fn documents_in(session: &Session, store: &Store) -> Vec<Document> {
    let events = match crate::activity::read_events(store, &session.activity_key()) {
        Ok(events) => events,
        Err(error) => {
            eprintln!("cannot read session activity: {error:#}");
            return Vec::new();
        }
    };
    session_activity::files(
        events.into_iter().filter(|e| e.operation != Operation::Read && e.outcome != Outcome::Failed),
    )
    .into_iter()
    .filter(|file| file.exists && is_markdown(&file.path))
    .map(|file| Document {
        label: label_for(&file.path, session),
        scanned: scanned(&file),
        shared: scanned(&file) && file.events.iter().any(|e| !e.concurrent.is_empty()),
        path: file.path,
        touched_at: file.last_touched_at,
    })
    .collect()
}

fn scanned(file: &FileActivity) -> bool {
    file.events.iter().all(|e| e.operation == Operation::Observed)
}

pub(crate) fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown"))
}

fn label_for(path: &Path, session: &Session) -> String {
    if let Ok(relative) = path.strip_prefix(&session.cwd) {
        return relative.display().to_string();
    }
    for (dir, prefix) in [(session.scratchpad_dir(), "scratchpad"), (session.memory_dir(), "memory")] {
        if let Some(relative) = dir.and_then(|d| path.strip_prefix(d).ok().map(Path::to_path_buf)) {
            return format!("{prefix}/{}", relative.display());
        }
    }
    if let Some(home) = dirs::home_dir() {
        if let Ok(relative) = path.strip_prefix(&home) {
            return format!("~/{}", relative.display());
        }
    }
    path.display().to_string()
}
