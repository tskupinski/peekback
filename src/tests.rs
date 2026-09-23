use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};

use crate::discovery;
use crate::registry::{Agent, Session};

static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "peekback-test-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(path.join("project")).unwrap();
        Self(path)
    }

    fn project(&self) -> PathBuf {
        self.0.join("project")
    }
    fn record(&self) -> PathBuf {
        self.0.join("state/sessions/test-session.json")
    }
    fn session(&self) -> Session {
        serde_json::from_str(&fs::read_to_string(self.record()).unwrap()).unwrap()
    }

    fn hook(&self, agent: Option<&str>, event: &str, extra: Value) {
        let mut input = json!({
            "session_id": "test-session",
            "hook_event_name": event,
            "cwd": self.project(),
            "transcript_path": null
        });
        input.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        self.raw_hook(agent, &input.to_string());
    }

    fn raw_hook(&self, agent: Option<&str>, input: &str) {
        let Ok(input) = serde_json::from_str(input) else { return };
        let agent = if agent == Some("codex") { Agent::Codex } else { Agent::Claude };
        let terminal = crate::registry::Terminal {
            tmux_pane: Some("%7".into()),
            tmux_socket: Some("/tmp/peekback-test.sock".into()),
            ..Default::default()
        };
        crate::hooks::register(&self.0.join("state"), agent, &input, crate::registry::now_unix(), terminal).unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn all_session_documents_merge_paths_keep_provenance_and_skip_unusable_history() {
    use session_activity::{FileEvent, Operation, Outcome, SessionKey, Source, Store};
    let f = Fixture::new();
    let store = Store::new(f.0.join("history"));
    for name in ["shared.md", "newest.MARKDOWN", "read.md", "failed.md", "code.rs", "untracked.md"] {
        fs::write(f.project().join(name), "# Content").unwrap();
    }
    for agent in [Agent::Claude, Agent::Codex] {
        let key = SessionKey { agent, session_id: "same-id".into() };
        let event =
            FileEvent::new(&key, &f.project(), std::path::Path::new("shared.md"), 10, Operation::Write, Source::Hook);
        store.append(&key, &[event]).unwrap();
        store.maintain(&key, None, true).unwrap();
    }
    let key = SessionKey { agent: Agent::Codex, session_id: "ended".into() };
    for (name, operation, outcome, timestamp) in [
        ("newest.MARKDOWN", Operation::Modify, Outcome::Succeeded, 20),
        ("read.md", Operation::Read, Outcome::Succeeded, 30),
        ("failed.md", Operation::Write, Outcome::Failed, 30),
        ("code.rs", Operation::Write, Outcome::Succeeded, 30),
        ("missing.md", Operation::Delete, Outcome::Succeeded, 30),
    ] {
        let mut event =
            FileEvent::new(&key, &f.project(), std::path::Path::new(name), timestamp, operation, Source::Hook);
        event.outcome = outcome;
        store.append(&key, &[event]).unwrap();
    }
    fs::write(f.0.join("history/codex/ended/damaged.json"), "{").unwrap();
    let report = discovery::all_documents(&store);
    assert_eq!(report.documents.len(), 2);
    assert_eq!(report.documents[0].document.path, f.project().join("newest.MARKDOWN"));
    assert_eq!(report.documents[1].document.path, f.project().join("shared.md"));
    assert_eq!(report.documents[1].sessions.len(), 2);
    assert_ne!(report.documents[1].sessions[0].agent, report.documents[1].sessions[1].agent);
    assert_eq!(report.warnings.len(), 1);
    assert!(report.warnings[0].contains("damaged.json"));
    assert!(!f.record().exists()); // No live registry entries required.
}

#[test]
fn codex_hook_lifecycle_without_transcript() {
    let f = Fixture::new();
    f.hook(Some("codex"), "SessionStart", json!({}));
    let s = f.session();
    assert_eq!(s.agent, Agent::Codex);
    assert!(s.transcript_path.is_none());
    assert!(s.memory_dir().is_none());
    assert!(s.scratchpad_dir().is_none());
    assert_eq!(s.terminal.tmux_pane.as_deref(), Some("%7"));
    assert_eq!(s.terminal.tmux_socket.as_deref(), Some("/tmp/peekback-test.sock"));
    let mut record: Value = serde_json::from_str(&fs::read_to_string(f.record()).unwrap()).unwrap();
    record["started_at"] = json!(123);
    fs::write(f.record(), record.to_string()).unwrap();
    f.hook(Some("codex"), "UserPromptSubmit", json!({}));
    f.hook(Some("codex"), "Stop", json!({}));
    assert_eq!(f.session().started_at, 123);
    f.hook(Some("codex"), "SessionEnd", json!({}));
    assert!(!f.record().exists());
}

#[test]
fn codex_discovers_patch_paths_and_shell_written_project_files() {
    let f = Fixture::new();
    fs::create_dir_all(f.project().join(".hidden")).unwrap();
    let external = f.0.join("outside with spaces.md");
    let hidden = f.project().join(".hidden/plan.md");
    let moved = f.project().join("renamed.MARKDOWN");
    let shell = f.project().join("shell.md");
    for path in [&external, &hidden, &moved, &shell] {
        fs::write(path, "# Plan\n").unwrap();
    }
    f.hook(Some("codex"), "SessionStart", json!({}));
    let patch = format!(
        "*** Begin Patch\n*** Add File: {}\n+# External\n*** Add File: .hidden/plan.md\n+# Plan\n+*** Add File: fake.md\n*** Update File: old.md\n*** Move to: renamed.MARKDOWN\n@@\n-old\n+new\n*** Delete File: deleted.md\n*** Add File: main.rs\n+fn main() {{}}\n*** End Patch",
        external.display()
    );
    for _ in 0..2 {
        f.hook(
            Some("codex"),
            "PostToolUse",
            json!({
                "tool_name": "apply_patch", "tool_input": {"command": patch}
            }),
        );
    }
    f.hook(Some("codex"), "Stop", json!({}));
    let s = f.session();
    let store = session_activity::Store::new(f.0.join("state/activity"));
    assert_eq!(store.events(&s.activity_key()).unwrap().len(), 10);
    let docs = discovery::documents_in(&s, &store);
    assert_eq!(docs.len(), 4);
    for path in [&external, &hidden, &moved, &shell] {
        assert!(docs.iter().any(|d| &d.path == path), "missing {}", path.display());
    }
}

#[test]
fn codex_ignores_patch_text_in_shell_commands_and_missing_input() {
    let f = Fixture::new();
    for (tool, input) in [
        ("Bash", json!({"command": "*** Begin Patch\n*** Add File: fake.md\n+x\n*** End Patch"})),
        ("apply_patch", json!({"command": 42})),
        ("apply_patch", json!({})),
    ] {
        f.hook(Some("codex"), "PostToolUse", json!({"tool_name": tool, "tool_input": input}));
        assert!(f.session().written_files.is_empty());
    }
}

#[test]
fn old_claude_records_and_default_hook_still_work() {
    let f = Fixture::new();
    let transcript = f.0.join("transcript.jsonl");
    let document = f.0.join("claude.md");
    fs::write(&document, "# Claude\n").unwrap();
    fs::write(
        &transcript,
        json!({
            "type": "assistant", "timestamp": "2026-09-23T00:00:00Z",
            "message": {"content": [{"type": "tool_use", "name": "Write", "input": {"file_path": document}}]}
        })
        .to_string(),
    )
    .unwrap();
    f.hook(None, "SessionStart", json!({"transcript_path": transcript}));
    let mut legacy = serde_json::to_value(f.session()).unwrap();
    legacy.as_object_mut().unwrap().remove("agent");
    legacy.as_object_mut().unwrap().remove("written_files");
    let s: Session = serde_json::from_value(legacy).unwrap();
    assert_eq!(s.agent, Agent::Claude);
    assert!(s.memory_dir().is_some());
    assert!(s.scratchpad_dir().is_some());
    assert_eq!(
        discovery::documents_in(&s, &session_activity::Store::new(f.0.join("state/activity")))[0].path,
        document
    );
}

#[test]
fn malformed_hooks_do_not_create_sessions() {
    let f = Fixture::new();
    for input in ["not json", "{}", r#"{"session_id":42}"#] {
        f.raw_hook(Some("codex"), input);
    }
    f.hook(Some("codex"), "SessionStart", json!({"session_id": "../../escape"}));
    f.hook(Some("codex"), "SessionStart", json!({"cwd": null}));
    f.hook(Some("codex"), "SessionStart", json!({"transcript_path": 42}));
    f.hook(Some("codex"), "UnknownEvent", json!({}));
    assert!(!f.record().exists());
}

#[test]
fn claude_transcript_history_is_retained_once_across_resumes() {
    let f = Fixture::new();
    let transcript = f.0.join("transcript.jsonl");
    fs::write(&transcript, json!({
        "type": "assistant", "timestamp": "2026-09-23T00:00:00Z",
        "message": {"content": [{"type":"tool_use", "id":"write-1", "name":"Write", "input":{"file_path":"gone.rs"}}]}
    }).to_string()).unwrap();
    for _ in 0..2 {
        f.hook(None, "SessionStart", json!({"transcript_path":transcript}));
        f.hook(None, "SessionEnd", json!({}));
    }
    let key = session_activity::SessionKey { agent: Agent::Claude, session_id: "test-session".into() };
    let store = session_activity::Store::new(f.0.join("state/activity"));
    let events = store.events(&key).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].path, f.project().join("gone.rs"));
    assert_eq!(events[0].source, session_activity::Source::Transcript);
    assert!(!f.record().exists());
}

