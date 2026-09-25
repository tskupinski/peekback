use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;

use crate::*;

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "session-activity-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn key(agent: Agent) -> SessionKey {
    SessionKey { agent, session_id: "same-id".into() }
}

#[test]
fn all_history_keeps_namespaces_compaction_retention_and_partial_failures() {
    let temp = Temp::new();
    let store = Store::new(temp.0.join("history"));
    assert!(store.sessions().sessions.is_empty());
    assert!(store.read_all().events.is_empty());
    assert!(!temp.0.join("history").exists());
    for agent in [Agent::Claude, Agent::Codex] {
        let session = key(agent);
        let event = FileEvent::new(&session, &temp.0, Path::new("shared.md"), 10, Operation::Write, Source::Hook);
        store.append(&session, &[event.clone()]).unwrap();
        store.append(&session, &[FileEvent { timestamp: 20, ..event }]).unwrap();
        store.maintain(&session, Some(15), true).unwrap();
    }
    assert_eq!(store.sessions().sessions, vec![key(Agent::Claude), key(Agent::Codex)]);
    let report = store.read_all();
    assert!(report.warnings.is_empty());
    assert_eq!(report.events.len(), 2);
    let summaries = files(report.events);
    assert_eq!(summaries.len(), 1);
    assert_ne!(summaries[0].events[0].session.agent, summaries[0].events[1].session.agent);
    assert!(summaries[0].events.iter().all(|e| e.timestamp == 20));
    fs::write(store.directory(&key(Agent::Claude)).unwrap().join(".checkpoint"), "broken").unwrap();
    let report = store.read_all();
    assert_eq!(report.events.len(), 1);
    assert_eq!(report.events[0].session.agent, Agent::Codex);
    assert_eq!(report.warnings.len(), 1);
    assert!(report.warnings[0].path.ends_with("claude/same-id"));
}

#[cfg(unix)]
#[test]
fn session_enumeration_ignores_symlinks_and_reports_invalid_directories() {
    use std::os::unix::fs::symlink;
    let temp = Temp::new();
    let root = temp.0.join("history");
    fs::create_dir_all(root.join("codex/valid")).unwrap();
    fs::create_dir(root.join("codex/invalid name")).unwrap();
    fs::write(root.join("codex/stray.json"), "{}").unwrap();
    symlink(root.join("codex"), root.join("claude")).unwrap();
    symlink(root.join("codex/valid"), root.join("codex/linked")).unwrap();
    let report = Store::new(root).sessions();
    assert_eq!(report.sessions, vec![SessionKey { agent: Agent::Codex, session_id: "valid".into() }]);
    assert_eq!(report.warnings.len(), 3);
}

fn patch_event(cwd: &Path) -> Vec<FileEvent> {
    hook_events(Agent::Codex, &json!({
        "session_id": "same-id", "cwd": cwd, "hook_event_name": "PostToolUse",
        "tool_name": "apply_patch", "tool_use_id": "call-1",
        "tool_input": {"command": "*** Begin Patch\n*** Add File: ./src/main.rs\n+// code\n+*** Add File: fake.md\n*** Update File: old name.md\n*** Move to: new name.md\n@@\n-old\n+new\n*** Delete File: deleted.txt\n*** End Patch"},
        "tool_response": {"success": true}
    }), 42).unwrap()
}

#[test]
fn normalizes_all_file_types_renames_deletes_and_outcomes() {
    let events = patch_event(Path::new("/project"));
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].path, Path::new("/project/src/main.rs"));
    assert_eq!(events[0].operation, Operation::Create);
    assert_eq!(events[1].operation, Operation::Rename);
    assert_eq!(events[1].previous_path.as_deref(), Some(Path::new("/project/old name.md")));
    assert_eq!(events[1].path, Path::new("/project/new name.md"));
    assert_eq!(events[2].operation, Operation::Delete);
    assert!(events.iter().all(|e| e.outcome == Outcome::Succeeded && e.tool_call_id.as_deref() == Some("call-1")));
    assert_eq!(files(events).len(), 4);
}

