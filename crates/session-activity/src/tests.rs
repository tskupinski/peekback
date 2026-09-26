use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

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

fn agent(id: &str) -> AgentId {
    AgentId::new(id).unwrap()
}

fn key(id: &str) -> SessionKey {
    SessionKey { agent: agent(id), session_id: "same-id".into() }
}

#[test]
fn all_history_keeps_namespaces_compaction_retention_and_partial_failures() {
    let temp = Temp::new();
    let store = Store::new(temp.0.join("history"));
    assert!(store.sessions().sessions.is_empty());
    assert!(store.read_all().events.is_empty());
    assert!(!temp.0.join("history").exists());
    for id in ["claude", "codex"] {
        let session = key(id);
        let event = FileEvent::new(&session, &temp.0, Path::new("shared.md"), 10, Operation::Write, Source::Hook);
        store.append(&session, &[event.clone()]).unwrap();
        store.append(&session, &[FileEvent { timestamp: 20, ..event }]).unwrap();
        store.maintain(&session, Some(15), true).unwrap();
    }
    assert_eq!(store.sessions().sessions, vec![key("claude"), key("codex")]);
    let report = store.read_all();
    assert!(report.warnings.is_empty());
    assert_eq!(report.events.len(), 2);
    let summaries = files(report.events);
    assert_eq!(summaries.len(), 1);
    assert_ne!(summaries[0].events[0].session.agent, summaries[0].events[1].session.agent);
    assert!(summaries[0].events.iter().all(|e| e.timestamp == 20));
    fs::write(store.directory(&key("claude")).unwrap().join(".checkpoint"), "broken").unwrap();
    let report = store.read_all();
    assert_eq!(report.events.len(), 1);
    assert_eq!(report.events[0].session.agent, agent("codex"));
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
    assert_eq!(report.sessions, vec![SessionKey { agent: agent("codex"), session_id: "valid".into() }]);
    assert_eq!(report.warnings.len(), 3);
}

#[test]
fn session_enumeration_lists_every_valid_agent_namespace() {
    let temp = Temp::new();
    let root = temp.0.join("history");
    fs::create_dir_all(root.join("future-agent/one")).unwrap();
    fs::create_dir_all(root.join("codex/two")).unwrap();
    fs::create_dir_all(root.join("Not An Agent/three")).unwrap();
    fs::write(root.join("stray.json"), "{}").unwrap();
    let report = Store::new(root).sessions();
    let listed: Vec<_> = report.sessions.iter().map(|k| (k.agent.as_str(), k.session_id.as_str())).collect();
    assert_eq!(listed, [("codex", "two"), ("future-agent", "one")]);
    assert_eq!(report.warnings.len(), 1);
    assert!(report.warnings[0].path.ends_with("Not An Agent"));
}

