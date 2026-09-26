use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, ensure};

use crate::{AgentId, FileEvent, SessionKey};

pub(crate) const MAX_BATCH_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Default)]
pub struct ReadReport {
    pub events: Vec<FileEvent>,
    pub warnings: Vec<BatchWarning>,
}

#[derive(Debug, Default)]
pub struct SessionReport {
    pub sessions: Vec<SessionKey>,
    pub warnings: Vec<BatchWarning>,
}

#[derive(Debug)]
pub struct BatchWarning {
    pub path: PathBuf,
    pub message: String,
}

static NEXT_BATCH: AtomicU64 = AtomicU64::new(0);

/// An append-only store of metadata-only batches, namespaced by agent/session.
/// Each hook writes its own batch then renames it atomically. Concurrent hooks
/// cannot overwrite each other, and readers never see a partially written batch.
#[derive(Clone, Debug)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub(crate) fn directory(&self, session: &SessionKey) -> Result<PathBuf> {
        session.validate()?;
        Ok(self.root.join(session.agent.as_str()).join(&session.session_id))
    }

    /// Enumerate retained session directories, including ended/compacted sessions.
    /// Does not create state or follow namespace/session directory symlinks.
    pub fn sessions(&self) -> SessionReport {
        let mut report = SessionReport::default();
        let namespaces = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return report,
            Err(e) => {
                report.warnings.push(BatchWarning { path: self.root.clone(), message: e.to_string() });
                return report;
            }
        };
        for entry in namespaces {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    report.warnings.push(BatchWarning { path: self.root.clone(), message: error.to_string() });
                    continue;
                }
            };
            let namespace = (|| -> Result<Option<AgentId>> {
                let kind = entry.file_type()?;
                ensure!(!kind.is_symlink(), "agent namespace must be a directory, not a symlink");
                if !kind.is_dir() {
                    return Ok(None);
                }
                let name = entry.file_name().into_string().map_err(|_| anyhow::anyhow!("invalid agent namespace"))?;
                Ok(Some(AgentId::new(name)?))
            })();
            match namespace {
                Ok(Some(agent)) => self.sessions_in(agent, &entry.path(), &mut report),
                Ok(None) => {}
                Err(error) => report.warnings.push(BatchWarning { path: entry.path(), message: format!("{error:#}") }),
            }
        }
        report.sessions.sort_by(|a, b| a.agent.cmp(&b.agent).then_with(|| a.session_id.cmp(&b.session_id)));
        report
    }

    fn sessions_in(&self, agent: AgentId, path: &Path, report: &mut SessionReport) {
        let entries = match fs::read_dir(path) {
            Ok(entries) => entries,
            Err(error) => {
                report.warnings.push(BatchWarning { path: path.to_path_buf(), message: error.to_string() });
                return;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    report.warnings.push(BatchWarning { path: path.to_path_buf(), message: error.to_string() });
                    continue;
                }
            };
            let result = (|| -> Result<Option<SessionKey>> {
                let kind = entry.file_type()?;
                ensure!(!kind.is_symlink(), "session directory symlinks are not followed");
                if !kind.is_dir() {
                    return Ok(None);
                }
                let session_id =
                    entry.file_name().into_string().map_err(|_| anyhow::anyhow!("invalid session directory name"))?;
                let key = SessionKey { agent: agent.clone(), session_id };
                key.validate()?;
                Ok(Some(key))
            })();
            match result {
                Ok(Some(key)) => report.sessions.push(key),
                Ok(None) => {}
                Err(error) => report.warnings.push(BatchWarning { path: entry.path(), message: format!("{error:#}") }),
            }
        }
    }

    /// Read retained history across agents/sessions. Per-session failures become
    /// warnings so one damaged checkpoint cannot hide other sessions. This is
    /// not a globally atomic snapshot; each session is read under its own lock.
    pub fn read_all(&self) -> ReadReport {
        let sessions = self.sessions();
        let mut report = ReadReport { events: Vec::new(), warnings: sessions.warnings };
        for key in sessions.sessions {
            match self.read(&key) {
                Ok(mut read) => {
                    report.events.append(&mut read.events);
                    report.warnings.append(&mut read.warnings);
                }
                Err(error) => report.warnings.push(BatchWarning {
                    path: self.root.join(key.agent.as_str()).join(&key.session_id),
                    message: format!("{error:#}"),
                }),
            }
        }
        report.events.sort_by_key(|e| e.timestamp);
        report
    }

    pub fn append(&self, session: &SessionKey, events: &[FileEvent]) -> Result<()> {
        let Some(dir) = self.prepare(session, events)? else { return Ok(()) };
        let _lock = crate::lock::Lock::acquire(&dir, false)?;
        self.append_unlocked(&dir, events)
    }

    /// Append the events not already retained, and return how many. Captures
    /// repeat evidence, such as a transcript parsed at every turn; this keeps
    /// one copy. `concurrent` is not part of an event's identity, and the read
    /// and append happen under one exclusive lock.
    pub fn append_new(&self, session: &SessionKey, events: &[FileEvent]) -> Result<usize> {
        let Some(dir) = self.prepare(session, events)? else { return Ok(0) };
        let _lock = crate::lock::Lock::acquire(&dir, true)?;
        let report = self.read_unlocked(session)?;
        let mut seen: std::collections::HashSet<_> = report.events.iter().map(identity).collect();
        let fresh: Vec<_> = events.iter().filter(|e| seen.insert(identity(e))).cloned().collect();
        self.append_unlocked(&dir, &fresh)?;
        Ok(fresh.len())
    }

    fn prepare(&self, session: &SessionKey, events: &[FileEvent]) -> Result<Option<PathBuf>> {
        let dir = self.directory(session)?;
        if events.is_empty() {
            return Ok(None);
        }
        validate_batch(session, events)?;
        let missing: Vec<_> =
            dir.ancestors().take_while(|p| !p.as_os_str().is_empty() && !p.exists()).map(Path::to_path_buf).collect();
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&dir)?;
        // Persist newly created directory links as well as the final batch.
        for path in missing {
            sync_directory(&path)?;
            if let Some(parent) = path.parent() {
                sync_directory(if parent.as_os_str().is_empty() { Path::new(".") } else { parent })?;
            }
        }
        Ok(Some(dir))
    }

    fn append_unlocked(&self, dir: &Path, events: &[FileEvent]) -> Result<()> {
        let cutoff = crate::maintenance::retained_from(dir)?;
        let retained: Vec<_> = events.iter().filter(|e| cutoff.is_none_or(|at| e.timestamp >= at)).collect();
        if retained.is_empty() {
            return Ok(());
        }
        let bytes = serde_json::to_vec(&retained)?;
        ensure!(bytes.len() as u64 <= MAX_BATCH_BYTES, "activity batch exceeds 16 MiB limit");
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let name = format!("{nonce:039}-{}-{}", std::process::id(), NEXT_BATCH.fetch_add(1, Ordering::Relaxed));
        let temporary = dir.join(format!("{name}.tmp"));
        let destination = dir.join(format!("{name}.json"));
        let result = write_batch(&temporary, &bytes).and_then(|()| {
            fs::rename(&temporary, destination).context("publishing activity batch")?;
            sync_directory(dir)
        });
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    /// Strict reads remain available for callers that require complete history.
    pub fn events(&self, session: &SessionKey) -> Result<Vec<FileEvent>> {
        let report = self.read(session)?;
        ensure!(
            report.warnings.is_empty(),
            "incomplete activity history: {}",
            report
                .warnings
                .iter()
                .map(|w| format!("{}: {}", w.path.display(), w.message))
                .collect::<Vec<_>>()
                .join("; ")
        );
        Ok(report.events)
    }

    /// Read every valid batch and report damaged/unreadable batches separately.
    /// Bad batches are left untouched for inspection and recovery.
    pub fn read(&self, session: &SessionKey) -> Result<ReadReport> {
        let dir = self.directory(session)?;
        if !dir.try_exists()? {
            return Ok(ReadReport::default());
        }
        let _lock = crate::lock::Lock::acquire(&dir, false)?;
        self.read_unlocked(session)
    }

    pub(crate) fn read_unlocked(&self, session: &SessionKey) -> Result<ReadReport> {
        let dir = self.directory(session)?;
        let paths = crate::maintenance::active_paths(&dir)?;
        let mut report = ReadReport::default();
        for path in paths {
            match read_batch(&path, session) {
                Ok(batch) => report.events.extend(batch),
                Err(error) => report.warnings.push(BatchWarning { path, message: format!("{error:#}") }),
            }
        }
        report.events.sort_by_key(|e| e.timestamp);
        if let Some(cutoff) = crate::maintenance::retained_from(&dir)? {
            report.events.retain(|event| event.timestamp >= cutoff);
        }
        Ok(report)
    }
}