#[test]
fn viewer_filters_read_and_failed_events_but_keeps_legacy_markdown() {
    let f = Fixture::new();
    let external = f.0.join("external.md");
    fs::write(&external, "# Existing document").unwrap();
    f.hook(
        None,
        "PostToolUse",
        json!({
            "tool_name":"Read", "tool_input":{"file_path":external}, "tool_response":{"success":true}
        }),
    );
    f.hook(
        None,
        "PostToolUse",
        json!({
            "tool_name":"Edit", "tool_input":{"file_path":external}, "tool_response":{"is_error":true}
        }),
    );
    let store = session_activity::Store::new(f.0.join("state/activity"));
    let mut session = f.session();
    assert_eq!(store.events(&session.activity_key()).unwrap().len(), 2);
    assert!(discovery::documents_in(&session, &store).is_empty());
    session.written_files.push(external.clone());
    assert_eq!(discovery::documents_in(&session, &store)[0].path, external);
}

#[test]
fn failed_hook_is_not_resurrected_by_an_unknown_transcript_request() {
    let f = Fixture::new();
    let external = f.0.join("external.md");
    let transcript = f.0.join("transcript.jsonl");
    fs::write(&external, "# Original document").unwrap();
    fs::write(
        &transcript,
        json!({"type":"assistant", "message":{"content":[
            {"type":"tool_use", "id":"edit-1", "name":"Edit", "input":{"file_path":external}}
        ]}})
        .to_string(),
    )
    .unwrap();
    f.hook(
        None,
        "PostToolUseFailure",
        json!({"transcript_path":transcript, "tool_name":"Edit",
        "tool_use_id":"edit-1", "tool_input":{"file_path":external}}),
    );
    let store = session_activity::Store::new(f.0.join("state/activity"));
    assert!(discovery::documents_in(&f.session(), &store).is_empty());
}

