use std::path::{Path, PathBuf};

use anyhow::Result;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tao::event_loop::EventLoopProxy;

use crate::daemon::UserEvent;

/// Watches the current document's parent directory, since editors commonly
/// replace files by rename and a watch on the file itself would go stale.
pub struct DocWatcher {
    inner: RecommendedWatcher,
    watched_dir: Option<PathBuf>,
    watched_file: std::sync::Arc<std::sync::Mutex<Option<PathBuf>>>,
}

impl DocWatcher {
    pub fn new(proxy: EventLoopProxy<UserEvent>) -> Result<Self> {
        let watched_file = std::sync::Arc::new(std::sync::Mutex::new(None::<PathBuf>));
        let filter = watched_file.clone();
        let inner = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            let Ok(event) = event else { return };
            if !matches!(
                event.kind,
                EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
            ) {
                return;
            }
            let target = filter.lock().unwrap().clone();
            if target.is_some_and(|t| event.paths.contains(&t)) {
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
