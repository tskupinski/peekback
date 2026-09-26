//! File activity shared by consumers such as Peekback. No viewer, terminal,
//! Markdown, or global state-directory dependency.
#![doc = include_str!("../README.md")]
mod lock;
mod maintenance;
mod query;
mod store;

use std::fmt;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

pub use maintenance::MaintenanceReport;
pub use query::{FileActivity, ScanReport, ScanRoot, ScanWarning, files, reconcile, scan, scan_report};
pub use store::{BatchWarning, ReadReport, SessionReport, Store};

/// Schema 2 adds `FileEvent::concurrent`. Schema 1 batches remain readable.
pub const SCHEMA_VERSION: u32 = 2;

/// The namespace of the agent that produced a session, such as `claude`. The
/// caller chooses it; the library only requires lowercase ASCII letters,
/// digits and `-`, since it names a directory.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct AgentId(String);

impl AgentId {
    pub fn new(id: impl Into<String>) -> anyhow::Result<Self> {
        let id = id.into();
        anyhow::ensure!(
            !id.is_empty() && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
            "invalid agent id {id:?}"
        );
        Ok(Self(id))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for AgentId {
    type Error = anyhow::Error;

    fn try_from(id: String) -> anyhow::Result<Self> {
        Self::new(id)
    }
}

impl From<AgentId> for String {
    fn from(id: AgentId) -> Self {
        id.0
    }
}

impl fmt::Display for AgentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionKey {
    pub agent: AgentId,
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
    /// Other sessions that were working where a scan observed this file, so
    /// the filesystem cannot tell which of them changed it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub concurrent: Vec<SessionKey>,
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
            schema_version: SCHEMA_VERSION,
            session: session.clone(),
            timestamp,
            cwd: cwd.to_path_buf(),
            path: resolve_path(cwd, path),
            previous_path: None,
            operation,
            source,
            outcome: Outcome::Unknown,
            tool_call_id: None,
            concurrent: Vec::new(),
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
