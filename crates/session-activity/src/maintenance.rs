use std::collections::HashSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

use crate::store::{MAX_BATCH_BYTES, sync_directory, write_batch};
use crate::{FileEvent, SessionKey, Store};

static NEXT: AtomicU64 = AtomicU64::new(0);
const CHECKPOINT: &str = ".checkpoint";

/// The checkpoint is the atomic commit point. A reader ignores covered source
/// batches even if a crash interrupted their deletion. Unreferenced packs are
/// invisible, including ones written before an interrupted commit.
#[derive(Default, Serialize, Deserialize)]
struct Checkpoint {
    version: u32,
    #[serde(default)]
    retained_from: Option<i64>,
    chunks: Vec<String>,
    covered: Vec<String>,
}

#[derive(Debug, Default, Serialize)]
pub struct MaintenanceReport {
    pub dry_run: bool,
    pub batches_before: usize,
    pub batches_after: usize,
    pub events_before: usize,
    pub events_after: usize,
    pub events_removed: usize,
    pub retained_from: Option<i64>,
    pub cleanup_warnings: Vec<String>,
}

fn checkpoint(dir: &Path) -> Result<Checkpoint> {
    let path = dir.join(CHECKPOINT);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Checkpoint::default()),
        Err(e) => return Err(e.into()),
    };
    ensure!(metadata.file_type().is_file(), "checkpoint must be a regular file");
    let mut bytes = Vec::new();
    fs::File::open(path)?.take(MAX_BATCH_BYTES + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= MAX_BATCH_BYTES, "checkpoint exceeds size limit");
    let checkpoint: Checkpoint = serde_json::from_slice(&bytes)?;
    ensure!(checkpoint.version == 1, "unsupported history checkpoint version");
    for name in &checkpoint.chunks {
        ensure!(safe_name(name, ".pack"), "invalid checkpoint chunk name");
    }
    for name in &checkpoint.covered {
        ensure!(safe_name(name, ".json"), "invalid checkpoint source name");
    }
    ensure!(
        checkpoint.chunks.iter().collect::<HashSet<_>>().len() == checkpoint.chunks.len(),
        "duplicate checkpoint chunk"
    );
    Ok(checkpoint)
}

fn safe_name(name: &str, suffix: &str) -> bool {
    name.ends_with(suffix) && crate::valid_id(name)
}

pub(crate) fn retained_from(dir: &Path) -> Result<Option<i64>> {
    Ok(checkpoint(dir)?.retained_from)
}

pub(crate) fn active_paths(dir: &Path) -> Result<Vec<PathBuf>> {
    let checkpoint = checkpoint(dir)?;
    let covered: HashSet<_> = checkpoint.covered.into_iter().collect();
    let mut incoming = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "json")
            && !path.file_name().and_then(|n| n.to_str()).is_some_and(|n| covered.contains(n))
        {
            incoming.push(path);
        }
    }
    incoming.sort();
    let mut paths: Vec<_> = checkpoint.chunks.iter().map(|name| dir.join(name)).collect();
    paths.extend(incoming);
    Ok(paths)
}