#[test]
fn failed_and_unknown_results_remain_distinct() {
    let mut input = json!({"session_id":"same-id", "cwd":"/project", "hook_event_name":"PostToolUse",
        "tool_name":"Edit", "tool_input":{"file_path":"src/main.rs"}, "tool_response":{"is_error":true}});
    assert_eq!(hook_events(Agent::Claude, &input, 1).unwrap()[0].outcome, Outcome::Failed);
    input["tool_response"] = json!("unrecognized result text");
    assert_eq!(hook_events(Agent::Claude, &input, 1).unwrap()[0].outcome, Outcome::Unknown);
    input["tool_name"] = json!("Read");
    assert_eq!(hook_events(Agent::Claude, &input, 1).unwrap()[0].operation, Operation::Read);
}

#[test]
fn concurrent_batches_survive_and_agent_namespaces_do_not_collide() {
    let temp = Temp::new();
    let store = Store::new(&temp.0);
    std::thread::scope(|scope| {
        for _ in 0..16 {
            let store = &store;
            scope.spawn(move || store.append(&key(Agent::Codex), &patch_event(Path::new("/project"))).unwrap());
        }
    });
    assert_eq!(store.events(&key(Agent::Codex)).unwrap().len(), 48);
    assert!(store.events(&key(Agent::Claude)).unwrap().is_empty());
    let mut events = patch_event(Path::new("/project"));
    for event in &mut events {
        event.session.agent = Agent::Claude;
    }
    store.append(&key(Agent::Claude), &events).unwrap();
    assert_eq!(store.events(&key(Agent::Claude)).unwrap().len(), 3);
    let reopened = Store::new(&temp.0);
    assert_eq!(reopened.events(&key(Agent::Codex)).unwrap().len(), 48);
}

#[test]
fn paths_and_schema_are_validated_and_partial_batches_ignored() {
    let temp = Temp::new();
    let store = Store::new(&temp.0);
    for id in ["", ".", "..", "../escape", "a/b"] {
        assert!(store.events(&SessionKey { agent: Agent::Codex, session_id: id.into() }).is_err());
    }
    let mut events = patch_event(Path::new("/project"));
    events[0].schema_version = 99;
    assert!(store.append(&key(Agent::Codex), &events).is_err());
    events[0].schema_version = 1;
    store.append(&key(Agent::Codex), &events).unwrap();
    fs::write(temp.0.join("codex/same-id/interrupted.tmp"), "{").unwrap();
    assert_eq!(store.events(&key(Agent::Codex)).unwrap().len(), 3);
    fs::write(temp.0.join("codex/same-id/corrupt.json"), "{").unwrap();
    assert!(store.events(&key(Agent::Codex)).is_err());
}

#[test]
fn scans_are_separate_evidence_and_respect_depth_and_budget() {
    let temp = Temp::new();
    fs::create_dir_all(temp.0.join(".hidden")).unwrap();
    fs::create_dir_all(temp.0.join("target")).unwrap();
    fs::create_dir_all(temp.0.join("nested")).unwrap();
    for path in ["main.rs", ".hidden/secret.md", "target/output.md", "nested/deep.md"] {
        fs::write(temp.0.join(path), "text").unwrap();
    }
    let roots = [ScanRoot { path: temp.0.clone(), since: 0, until: i64::MAX, depth: 1, source: Source::ProjectScan }];
    let events = scan(&key(Agent::Claude), &temp.0, &roots, 100);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].operation, Operation::Observed);
    assert_eq!(events[0].source, Source::ProjectScan);
    assert_eq!(events[0].outcome, Outcome::Unknown);
    assert!(scan(&key(Agent::Claude), &temp.0, &roots, 0).is_empty());
}