type Identity<'a> =
    (i64, &'a Path, Option<&'a Path>, &'a Path, crate::Operation, crate::Source, crate::Outcome, Option<&'a str>);

fn identity(event: &FileEvent) -> Identity<'_> {
    (
        event.timestamp,
        &event.path,
        event.previous_path.as_deref(),
        &event.cwd,
        event.operation,
        event.source,
        event.outcome,
        event.tool_call_id.as_deref(),
    )
}

pub(crate) fn write_batch(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn validate_batch(session: &SessionKey, events: &[FileEvent]) -> Result<()> {
    for event in events {
        ensure!(&event.session == session, "event belongs to a different session");
        ensure!((1..=crate::SCHEMA_VERSION).contains(&event.schema_version), "unsupported activity schema");
        ensure!(event.path.is_absolute() && event.cwd.is_absolute(), "event paths must be absolute");
        ensure!(event.previous_path.as_ref().is_none_or(|p| p.is_absolute()), "rename origin must be absolute");
        ensure!(
            (event.operation == crate::Operation::Rename) == event.previous_path.is_some(),
            "rename operation and origin must agree"
        );
    }
    Ok(())
}

fn read_batch(path: &Path, session: &SessionKey) -> Result<Vec<FileEvent>> {
    ensure!(fs::symlink_metadata(path)?.file_type().is_file(), "batch must be a regular file");
    let mut bytes = Vec::new();
    fs::File::open(path)?.take(MAX_BATCH_BYTES + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= MAX_BATCH_BYTES, "activity batch exceeds 16 MiB limit");
    let events = serde_json::from_slice::<Vec<FileEvent>>(&bytes)?;
    validate_batch(session, &events)?;
    Ok(events)
}

pub(crate) fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    fs::File::open(path)?.sync_all().with_context(|| format!("syncing {}", path.display()))?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
