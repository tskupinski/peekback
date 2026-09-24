use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// One request per connection over the daemon socket, newline-delimited JSON.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Request {
    /// Show a session's document: the given path, or its newest one.
    /// Without a session, shows the path on its own.
    Show {
        session_id: Option<String>,
        path: Option<PathBuf>,
        /// Take keyboard focus. Clients from before this field never asked
        /// for focus, so a missing value keeps that behavior.
        #[serde(default)]
        focus: bool,
    },
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn show_requests_from_older_clients_do_not_take_focus() {
        let old: Request = serde_json::from_str(r#"{"type":"show","session_id":null,"path":"/a.md"}"#).unwrap();
        assert!(matches!(old, Request::Show { focus: false, .. }));
    }
}