#[test]
fn claude_transcripts_resolve_relative_paths_and_keep_unknown_outcomes() {
    let temp = Temp::new();
    let path = temp.0.join("transcript.jsonl");
    let record = json!({"type":"assistant", "timestamp":"2026-09-23T00:00:00Z", "message":{"content":[
        {"type":"tool_use", "id":"call-1", "name":"Write", "input":{"file_path":"src/main.rs"}}
    ]}});
    fs::write(&path, format!("not json\n{record}\n{{")).unwrap();
    let events = transcript_events(&key(Agent::Claude), &temp.0, &path);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].path, temp.0.join("src/main.rs"));
    assert_eq!(events[0].outcome, Outcome::Unknown);
    assert_eq!(events[0].source, Source::Transcript);
    assert!(transcript_events(&key(Agent::Codex), &temp.0, &path).is_empty());
}

#[test]
fn damaged_oversized_and_invalid_batches_do_not_hide_healthy_history() {
    let temp = Temp::new();
    let store = Store::new(&temp.0);
    let session = key(Agent::Codex);
    store.append(&session, &patch_event(Path::new("/project"))).unwrap();
    let dir = temp.0.join("codex/same-id");
    fs::write(dir.join("damaged.json"), "{").unwrap();
    let mut invalid = patch_event(Path::new("/project"));
    invalid[1].previous_path = Some("relative.md".into());
    assert!(store.append(&session, &invalid).is_err());
    fs::write(dir.join("invalid.json"), serde_json::to_vec(&invalid).unwrap()).unwrap();
    invalid[1].previous_path = Some("/absolute.md".into());
    invalid[0].schema_version = 99;
    fs::write(dir.join("future.json"), serde_json::to_vec(&invalid).unwrap()).unwrap();
    fs::File::create(dir.join("oversized.json")).unwrap().set_len(16 * 1024 * 1024 + 1).unwrap();
    fs::write(dir.join("unfinished.tmp"), "{").unwrap();
    let report = store.read(&session).unwrap();
    assert_eq!(report.events.len(), 3);
    assert_eq!(report.warnings.len(), 4);
    assert!(store.events(&session).is_err());
    assert!(dir.join("damaged.json").exists(), "damaged data must remain available for repair");
}

#[test]
fn retries_are_collapsed_and_outcomes_reconciled_without_merging_distinct_calls() {
    let original = patch_event(Path::new("/project")).remove(0);
    let mut retry = original.clone();
    retry.timestamp += 10;
    let mut transcript = original.clone();
    transcript.source = Source::Transcript;
    transcript.outcome = Outcome::Unknown;
    let mut failed = original.clone();
    failed.outcome = Outcome::Failed;
    let resolved = reconcile([transcript.clone(), failed.clone(), failed.clone()]);
    assert_eq!(resolved.len(), 2); // Hook and transcript evidence are both retained.
    assert!(resolved.iter().all(|e| e.outcome == Outcome::Failed));
    assert_eq!(files([original.clone(), retry])[0].events.len(), 1);
    let mut other_call = original.clone();
    other_call.tool_call_id = Some("different-call".into());
    assert_eq!(reconcile([original.clone(), other_call]).len(), 2);
    let conflict = reconcile([original.clone(), failed, transcript]);
    assert_eq!(conflict.last().unwrap().outcome, Outcome::Unknown);
    let mut unkeyed = original;
    unkeyed.tool_call_id = None;
    assert_eq!(reconcile([unkeyed.clone(), unkeyed]).len(), 2);
}