impl Store {
    /// Fixed lower timestamp bound chosen by explicit retention. This also
    /// prevents a resumed session from reimporting expired transcript events.
    pub fn retained_from(&self, session: &SessionKey) -> Result<Option<i64>> {
        let dir = self.directory(session)?;
        if !dir.try_exists()? {
            return Ok(None);
        }
        let _lock = crate::lock::Lock::acquire(&dir, false)?;
        retained_from(&dir)
    }
    /// Pack history into fewer files, optionally retaining only timestamps at
    /// or after `before`. Preview is read-only apart from the advisory lock.
    /// Refuses damaged history. Never reconciles or discards raw observations
    /// unless an explicit retention cutoff is supplied.
    pub fn maintain(&self, session: &SessionKey, before: Option<i64>, apply: bool) -> Result<MaintenanceReport> {
        let dir = self.directory(session)?;
        let mut report = MaintenanceReport { dry_run: !apply, ..Default::default() };
        if !dir.try_exists()? {
            return Ok(report);
        }
        let _lock = crate::lock::Lock::acquire(&dir, true)?;
        let before = before.max(retained_from(&dir)?);
        report.retained_from = before;
        let paths = active_paths(&dir)?;
        let history = self.read_unlocked(session)?;
        ensure!(history.warnings.is_empty(), "refusing maintenance: history contains unreadable or invalid batches");
        report.batches_before = paths.len();
        report.events_before = history.events.len();
        let events: Vec<_> =
            history.events.into_iter().filter(|e| before.is_none_or(|cutoff| e.timestamp >= cutoff)).collect();
        report.events_after = events.len();
        report.events_removed = report.events_before - report.events_after;
        let chunks = pack(&events)?;
        report.batches_after = chunks.len();
        if !apply {
            return Ok(report);
        }

        // Include old covered sources left by an interrupted cleanup, but do
        // not accumulate names for files that were already successfully removed.
        let mut covered: Vec<_> = paths
            .iter()
            .filter(|path| path.extension().is_some_and(|e| e == "json"))
            .filter_map(|path| path.file_name().and_then(|n| n.to_str()).map(str::to_owned))
            .collect();
        covered.extend(checkpoint(&dir)?.covered.into_iter().filter(|name| dir.join(name).exists()));
        let mut old_packs = Vec::new();
        for entry in fs::read_dir(&dir)? {
            let path = entry?.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            if safe_name(name, ".pack") {
                old_packs.push(path);
            }
        }
        covered.sort();
        covered.dedup();
        ensure!(covered.iter().all(|name| safe_name(name, ".json")), "invalid source batch name");
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let prefix = format!("compact-{nonce}-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
        let mut names = Vec::new();
        for (index, bytes) in chunks.iter().enumerate() {
            let name = format!("{prefix}-{index}.pack");
            publish(&dir, &name, bytes)?;
            names.push(name);
        }
        let checkpoint = Checkpoint { version: 1, retained_from: before, chunks: names, covered: covered.clone() };
        let bytes = serde_json::to_vec(&checkpoint)?;
        ensure!(bytes.len() as u64 <= MAX_BATCH_BYTES, "checkpoint exceeds size limit");
        publish(&dir, CHECKPOINT, &bytes)?;

        // The checkpoint is durable before cleanup. Failure here only leaves
        // ignored storage behind; report it so a later run can reclaim it.
        for path in covered.iter().map(|name| dir.join(name)).chain(old_packs) {
            if let Err(error) = fs::remove_file(&path) {
                report.cleanup_warnings.push(format!("{}: {error}", path.display()));
            }
        }
        sync_directory(&dir)?;
        Ok(report)
    }
}

fn publish(dir: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    let temporary = dir.join(format!("{name}.{}-{}.tmp", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let result = (|| {
        write_batch(&temporary, bytes)?;
        fs::rename(&temporary, dir.join(name))?;
        sync_directory(dir)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn pack(events: &[FileEvent]) -> Result<Vec<Vec<u8>>> {
    let mut chunks = Vec::new();
    let mut chunk = vec![b'['];
    for event in events {
        let bytes = serde_json::to_vec(event)?;
        ensure!(bytes.len() as u64 + 2 <= MAX_BATCH_BYTES, "event exceeds batch size limit");
        if chunk.len() > 1 && (chunk.len() + bytes.len() + 2) as u64 > MAX_BATCH_BYTES {
            chunk.push(b']');
            chunks.push(chunk);
            chunk = vec![b'['];
        }
        if chunk.len() > 1 {
            chunk.push(b',');
        }
        chunk.extend(bytes);
    }
    if chunk.len() > 1 {
        chunk.push(b']');
        chunks.push(chunk);
    }
    Ok(chunks)
}
