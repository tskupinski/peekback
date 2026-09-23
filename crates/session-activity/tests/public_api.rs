//! Exercise the published API as an external consumer, including from the
//! unpacked .crate artifact, without importing any Peekback host code.
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use session_activity::{Agent, Operation, Outcome, SessionKey, Store, files, hook_events};

#[test]
fn external_consumer_records_queries_and_compacts_both_agents() {
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let root = std::env::temp_dir().join(format!("session-activity-public-{}-{nonce}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let store = Store::new(root.join("history"));
    for agent in [Agent::Claude, Agent::Codex] {
        let key = SessionKey { agent, session_id: "same-id".into() };
        let mut input = serde_json::json!({
            "session_id": key.session_id, "cwd": root, "hook_event_name": "PostToolUse",
            "tool_use_id": "call-1", "tool_response": {"success": true}
        });
        if agent == Agent::Claude {
            input["tool_name"] = serde_json::json!("Write");
            input["tool_input"] = serde_json::json!({"file_path": "note.md"});
        } else {
            input["tool_name"] = serde_json::json!("apply_patch");
            input["tool_input"] =
                serde_json::json!({"command": "*** Begin Patch\n*** Add File: note.md\n+# Note\n*** End Patch"});
        }
        let events = hook_events(agent, &input, 100).unwrap();
        store.append(&key, &events).unwrap();
        let report = store.read(&key).unwrap();
        assert!(report.warnings.is_empty());
        let summary = files(report.events);
        assert_eq!(summary.len(), 1);
        assert_eq!(summary[0].path, root.join("note.md"));
        assert_eq!(summary[0].events[0].outcome, Outcome::Succeeded);
        assert_eq!(
            summary[0].events[0].operation,
            if agent == Agent::Claude { Operation::Write } else { Operation::Create }
        );
        assert_eq!(store.maintain(&key, None, true).unwrap().events_after, 1);
        assert_eq!(store.events(&key).unwrap(), events);
        assert_eq!(store.maintain(&key, Some(101), true).unwrap().events_removed, 1);
        assert!(store.events(&key).unwrap().is_empty());
    }
    fs::remove_dir_all(root).unwrap();
}