#[test]
fn transcript_results_resolve_outcomes_by_call_id_and_record_cwd() {
    let temp = Temp::new();
    let path = temp.0.join("transcript.jsonl");
    let request = json!({"type":"assistant", "cwd":"/changed-directory", "message":{"content":[
        {"type":"tool_use", "id":"failed", "name":"Edit", "input":{"file_path":"plan.md"}},
        {"type":"tool_use", "id":"ok", "name":"Write", "input":{"file_path":"other.md"}},
        {"type":"tool_use", "id":"pending", "name":"Read", "input":{"file_path":"third.md"}}
    ]}});
    let response = json!({"type":"user", "message":{"content":[
        {"type":"tool_result", "tool_use_id":"ok", "content":"done"},
        {"type":"tool_result", "tool_use_id":"failed", "is_error":true, "content":"failed"}
    ]}});
    fs::write(&path, format!("{request}\n{{\n{response}\n")).unwrap();
    let events = transcript_events(&key(Agent::Claude), &temp.0, &path);
    assert_eq!(events[0].path, Path::new("/changed-directory/plan.md"));
    assert_eq!(events[0].outcome, Outcome::Failed);
    assert_eq!(events[1].outcome, Outcome::Succeeded);
    assert_eq!(events[2].outcome, Outcome::Unknown);
}

#[test]
fn explicit_failure_hook_and_conflicting_response_flags_report_failure() {
    let mut input = json!({"session_id":"same-id", "cwd":"/project", "hook_event_name":"PostToolUseFailure",
        "tool_name":"Edit", "tool_input":{"file_path":"plan.md"}, "tool_use_id":"call-1"});
    assert_eq!(hook_events(Agent::Claude, &input, 1).unwrap()[0].outcome, Outcome::Failed);
    input["hook_event_name"] = json!("PostToolUse");
    input["tool_response"] = json!({"is_error":false, "success":false});
    assert_eq!(hook_events(Agent::Claude, &input, 1).unwrap()[0].outcome, Outcome::Failed);
}

#[test]
fn scan_reports_budget_depth_and_io_limits_without_false_empty_results() {
    let temp = Temp::new();
    fs::write(temp.0.join("first.md"), "text").unwrap();
    let roots = [ScanRoot { path: temp.0.clone(), since: 0, until: i64::MAX, depth: 1, source: Source::ProjectScan }];
    let complete = scan_report(&key(Agent::Codex), &temp.0, &roots, 1);
    assert_eq!(complete.entries_visited, 1);
    assert!(!complete.budget_exhausted); // Exactly meeting the budget is not truncation.
    fs::create_dir(temp.0.join("nested")).unwrap();
    let budget = scan_report(&key(Agent::Codex), &temp.0, &roots, 1);
    assert!(budget.budget_exhausted);
    assert_eq!(budget.entries_visited, 1);
    let depth = scan_report(&key(Agent::Codex), &temp.0, &roots, 10);
    assert!(depth.depth_limited);
    let bad =
        [ScanRoot { path: temp.0.join("first.md"), since: 0, until: i64::MAX, depth: 1, source: Source::ProjectScan }];
    assert_eq!(scan_report(&key(Agent::Codex), &temp.0, &bad, 10).warnings.len(), 1);
    let empty = Temp::new();
    let roots = [ScanRoot { path: empty.0.clone(), since: 0, until: i64::MAX, depth: 1, source: Source::ProjectScan }];
    assert!(!scan_report(&key(Agent::Codex), &empty.0, &roots, 0).budget_exhausted);
}

#[test]
fn compaction_preview_and_commit_preserve_raw_events_and_future_appends() {
    let temp = Temp::new();
    let store = Store::new(&temp.0);
    let session = key(Agent::Codex);
    for time in 0..8 {
        let mut events = patch_event(Path::new("/project"));
        for event in &mut events {
            event.timestamp = time;
        }
        store.append(&session, &events).unwrap();
    }
    let before = store.events(&session).unwrap();
    let preview = store.maintain(&session, None, false).unwrap();
    assert!(preview.dry_run);
    assert_eq!(preview.batches_before, 8);
    assert_eq!(preview.batches_after, 1);
    assert!(!temp.0.join("codex/same-id/.checkpoint").exists());
    let committed = store.maintain(&session, None, true).unwrap();
    assert_eq!(committed.events_removed, 0);
    assert_eq!(store.events(&session).unwrap(), before);
    store.append(&session, &patch_event(Path::new("/project"))).unwrap();
    assert_eq!(store.events(&session).unwrap().len(), before.len() + 3);
    let again = store.maintain(&session, None, true).unwrap();
    assert_eq!(again.batches_before, 2);
    assert_eq!(again.batches_after, 1);
}