#[test]
fn agent_ids_are_safe_path_components_stored_as_plain_strings() {
    for invalid in ["", "Claude", "a/b", "..", "a b", "a_b"] {
        assert!(AgentId::new(invalid).is_err(), "{invalid}");
    }
    let key = SessionKey { agent: agent("codex"), session_id: "s".into() };
    let json = serde_json::to_string(&key).unwrap();
    assert_eq!(json, r#"{"agent":"codex","session_id":"s"}"#);
    assert_eq!(serde_json::from_str::<SessionKey>(&json).unwrap(), key);
    assert!(serde_json::from_str::<SessionKey>(r#"{"agent":"../x","session_id":"s"}"#).is_err());
}

/// One tool call that created, renamed and deleted files.
fn patch_event(cwd: &Path) -> Vec<FileEvent> {
    let session = key("codex");
    let mut events = vec![
        FileEvent::new(&session, cwd, Path::new("./src/main.rs"), 42, Operation::Create, Source::Hook),
        FileEvent::new(&session, cwd, Path::new("new name.md"), 42, Operation::Rename, Source::Hook),
        FileEvent::new(&session, cwd, Path::new("deleted.txt"), 42, Operation::Delete, Source::Hook),
    ];
    events[1].previous_path = Some(cwd.join("old name.md"));
    for event in &mut events {
        event.tool_call_id = Some("call-1".into());
        event.outcome = Outcome::Succeeded;
    }
    events
}

#[test]
fn concurrent_batches_survive_and_agent_namespaces_do_not_collide() {
    let temp = Temp::new();
    let store = Store::new(&temp.0);
    std::thread::scope(|scope| {
        for _ in 0..16 {
            let store = &store;
            scope.spawn(move || store.append(&key("codex"), &patch_event(Path::new("/project"))).unwrap());
        }
    });
    assert_eq!(store.events(&key("codex")).unwrap().len(), 48);
    assert!(store.events(&key("claude")).unwrap().is_empty());
    let mut events = patch_event(Path::new("/project"));
    for event in &mut events {
        event.session.agent = agent("claude");
    }
    store.append(&key("claude"), &events).unwrap();
    assert_eq!(store.events(&key("claude")).unwrap().len(), 3);
    let reopened = Store::new(&temp.0);
    assert_eq!(reopened.events(&key("codex")).unwrap().len(), 48);
}

#[test]
fn paths_and_schema_are_validated_and_partial_batches_ignored() {
    let temp = Temp::new();
    let store = Store::new(&temp.0);
    for id in ["", ".", "..", "../escape", "a/b"] {
        assert!(store.events(&SessionKey { agent: agent("codex"), session_id: id.into() }).is_err());
    }
    let mut events = patch_event(Path::new("/project"));
    events[0].schema_version = 99;
    assert!(store.append(&key("codex"), &events).is_err());
    events[0].schema_version = 1;
    store.append(&key("codex"), &events).unwrap();
    fs::write(temp.0.join("codex/same-id/interrupted.tmp"), "{").unwrap();
    assert_eq!(store.events(&key("codex")).unwrap().len(), 3);
    fs::write(temp.0.join("codex/same-id/corrupt.json"), "{").unwrap();
    assert!(store.events(&key("codex")).is_err());
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
    let events = scan(&key("claude"), &temp.0, &roots, 100);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].operation, Operation::Observed);
    assert_eq!(events[0].source, Source::ProjectScan);
    assert_eq!(events[0].outcome, Outcome::Unknown);
    assert!(scan(&key("claude"), &temp.0, &roots, 0).is_empty());
}

#[test]
fn damaged_oversized_and_invalid_batches_do_not_hide_healthy_history() {
    let temp = Temp::new();
    let store = Store::new(&temp.0);
    let session = key("codex");
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
fn scan_reports_budget_depth_and_io_limits_without_false_empty_results() {
    let temp = Temp::new();
    fs::write(temp.0.join("first.md"), "text").unwrap();
    let roots = [ScanRoot { path: temp.0.clone(), since: 0, until: i64::MAX, depth: 1, source: Source::ProjectScan }];
    let complete = scan_report(&key("codex"), &temp.0, &roots, 1);
    assert_eq!(complete.entries_visited, 1);
    assert!(!complete.budget_exhausted); // Exactly meeting the budget is not truncation.
    fs::create_dir(temp.0.join("nested")).unwrap();
    let budget = scan_report(&key("codex"), &temp.0, &roots, 1);
    assert!(budget.budget_exhausted);
    assert_eq!(budget.entries_visited, 1);
    let depth = scan_report(&key("codex"), &temp.0, &roots, 10);
    assert!(depth.depth_limited);
    let bad =
        [ScanRoot { path: temp.0.join("first.md"), since: 0, until: i64::MAX, depth: 1, source: Source::ProjectScan }];
    assert_eq!(scan_report(&key("codex"), &temp.0, &bad, 10).warnings.len(), 1);
    let empty = Temp::new();
    let roots = [ScanRoot { path: empty.0.clone(), since: 0, until: i64::MAX, depth: 1, source: Source::ProjectScan }];
    assert!(!scan_report(&key("codex"), &empty.0, &roots, 0).budget_exhausted);
}

#[test]
fn compaction_preview_and_commit_preserve_raw_events_and_future_appends() {
    let temp = Temp::new();
    let store = Store::new(&temp.0);
    let session = key("codex");
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
    let session = key("codex");
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
    let session = key("codex");
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
    let session = key("codex");
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
    let session = key("codex");
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
    let session = key("codex");
    store.append(&session, &patch_event(Path::new("/project"))).unwrap();
    std::thread::scope(|scope| {
        let store = &store;
        for _ in 0..8 {
            scope.spawn(move || {
                for _ in 0..3 {
                    store.append(&key("codex"), &patch_event(Path::new("/project"))).unwrap();
                }
            });
        }
        scope.spawn(move || {
            for _ in 0..3 {
                store.maintain(&key("codex"), None, true).unwrap();
            }
        });
        scope.spawn(move || {
            for _ in 0..5 {
                assert!(store.read(&key("codex")).unwrap().warnings.is_empty());
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
    let mut found: Vec<_> = scan(&key("claude"), &temp.0, &roots, 100)
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
    let mut found: Vec<_> = scan(&key("claude"), &temp.0, &roots, 100).into_iter().map(|e| e.timestamp).collect();
    found.sort();
    assert_eq!(found, [100, 200]);
}

#[test]
fn file_activity_tells_scan_only_and_shared_files_apart() {
    let session = key("claude");
    let observed =
        FileEvent::new(&session, Path::new("/p"), Path::new("a.md"), 1, Operation::Observed, Source::ProjectScan);
    let mut shared = observed.clone();
    shared.concurrent = vec![SessionKey { agent: agent("codex"), session_id: "other".into() }];
    let written = FileEvent::new(&session, Path::new("/p"), Path::new("a.md"), 2, Operation::Write, Source::Hook);
    let file = |events: Vec<FileEvent>| files(events).remove(0);
    assert!(file(vec![observed.clone()]).scan_only());
    assert!(!file(vec![observed.clone()]).possibly_shared());
    assert!(file(vec![observed.clone(), shared.clone()]).possibly_shared());
    assert!(!file(vec![shared, written]).possibly_shared()); // A tool reported writing it.
}

#[test]
fn append_new_keeps_one_copy_across_schemas_and_concurrent_annotations() {
    let temp = Temp::new();
    let store = Store::new(&temp.0);
    let session = key("codex");
    let mut old = patch_event(Path::new("/project"));
    for event in &mut old {
        event.schema_version = 1;
    }
    store.append(&session, &old).unwrap();
    let mut again = patch_event(Path::new("/project"));
    again[0].concurrent = vec![SessionKey { agent: agent("claude"), session_id: "other".into() }];
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
