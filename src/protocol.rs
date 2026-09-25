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

/// What a client sends: the request and when it stops waiting for the reply,
/// in Unix milliseconds. The daemon must not apply it later than that.
#[derive(Serialize)]
pub struct Outgoing<'a> {
    #[serde(flatten)]
    pub request: &'a Request,
    pub expires_at_ms: u64,
}

/// Clients from before the expiry field leave the daemon's own limit in charge.
#[derive(Deserialize)]
pub struct Incoming {
    #[serde(flatten)]
    pub request: Request,
    #[serde(default)]
    pub expires_at_ms: Option<u64>,
}

pub fn unix_ms(time: std::time::SystemTime) -> u64 {
    time.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
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
    fn requests_carry_their_expiry_and_older_ones_parse_without_it() {
        let request = Request::Show { session_id: None, path: Some("/a.md".into()), focus: true };
        let sent = serde_json::to_string(&Outgoing { request: &request, expires_at_ms: 42 }).unwrap();
        let received: Incoming = serde_json::from_str(&sent).unwrap();
        assert_eq!(received.expires_at_ms, Some(42));
        assert!(matches!(received.request, Request::Show { focus: true, .. }));
        let old: Incoming = serde_json::from_str(r#"{"type":"status"}"#).unwrap();
        assert_eq!(old.expires_at_ms, None);
    }

    #[test]
    fn show_requests_from_older_clients_do_not_take_focus() {
        let old: Request = serde_json::from_str(r#"{"type":"show","session_id":null,"path":"/a.md"}"#).unwrap();
        assert!(matches!(old, Request::Show { focus: false, .. }));
    }
}
