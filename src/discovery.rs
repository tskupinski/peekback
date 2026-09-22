use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::registry::{Session, unix_secs};

#[derive(Clone, Debug, Serialize)]
pub struct Document {
    pub path: PathBuf,
    pub touched_at: i64,
    /// Path relative to the session's cwd when under it, otherwise absolute
    /// with the home directory shortened.
    pub label: String,
}

const CWD_WALK_DEPTH: usize = 4;
const CWD_WALK_BUDGET: usize = 20_000;
const SKIPPED_DIRS: &[&str] = &["node_modules", "target"];

/// Markdown files the session produced, newest touch first.
pub fn documents(session: &Session) -> Vec<Document> {
    let mut touched: HashMap<PathBuf, i64> = HashMap::new();
    let mut note = |path: PathBuf, at: i64| {
        let entry = touched.entry(path).or_insert(at);
        *entry = (*entry).max(at);
    };

    for (path, at) in transcript_writes(&session.transcript_path) {
        note(path, at);
    }
    if let Some(dir) = session.scratchpad_dir() {
        for (path, at) in markdown_under(&dir, usize::MAX, 0) {
            note(path, at);
        }
    }
    if let Some(dir) = session.memory_dir() {
        for (path, at) in markdown_under(&dir, 1, 0) {
            note(path, at);
        }
    }
    // A session started in the home directory or at the root would walk the
    // whole disk; those sessions get only the transcript-based sources.
    let cwd_is_broad = session.cwd == Path::new("/") || dirs::home_dir().is_some_and(|h| h == session.cwd);
    if !cwd_is_broad {
        for (path, at) in markdown_under(&session.cwd, CWD_WALK_DEPTH, session.started_at()) {
            note(path, at);
        }
    }

    let mut documents: Vec<Document> = touched
        .into_iter()
        .filter(|(path, _)| path.is_file())
        .map(|(path, touched_at)| Document { label: label_for(&path, session), path, touched_at })
        .collect();
    documents.sort_by(|a, b| b.touched_at.cmp(&a.touched_at).then_with(|| a.path.cmp(&b.path)));
    documents
}

/// Every `Write` or `Edit` of a `.md` file in the transcript, with the time
/// of the last one per path.
fn transcript_writes(transcript: &Path) -> Vec<(PathBuf, i64)> {
    let Ok(file) = fs::File::open(transcript) else { return Vec::new() };
    let mut writes = Vec::new();
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if !line.contains("\"tool_use\"") {
            continue;
        }
        let Ok(record) = serde_json::from_str::<Value>(&line) else { continue };
        if record["type"] != "assistant" {
            continue;
        }
        let at = record["timestamp"]
            .as_str()
            .and_then(|ts| OffsetDateTime::parse(ts, &Rfc3339).ok())
            .map(|t| t.unix_timestamp())
            .unwrap_or(0);
        let Some(content) = record["message"]["content"].as_array() else { continue };
        for item in content {
            if item["type"] != "tool_use" {
                continue;
            }
            if !matches!(item["name"].as_str(), Some("Write") | Some("Edit")) {
                continue;
            }
            if let Some(path) = item["input"]["file_path"].as_str().filter(|p| is_markdown(p)) {
                writes.push((PathBuf::from(path), at));
            }
        }
    }
    writes
}

/// `.md` files under `dir` modified after `since`, walking at most `depth`
/// levels and skipping hidden and build directories. The walk runs on the
/// main thread, so it stops after a fixed number of entries.
fn markdown_under(dir: &Path, depth: usize, since: i64) -> Vec<(PathBuf, i64)> {
    let mut found = Vec::new();
    let mut budget = CWD_WALK_BUDGET;
    walk(dir, depth, since, &mut found, &mut budget);
    found
}

fn walk(dir: &Path, depth: usize, since: i64, found: &mut Vec<(PathBuf, i64)>, budget: &mut usize) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        if *budget == 0 {
            return;
        }
        *budget -= 1;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            if depth > 1 && !name.starts_with('.') && !SKIPPED_DIRS.contains(&name.as_ref()) {
                walk(&path, depth - 1, since, found, budget);
            }
        } else if kind.is_file() && is_markdown(&name) {
            let modified = entry.metadata().and_then(|m| m.modified()).map(unix_secs).unwrap_or(0);
            if modified >= since {
                found.push((path, modified));
            }
        }
    }
}

fn is_markdown(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".md") || lower.ends_with(".markdown")
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
