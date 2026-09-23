//! File activity shared by consumers such as Peekback. No viewer, terminal,
//! Markdown, or global state-directory dependency.
#![doc = include_str!("../README.md")]
mod adapters;
mod lock;
mod maintenance;
mod query;
mod store;

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

pub use adapters::{hook_events, transcript_events};
pub use maintenance::MaintenanceReport;
pub use query::{FileActivity, ScanReport, ScanRoot, ScanWarning, files, reconcile, scan, scan_report};
pub use store::{BatchWarning, ReadReport, SessionReport, Store};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Agent {
    #[default]
    Claude,
    Codex,
}

impl Agent {
    pub fn name(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
        }
    }

    pub fn slug(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionKey {
    pub agent: Agent,
    pub session_id: String,
}

impl SessionKey {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(valid_id(&self.session_id), "invalid session id");
        Ok(())
    }
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id != "."
        && id != ".."
        && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Read,
    Write,
    Create,
    Modify,
    Delete,
    Rename,
    Observed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Hook,
    Transcript,
    ProjectScan,
    ScratchpadScan,
    MemoryScan,
    LegacyRegistry,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Succeeded,
    Failed,
    Unknown,
}

/// A metadata-only observation. Renames use `path` for the destination and
/// `previous_path` for the source. Unknown outcomes are never promoted by a scan.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileEvent {
    pub schema_version: u32,
    pub session: SessionKey,
    pub timestamp: i64,
    pub cwd: PathBuf,
    pub path: PathBuf,
    pub previous_path: Option<PathBuf>,
    pub operation: Operation,
    pub source: Source,
    pub outcome: Outcome,
    pub tool_call_id: Option<String>,
}

impl FileEvent {
    pub fn new(
        session: &SessionKey,
        cwd: &Path,
        path: &Path,
        timestamp: i64,
        operation: Operation,
        source: Source,
    ) -> Self {
        Self {
            schema_version: 1,
            session: session.clone(),
            timestamp,
            cwd: cwd.to_path_buf(),
            path: resolve_path(cwd, path),
            previous_path: None,
            operation,
            source,
            outcome: Outcome::Unknown,
            tool_call_id: None,
        }
    }
}

/// Lexical normalization preserves paths even after deletion and never needs
/// to follow symlinks or read the file.
pub fn resolve_path(cwd: &Path, path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in cwd.join(path).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            part => result.push(part.as_os_str()),
        }
    }
    result
}

#[cfg(test)]
mod tests;
