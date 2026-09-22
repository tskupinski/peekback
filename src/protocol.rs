use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// One request per connection over the daemon socket, newline-delimited JSON.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Request {
    /// Show a session's document: the given path, or its newest one.
    /// Without a session, shows the path on its own.
    Show { session_id: Option<String>, path: Option<PathBuf> },
    Status,
    Hide,
    Quit,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Response {
    Ok,
    Status { document: Option<PathBuf>, session_id: Option<String> },
    Error { message: String },
}