#[test]
fn viewer_keeps_healthy_files_when_one_batch_is_corrupt() {
    let f = Fixture::new();
    let external = f.0.join("external.md");
    fs::write(&external, "# Document").unwrap();
    f.hook(
        None,
        "PostToolUse",
        json!({"tool_name":"Write", "tool_input":{"file_path":external},
        "tool_response":{"success":true}}),
    );
    fs::write(f.0.join("state/activity/claude/test-session/corrupt.json"), "{").unwrap();
    let store = session_activity::Store::new(f.0.join("state/activity"));
    assert_eq!(discovery::documents_in(&f.session(), &store)[0].path, external);
    f.hook(None, "SessionEnd", json!({}));
    assert!(!f.record().exists());
    assert_eq!(
        store
            .read(&session_activity::SessionKey { agent: Agent::Claude, session_id: "test-session".into() })
            .unwrap()
            .events
            .len(),
        1
    );
}

#[test]
fn late_hooks_cannot_revive_an_ended_session_but_keep_file_history() {
    let f = Fixture::new();
    f.hook(Some("codex"), "SessionStart", json!({}));
    f.hook(Some("codex"), "SessionEnd", json!({}));
    for event in ["PostToolUse", "Stop", "UserPromptSubmit"] {
        f.hook(
            Some("codex"),
            event,
            json!({"tool_name":"apply_patch",
            "tool_input":{"command":"*** Begin Patch\n*** Add File: late.md\n+late\n*** End Patch"}}),
        );
        assert!(!f.record().exists());
    }
    let key = session_activity::SessionKey { agent: Agent::Codex, session_id: "test-session".into() };
    assert!(crate::lifecycle::ended(&f.0.join("state"), &key));
    assert_eq!(session_activity::Store::new(f.0.join("state/activity")).events(&key).unwrap().len(), 1);
    f.hook(Some("codex"), "SessionStart", json!({"source":"resume"}));
    assert!(f.record().exists());
    assert!(!crate::lifecycle::ended(&f.0.join("state"), &key));
}

#[test]
fn concurrent_hooks_and_end_leave_session_closed_without_losing_events() {
    let f = Fixture::new();
    f.hook(Some("codex"), "SessionStart", json!({}));
    std::thread::scope(|scope| {
        for _ in 0..12 {
            let f = &f;
            scope.spawn(move || {
                f.hook(
                    Some("codex"),
                    "PostToolUse",
                    json!({"tool_name":"apply_patch",
                "tool_input":{"command":"*** Begin Patch\n*** Add File: concurrent.md\n+text\n*** End Patch"}}),
                )
            });
        }
        let f = &f;
        scope.spawn(move || f.hook(Some("codex"), "SessionEnd", json!({})));
    });
    assert!(!f.record().exists());
    let key = session_activity::SessionKey { agent: Agent::Codex, session_id: "test-session".into() };
    assert_eq!(session_activity::Store::new(f.0.join("state/activity")).events(&key).unwrap().len(), 12);
}

#[test]
fn delayed_hooks_do_not_move_activity_backwards_or_erase_transcript() {
    let f = Fixture::new();
    let input = json!({"session_id":"test-session", "cwd":f.project(), "transcript_path":"/tmp/history.jsonl",
        "hook_event_name":"SessionStart"});
    crate::hooks::register(&f.0.join("state"), Agent::Codex, &input, 200, Default::default()).unwrap();
    let input = json!({"session_id":"test-session", "cwd":f.project(), "transcript_path":null,
        "hook_event_name":"Stop"});
    crate::hooks::register(&f.0.join("state"), Agent::Codex, &input, 100, Default::default()).unwrap();
    let session = f.session();
    assert_eq!(session.last_active_at, 200);
    assert_eq!(session.transcript_path, Some("/tmp/history.jsonl".into()));
}