#[test]
fn retention_is_explicit_inclusive_and_prevents_old_imports() {
    let temp = Temp::new();
    let store = Store::new(&temp.0);
    let session = key(Agent::Codex);
    let mut events = patch_event(Path::new("/project"));
    for (index, event) in events.iter_mut().enumerate() {
        event.timestamp = index as i64;
    }
    store.append(&session, &events).unwrap();
    let preview = store.maintain(&session, Some(1), false).unwrap();
    assert_eq!(preview.events_removed, 1);
    assert_eq!(store.events(&session).unwrap().len(), 3);
    store.maintain(&session, Some(1), true).unwrap();
    assert_eq!(store.events(&session).unwrap().len(), 2);
    store.append(&session, &events[..1]).unwrap();
    assert_eq!(store.events(&session).unwrap().len(), 2);
    store.maintain(&session, None, true).unwrap();
    assert_eq!(store.retained_from(&session).unwrap(), Some(1));
    store.maintain(&session, Some(3), true).unwrap();
    assert!(store.events(&session).unwrap().is_empty());
    store.append(&session, &patch_event(Path::new("/project"))).unwrap();
    assert_eq!(store.events(&session).unwrap().len(), 3);
}

#[test]
fn future_cutoffs_are_rejected_so_recording_cannot_stop_for_good() {
    let temp = Temp::new();
    let store = Store::new(&temp.0);
    let session = key(Agent::Codex);
    store.append(&session, &patch_event(Path::new("/project"))).unwrap();
    let milliseconds = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64;
    for apply in [false, true] {
        let error = store.maintain(&session, Some(milliseconds), apply).unwrap_err();
        assert!(error.to_string().contains("future"), "{error}");
    }
    assert_eq!(store.retained_from(&session).unwrap(), None);
    store.append(&session, &patch_event(Path::new("/project"))).unwrap();
    assert_eq!(store.events(&session).unwrap().len(), 6);
}

#[test]
fn unexpected_history_files_stop_preview_and_apply_alike() {
    let temp = Temp::new();
    let store = Store::new(&temp.0);
    let session = key(Agent::Codex);
    store.append(&session, &patch_event(Path::new("/project"))).unwrap();
    let dir = temp.0.join("codex/same-id");
    let batch = fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path()).next().unwrap();
    #[allow(unused_mut)]
    let mut copies = vec![dir.join("notes (1).json")];
    #[cfg(target_os = "linux")] // APFS refuses names that are not UTF-8.
    copies.push(dir.join(<std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(b"copy-\xff.json")));
    for copy in copies {
        fs::copy(&batch, &copy).unwrap();
        for apply in [false, true] {
            let error = store.maintain(&session, None, apply).unwrap_err();
            assert!(error.to_string().contains("unexpected file"), "{error}");
        }
        assert!(!dir.join(".checkpoint").exists());
        fs::remove_file(copy).unwrap();
    }
    store.maintain(&session, None, true).unwrap();
    assert_eq!(store.events(&session).unwrap().len(), 3);
}

