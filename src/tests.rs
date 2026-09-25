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

/// Backdate a file, since captures compare modification times with turns.
fn age(path: &std::path::Path, secs: u64) {
    let at = std::time::SystemTime::now() - std::time::Duration::from_secs(secs);
    fs::File::options().write(true).open(path).unwrap().set_modified(at).unwrap();
}

fn store(f: &Fixture) -> session_activity::Store {
    session_activity::Store::new(f.0.join("state/activity"))
}

fn documents(f: &Fixture) -> Vec<PathBuf> {
    discovery::documents_in(&f.session(), &store(f)).into_iter().map(|d| d.path).collect()
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
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
    f.hook(Some("codex"), "SessionStart", json!({}));
    f.hook(Some("codex"), "UserPromptSubmit", json!({}));
    for path in [&external, &hidden, &moved, &shell] {
        fs::write(path, "# Plan\n").unwrap();
    }
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
    let store = store(&f);
    let events = store.events(&s.activity_key()).unwrap();
    assert_eq!(events.iter().filter(|e| e.source == session_activity::Source::Hook).count(), 10);
    let docs = discovery::documents_in(&s, &store);
    assert_eq!(docs.len(), 4);
    assert!(docs.iter().all(|d| d.scanned == (d.path == shell)));
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
    for field in ["agent", "written_files", "turn_started_at", "recent_turns"] {
        legacy.as_object_mut().unwrap().remove(field);
    }
    let s: Session = serde_json::from_value(legacy).unwrap();
    assert_eq!(s.agent, Agent::Claude);
    assert!(s.memory_dir().is_some());
    assert!(s.scratchpad_dir().is_some());
    assert!(discovery::documents_in(&s, &store(&f)).is_empty()); // Nothing is inferred at display time.
    f.hook(None, "Stop", json!({}));
    assert_eq!(documents(&f), [document]);
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
    assert_eq!(store(&f).events(&f.session().activity_key()).unwrap().len(), 2);
    assert!(documents(&f).is_empty());
    let mut record: Value = serde_json::from_str(&fs::read_to_string(f.record()).unwrap()).unwrap();
    record["written_files"] = json!([external]);
    fs::write(f.record(), record.to_string()).unwrap();
    f.hook(None, "Stop", json!({}));
    assert_eq!(documents(&f), [external]);
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

#[test]
fn turns_capture_shell_writes_once_and_keep_them_after_later_edits() {
    let f = Fixture::new();
    let before = f.project().join("before.md");
    let during = f.project().join("during.md");
    let nested = f.project().join("worktree/other.md");
    fs::create_dir_all(f.project().join("worktree")).unwrap();
    fs::write(f.project().join("worktree/.git"), "gitdir: elsewhere").unwrap();
    fs::write(&before, "# Before").unwrap();
    age(&before, 60);
    f.hook(None, "SessionStart", json!({}));
    f.hook(None, "UserPromptSubmit", json!({}));
    assert!(f.session().turn_started_at.is_some());
    fs::write(&during, "# During").unwrap();
    fs::write(&nested, "# Another worktree").unwrap();
    assert!(documents(&f).is_empty()); // Shell writes appear when the turn closes.
    f.hook(None, "Stop", json!({}));
    let session = f.session();
    assert!(session.turn_started_at.is_none());
    assert_eq!(session.recent_turns.len(), 1);
    assert_eq!(documents(&f), [during.clone()]);
    fs::write(&during, "# Edited by hand between turns").unwrap();
    age(&during, 0);
    f.hook(None, "Stop", json!({}));
    assert_eq!(documents(&f), [during]);
    let scans = store(&f).events(&session.activity_key()).unwrap();
    assert_eq!(scans.iter().filter(|e| e.source == session_activity::Source::ProjectScan).count(), 1);
}

#[test]
fn an_interrupted_turn_is_closed_by_the_next_prompt_and_compaction_keeps_it_open() {
    let f = Fixture::new();
    let interrupted = f.project().join("interrupted.md");
    f.hook(None, "SessionStart", json!({}));
    f.hook(None, "UserPromptSubmit", json!({}));
    fs::write(&interrupted, "# No Stop followed").unwrap();
    f.hook(None, "SessionStart", json!({"source": "compact"}));
    assert!(f.session().turn_started_at.is_some());
    f.hook(None, "UserPromptSubmit", json!({}));
    assert_eq!(documents(&f), [interrupted]);
    assert!(f.session().turn_started_at.is_some());
    f.hook(None, "SessionStart", json!({"source": "resume"}));
    assert!(f.session().turn_started_at.is_none()); // The process that owned it is gone.
}

#[test]
fn session_end_captures_the_open_turn_and_resume_keeps_everything() {
    let f = Fixture::new();
    let written = f.project().join("written.md");
    f.hook(None, "SessionStart", json!({}));
    f.hook(None, "UserPromptSubmit", json!({}));
    fs::write(&written, "# Written").unwrap();
    f.hook(None, "SessionEnd", json!({}));
    assert!(!f.record().exists());
    f.hook(None, "SessionStart", json!({"source": "resume"}));
    assert_eq!(documents(&f), [written]);
}

#[test]
fn subagent_transcripts_are_captured_even_without_their_hooks() {
    let f = Fixture::new();
    let transcript = f.0.join("session.jsonl");
    let document = f.project().join("from-subagent.md");
    fs::write(&document, "# Subagent").unwrap();
    age(&document, 600);
    fs::write(&transcript, "").unwrap();
    fs::create_dir_all(f.0.join("session/subagents")).unwrap();
    fs::write(
        f.0.join("session/subagents/agent-1.jsonl"),
        json!({"type":"assistant", "timestamp":"2026-09-25T00:00:00Z", "message":{"content":[
            {"type":"tool_use", "id":"sub-1", "name":"Write", "input":{"file_path":document}}
        ]}})
        .to_string(),
    )
    .unwrap();
    f.hook(None, "SessionStart", json!({"transcript_path": transcript}));
    f.hook(None, "Stop", json!({}));
    f.hook(None, "Stop", json!({}));
    assert_eq!(documents(&f), [document]);
    assert_eq!(store(&f).events(&f.session().activity_key()).unwrap().len(), 1);
}

#[test]
fn files_scanned_while_another_session_works_in_the_same_place_are_shared() {
    let f = Fixture::new();
    let other = f.0.join("elsewhere");
    fs::create_dir_all(&other).unwrap();
    f.hook(None, "SessionStart", json!({"session_id": "same-place"}));
    f.hook(None, "UserPromptSubmit", json!({"session_id": "same-place"}));
    f.hook(None, "SessionStart", json!({"session_id": "other-place", "cwd": other}));
    f.hook(None, "UserPromptSubmit", json!({"session_id": "other-place", "cwd": other}));
    f.hook(None, "SessionStart", json!({}));
    f.hook(None, "UserPromptSubmit", json!({}));
    fs::write(f.project().join("ambiguous.md"), "# Either session").unwrap();
    f.hook(None, "Stop", json!({}));
    let docs = discovery::documents_in(&f.session(), &store(&f));
    assert_eq!(docs.len(), 1);
    assert!(docs[0].scanned && docs[0].shared);
    let event = store(&f).events(&f.session().activity_key()).unwrap().remove(0);
    assert_eq!(
        event.concurrent,
        [session_activity::SessionKey { agent: Agent::Claude, session_id: "same-place".into() }]
    );
}

#[test]
fn a_session_whose_agent_exited_is_no_longer_live() {
    let f = Fixture::new();
    f.hook(None, "SessionStart", json!({}));
    let root = f.0.join("state");
    assert_eq!(crate::registry::load_all_in(&root).len(), 1); // Entries from before agent tracking.
    let mut record: Value = serde_json::from_str(&fs::read_to_string(f.record()).unwrap()).unwrap();
    record["terminal"]["agent"] = json!({"pid": std::process::id(), "started_at_us": 0});
    fs::write(f.record(), record.to_string()).unwrap();
    assert!(crate::registry::load_all_in(&root).is_empty()); // Same PID, different process.
    f.hook(None, "UserPromptSubmit", json!({})); // The fixture's hook records no agent, like a resume.
    assert_eq!(crate::registry::load_all_in(&root).len(), 1);
}
