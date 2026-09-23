use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::time::UNIX_EPOCH;

use serde::Serialize;

use crate::{FileEvent, Operation, Outcome, SessionKey, Source};

/// Reconcile outcome evidence for the same tool invocation and remove retries.
/// Missing call IDs are not sufficient evidence that two observations are the
/// same operation. Conflicting known outcomes remain visible to the consumer.
pub fn reconcile(events: impl IntoIterator<Item = FileEvent>) -> Vec<FileEvent> {
    let events: Vec<_> = events.into_iter().collect();
    let identity = |e: &FileEvent| {
        e.tool_call_id
            .as_ref()
            .filter(|id| !id.is_empty())
            .map(|id| (e.session.clone(), id.clone(), e.operation, e.path.clone(), e.previous_path.clone()))
    };
    let mut outcomes = HashMap::<_, HashSet<Outcome>>::new();
    for event in &events {
        if event.outcome != Outcome::Unknown {
            if let Some(key) = identity(event) {
                outcomes.entry(key).or_default().insert(event.outcome);
            }
        }
    }
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for mut event in events {
        if let Some(key) = identity(&event) {
            if event.outcome == Outcome::Unknown {
                if let Some(known) = outcomes.get(&key).filter(|values| values.len() == 1) {
                    event.outcome = *known.iter().next().unwrap();
                }
            }
            if !seen.insert((key, event.source, event.outcome)) {
                continue;
            }
        }
        result.push(event);
    }
    result
}

#[derive(Clone, Debug, Serialize)]
pub struct FileActivity {
    pub path: PathBuf,
    pub last_touched_at: i64,
    pub exists: bool,
    /// Keep evidence and outcomes attached to the summary, rather than turning
    /// filesystem candidates into attributed writes.
    pub events: Vec<FileEvent>,
}

/// Group observations by path; retain deleted paths and both sides of renames.
/// Filtering by extension, outcome, or operation belongs to the consumer.
pub fn files(events: impl IntoIterator<Item = FileEvent>) -> Vec<FileActivity> {
    let mut grouped: BTreeMap<PathBuf, Vec<FileEvent>> = BTreeMap::new();
    for event in reconcile(events) {
        if let Some(previous) = &event.previous_path {
            grouped.entry(previous.clone()).or_default().push(event.clone());
        }
        grouped.entry(event.path.clone()).or_default().push(event);
    }
    let mut result: Vec<_> = grouped
        .into_iter()
        .map(|(path, mut events)| {
            events.sort_by_key(|event| event.timestamp);
            FileActivity {
                exists: path.is_file(),
                path,
                last_touched_at: events.last().map_or(0, |e| e.timestamp),
                events,
            }
        })
        .collect();
    result.sort_by(|a, b| b.last_touched_at.cmp(&a.last_touched_at).then_with(|| a.path.cmp(&b.path)));
    result
}

pub struct ScanRoot {
    pub path: PathBuf,
    pub since: i64,
    pub depth: usize,
    pub source: Source,
}

/// Completion metadata for a bounded scan. An empty result alone cannot tell
/// the caller whether there were no matching files or the scan was incomplete.
#[derive(Debug, Default)]
pub struct ScanReport {
    pub events: Vec<FileEvent>,
    pub entries_visited: usize,
    pub budget_exhausted: bool,
    pub depth_limited: bool,
    pub warnings: Vec<ScanWarning>,
}

#[derive(Debug)]
pub struct ScanWarning {
    pub path: PathBuf,
    pub message: String,
}

pub fn scan(session: &SessionKey, cwd: &std::path::Path, roots: &[ScanRoot], budget: usize) -> Vec<FileEvent> {
    scan_report(session, cwd, roots, budget).events
}

/// Hidden/build directories are excluded by policy, symlink directories are
/// not followed, and all roots share the entry budget. Missing optional roots
/// are normal; other read/metadata errors are reported with their paths.
pub fn scan_report(session: &SessionKey, cwd: &std::path::Path, roots: &[ScanRoot], budget: usize) -> ScanReport {
    let mut report = ScanReport::default();
    for root in roots {
        walk(session, cwd, root, &root.path, root.depth, budget, &mut report);
        if report.budget_exhausted {
            break;
        }
    }
    report
}

fn walk(
    session: &SessionKey,
    cwd: &std::path::Path,
    root: &ScanRoot,
    dir: &std::path::Path,
    depth: usize,
    budget: usize,
    report: &mut ScanReport,
) {
    if depth == 0 {
        report.depth_limited = true;
        return;
    }
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if dir == root.path && e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            report.warnings.push(ScanWarning { path: dir.into(), message: e.to_string() });
            return;
        }
    };
    for entry in entries {
        if report.entries_visited == budget {
            report.budget_exhausted = true;
            return;
        }
        report.entries_visited += 1;
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                report.warnings.push(ScanWarning { path: dir.into(), message: e.to_string() });
                continue;
            }
        };
        let path = entry.path();
        let kind = match entry.file_type() {
            Ok(kind) => kind,
            Err(e) => {
                report.warnings.push(ScanWarning { path, message: e.to_string() });
                continue;
            }
        };
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if kind.is_dir() && !name.starts_with('.') && !["node_modules", "target"].contains(&name.as_ref()) {
            walk(session, cwd, root, &path, depth - 1, budget, report);
            if report.budget_exhausted {
                return;
            }
        } else if kind.is_file() {
            let at = match entry.metadata().and_then(|m| m.modified()) {
                Ok(at) => at.duration_since(UNIX_EPOCH).map_or(0, |t| t.as_secs() as i64),
                Err(e) => {
                    report.warnings.push(ScanWarning { path, message: e.to_string() });
                    continue;
                }
            };
            if at >= root.since {
                report.events.push(FileEvent::new(session, cwd, &path, at, Operation::Observed, root.source));
            }
        }
    }
}
