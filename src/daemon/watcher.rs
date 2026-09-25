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
    watched_dir: Option<PathBuf>,
    watched_file: Arc<Mutex<Option<PathBuf>>>,
}

impl DocWatcher {
    pub fn new(proxy: EventLoopProxy<UserEvent>) -> Result<Self> {
        let watched_file = Arc::new(Mutex::new(None::<PathBuf>));
        let filter = watched_file.clone();
        let inner = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            let Ok(event) = event else { return };
            if !matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)) {
                return;
            }
            // FSEvents reports real paths while the watched path may go
            // through a symlink (`/tmp` is one); the watch is on the parent
            // directory only, so the file name is enough to match.
            let target = filter.lock().unwrap().clone();
            let hit = target.is_some_and(|t| event.paths.iter().any(|p| p.file_name() == t.file_name()));
            if hit {
                let _ = proxy.send_event(UserEvent::DocChanged);
            }
        })?;
        Ok(Self { inner, watched_dir: None, watched_file })
    }

    pub fn watch(&mut self, file: &Path) -> Result<()> {
        let dir = file.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("/"));
        *self.watched_file.lock().unwrap() = Some(file.to_path_buf());
        if self.watched_dir.as_ref() == Some(&dir) {
            return Ok(());
        }
        if let Some(old) = self.watched_dir.take() {
            let _ = self.inner.unwatch(&old);
        }
        self.inner.watch(&dir, RecursiveMode::NonRecursive)?;
        self.watched_dir = Some(dir);
        Ok(())
    }

    pub fn clear(&mut self) {
        *self.watched_file.lock().unwrap() = None;
        if let Some(old) = self.watched_dir.take() {
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