#[test]
fn checkpoint_survives_interrupted_cleanup_and_ignores_uncommitted_packs() {
    let temp = Temp::new();
    let store = Store::new(&temp.0);
    let session = key(Agent::Codex);
    store.append(&session, &patch_event(Path::new("/project"))).unwrap();
    let dir = temp.0.join("codex/same-id");
    let source = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|e| e == "json"))
        .unwrap();
    let original = fs::read(&source).unwrap();
    fs::write(dir.join("aborted.pack"), &original).unwrap();
    assert_eq!(store.events(&session).unwrap().len(), 3);
    store.maintain(&session, None, true).unwrap();
    assert!(!dir.join("aborted.pack").exists());
    // Simulate a durable checkpoint followed by a crash before source cleanup.
    fs::write(&source, original).unwrap();
    assert_eq!(store.events(&session).unwrap().len(), 3);
    store.maintain(&session, None, true).unwrap();
    assert!(!source.exists());
    assert_eq!(store.events(&session).unwrap().len(), 3);
    fs::write(dir.join("corrupt.json"), "{").unwrap();
    let checkpoint = fs::read(dir.join(".checkpoint")).unwrap();
    assert!(store.maintain(&session, Some(0), true).is_err());
    assert_eq!(fs::read(dir.join(".checkpoint")).unwrap(), checkpoint);
}

#[test]
fn concurrent_appends_and_compaction_do_not_lose_history() {
    let temp = Temp::new();
    let store = Store::new(&temp.0);
    let session = key(Agent::Codex);
    store.append(&session, &patch_event(Path::new("/project"))).unwrap();
    std::thread::scope(|scope| {
        let store = &store;
        for _ in 0..8 {
            scope.spawn(move || {
                for _ in 0..3 {
                    store.append(&key(Agent::Codex), &patch_event(Path::new("/project"))).unwrap();
                }
            });
        }
        scope.spawn(move || {
            for _ in 0..3 {
                store.maintain(&key(Agent::Codex), None, true).unwrap();
            }
        });
        scope.spawn(move || {
            for _ in 0..5 {
                assert!(store.read(&key(Agent::Codex)).unwrap().warnings.is_empty());
            }
        });
    });
    assert_eq!(store.events(&session).unwrap().len(), 75);
}

#[test]
fn scans_skip_other_checkouts_but_keep_submodules_and_the_root_checkout() {
    let temp = Temp::new();
    fs::create_dir_all(temp.0.join(".git")).unwrap();
    for dir in ["worktrees/pr-1", "vendor/clone/.git", "docs"] {
        fs::create_dir_all(temp.0.join(dir)).unwrap();
    }
    fs::write(temp.0.join("worktrees/pr-1/.git"), "gitdir: /repo/.git/worktrees/pr-1\n").unwrap();
    fs::write(temp.0.join("docs/.git"), "gitdir: ../.git/modules/docs\n").unwrap();
    for path in ["plan.md", "docs/design.md", "worktrees/pr-1/other.md", "vendor/clone/readme.md"] {
        fs::write(temp.0.join(path), "text").unwrap();
    }
    let roots = [ScanRoot { path: temp.0.clone(), since: 0, until: i64::MAX, depth: 4, source: Source::ProjectScan }];
    let mut found: Vec<_> = scan(&key(Agent::Claude), &temp.0, &roots, 100)
        .into_iter()
        .map(|e| e.path)
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .collect();
    found.sort();
    assert_eq!(found, [temp.0.join("docs/design.md"), temp.0.join("plan.md")]);
}

#[test]
fn scans_accept_only_modification_times_inside_the_window() {
    let temp = Temp::new();
    for (name, secs) in [("before.md", 99), ("first.md", 100), ("last.md", 200), ("after.md", 201)] {
        let path = temp.0.join(name);
        fs::write(&path, "text").unwrap();
        let at = std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs);
        fs::File::options().write(true).open(&path).unwrap().set_modified(at).unwrap();
    }
    let roots = [ScanRoot { path: temp.0.clone(), since: 100, until: 200, depth: 1, source: Source::ProjectScan }];
    let mut found: Vec<_> = scan(&key(Agent::Claude), &temp.0, &roots, 100).into_iter().map(|e| e.timestamp).collect();
    found.sort();
    assert_eq!(found, [100, 200]);
}

