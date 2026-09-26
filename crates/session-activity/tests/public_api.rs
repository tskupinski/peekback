//! Exercise the public API as an external consumer, without importing any
//! Peekback host code.
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use session_activity::{AgentId, FileEvent, Operation, Outcome, SessionKey, Source, Store, files};

#[test]
fn external_consumer_records_queries_and_compacts_any_agent() {
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let root = std::env::temp_dir().join(format!("session-activity-public-{}-{nonce}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let store = Store::new(root.join("history"));
    for agent in ["claude", "codex", "another-agent"] {
        let key = SessionKey { agent: AgentId::new(agent).unwrap(), session_id: "same-id".into() };
        let mut event = FileEvent::new(&key, &root, Path::new("note.md"), 100, Operation::Write, Source::Hook);
        event.outcome = Outcome::Succeeded;
        event.tool_call_id = Some("call-1".into());
        let events = vec![event];
        store.append(&key, &events).unwrap();
        let report = store.read(&key).unwrap();
        assert!(report.warnings.is_empty());
        let summary = files(report.events);
        assert_eq!(summary.len(), 1);
        assert_eq!(summary[0].path, root.join("note.md"));
        assert_eq!(summary[0].events[0].outcome, Outcome::Succeeded);
        assert_eq!(store.maintain(&key, None, true).unwrap().events_after, 1);
        assert_eq!(store.events(&key).unwrap(), events);
        assert_eq!(store.maintain(&key, Some(101), true).unwrap().events_removed, 1);
        assert!(store.events(&key).unwrap().is_empty());
    }
    assert_eq!(store.sessions().sessions.len(), 3);
    fs::remove_dir_all(root).unwrap();
}
