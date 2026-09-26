use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tao::event_loop::EventLoopProxy;

use crate::daemon::UserEvent;

/// Watches the current document's parent directory, since editors commonly
/// replace files by rename and a watch on the file itself would go stale.
pub struct DocWatcher {
    inner: RecommendedWatcher,
    watched_dirs: Vec<PathBuf>,
    watched_files: Arc<Mutex<Vec<PathBuf>>>,
}

impl DocWatcher {
    pub fn new(proxy: EventLoopProxy<UserEvent>) -> Result<Self> {
        let watched_files = Arc::new(Mutex::new(Vec::<PathBuf>::new()));
        let filter = watched_files.clone();
        let inner = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            let Ok(event) = event else { return };
            if !matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)) {
                return;
            }
            // FSEvents reports real paths while the watched path may go
            // through a symlink (`/tmp` is one); the watch is on the parent
            // directory only, so the file name is enough to match.
            let target = filter.lock().unwrap().clone();
            let hit = target.iter().any(|t| event.paths.iter().any(|p| p.file_name() == t.file_name()));
            if hit {
                let _ = proxy.send_event(UserEvent::DocChanged);
            }
        })?;
        Ok(Self { inner, watched_dirs: Vec::new(), watched_files })
    }

    pub fn watch(&mut self, file: &Path) -> Result<()> {
        let mut targets = vec![file.to_path_buf()];
        if let Ok(resolved) = file.canonicalize() {
            if resolved != file {
                targets.push(resolved);
            }
        }
        let mut dirs: Vec<_> = targets.iter().filter_map(|p| p.parent().map(Path::to_path_buf)).collect();
        dirs.sort();
        dirs.dedup();
        *self.watched_files.lock().unwrap() = targets;
        for old in &self.watched_dirs {
            if !dirs.contains(old) {
                let _ = self.inner.unwatch(old);
            }
        }
        let mut watched = Vec::new();
        let mut error = None;
        for dir in dirs {
            if self.watched_dirs.contains(&dir) {
                watched.push(dir);
            } else {
                match self.inner.watch(&dir, RecursiveMode::NonRecursive) {
                    Ok(()) => watched.push(dir),
                    Err(e) => error = Some(e),
                }
            }
        }
        self.watched_dirs = watched;
        error.map_or(Ok(()), |e| Err(e.into()))
    }

    pub fn clear(&mut self) {
        self.watched_files.lock().unwrap().clear();
        for old in self.watched_dirs.drain(..) {
            let _ = self.inner.unwatch(&old);
        }
    }
}

/// Watches the session registry directory so the sidebar follows sessions
/// starting and ending.
pub fn watch_registry(proxy: EventLoopProxy<UserEvent>) -> Result<RecommendedWatcher> {
    let dir = crate::registry::dir();
    std::fs::create_dir_all(&dir)?;
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if event.is_ok() {
            let _ = proxy.send_event(UserEvent::RegistryChanged);
        }
    })?;
    watcher.watch(&dir, RecursiveMode::NonRecursive)?;
    Ok(watcher)
}