#[test]
fn last_activity_is_the_newest_agent_record_and_ignores_prompts() {
    let temp = Temp::new();
    let path = temp.0.join("transcript.jsonl");
    let record = |kind: &str, at: &str, content: serde_json::Value| {
        json!({"type": kind, "timestamp": at, "message": {"content": content}}).to_string()
    };
    let lines = [
        record("assistant", "2026-09-25T10:00:00Z", json!([{"type":"text", "text":"working"}])),
        record("user", "2026-09-25T10:05:00Z", json!([{"type":"tool_result", "tool_use_id":"t1"}])),
        record("user", "2026-09-25T12:00:00Z", json!("the next prompt, hours later")),
        json!({"type":"attachment", "timestamp":"2026-09-25T12:00:01Z"}).to_string(),
    ];
    fs::write(&path, lines.join("\n")).unwrap();
    let expected = time::OffsetDateTime::parse("2026-09-25T10:05:00Z", &time::format_description::well_known::Rfc3339)
        .unwrap()
        .unix_timestamp();
    assert_eq!(transcript_last_activity(&path), Some(expected));
    let padded = format!("{}\n{}", "x".repeat(600 * 1024), lines.join("\n"));
    fs::write(&path, padded).unwrap();
    assert_eq!(transcript_last_activity(&path), Some(expected)); // Only the tail is read.
    assert_eq!(transcript_last_activity(&temp.0.join("missing.jsonl")), None);
}

#[test]
fn file_activity_tells_scan_only_and_shared_files_apart() {
    let session = key(Agent::Claude);
    let observed =
        FileEvent::new(&session, Path::new("/p"), Path::new("a.md"), 1, Operation::Observed, Source::ProjectScan);
    let mut shared = observed.clone();
    shared.concurrent = vec![SessionKey { agent: Agent::Codex, session_id: "other".into() }];
    let written = FileEvent::new(&session, Path::new("/p"), Path::new("a.md"), 2, Operation::Write, Source::Hook);
    let file = |events: Vec<FileEvent>| files(events).remove(0);
    assert!(file(vec![observed.clone()]).scan_only());
    assert!(!file(vec![observed.clone()]).possibly_shared());
    assert!(file(vec![observed.clone(), shared.clone()]).possibly_shared());
    assert!(!file(vec![shared, written]).possibly_shared()); // A tool reported writing it.
}

#[test]
fn subagent_transcripts_sit_beside_the_main_transcript() {
    let temp = Temp::new();
    let main = temp.0.join("session.jsonl");
    fs::write(&main, "").unwrap();
    assert!(subagent_transcripts(&main).is_empty());
    let dir = temp.0.join("session/subagents");
    fs::create_dir_all(dir.join("nested.jsonl")).unwrap();
    for name in ["agent-b.jsonl", "agent-a.jsonl", "agent-a.meta.json"] {
        fs::write(dir.join(name), "").unwrap();
    }
    assert_eq!(subagent_transcripts(&main), [dir.join("agent-a.jsonl"), dir.join("agent-b.jsonl")]);
}

#[test]
fn append_new_keeps_one_copy_across_schemas_and_concurrent_annotations() {
    let temp = Temp::new();
    let store = Store::new(&temp.0);
    let session = key(Agent::Codex);
    let mut old = patch_event(Path::new("/project"));
    for event in &mut old {
        event.schema_version = 1;
    }
    store.append(&session, &old).unwrap();
    let mut again = patch_event(Path::new("/project"));
    again[0].concurrent = vec![SessionKey { agent: Agent::Claude, session_id: "other".into() }];
    let mut later = again[0].clone();
    later.timestamp += 1;
    again.push(later.clone());
    assert_eq!(store.append_new(&session, &again).unwrap(), 1);
    assert_eq!(store.append_new(&session, &again).unwrap(), 0);
    let events = store.events(&session).unwrap();
    assert_eq!(events.len(), old.len() + 1);
    assert_eq!(events.last().unwrap().concurrent, later.concurrent);
    assert!(events.iter().filter(|e| e.schema_version == 1).all(|e| e.concurrent.is_empty()));
}
