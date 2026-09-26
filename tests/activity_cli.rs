use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

fn hook(root: &Path, input: Value) {
    let mut child = Command::new("bash")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/hooks/register.sh"))
        .arg("codex")
        .env("PEEKBACK_BIN", env!("CARGO_BIN_EXE_peekback"))
        .env("PEEKBACK_STATE_DIR", root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input.to_string().as_bytes()).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty(), "{}", String::from_utf8_lossy(&output.stderr));
}

fn query(root: &Path, command: &str) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_peekback"))
        .args(["activity", command, "--agent", "codex", "--session", "integration"])
        .env("PEEKBACK_STATE_DIR", root)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn all_sessions_listing_works_without_live_sessions_and_exposes_provenance() {
    let root = std::env::temp_dir().join(format!("peekback-all-cli-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let store = session_activity::Store::new(root.join("activity"));
    for agent in ["claude", "codex"] {
        let agent = session_activity::AgentId::new(agent).unwrap();
        let key = session_activity::SessionKey { agent, session_id: "same-id".into() };
        let event = session_activity::FileEvent::new(
            &key,
            &root,
            Path::new("shared.rs"),
            1,
            session_activity::Operation::Read,
            session_activity::Source::Hook,
        );
        store.append(&key, &[event]).unwrap();
    }
    fs::write(root.join("activity/claude/same-id/broken.json"), "{").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_peekback"))
        .args(["browse", "--all-sessions", "--list"])
        .env("PEEKBACK_STATE_DIR", &root)
        .env("CODEX_THREAD_ID", "unrelated-active-session")
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("All sessions"));
    assert_eq!(text.matches("shared.rs").count(), 1);
    assert!(text.contains("Claude Code / same-id") && text.contains("Codex / same-id"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("broken.json"));
    assert!(!root.join("daemon.log").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn wrapper_and_queries_retain_history_after_session_end() {
    let root = std::env::temp_dir().join(format!("peekback-cli-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let mut input =
        json!({"session_id":"integration", "cwd":root, "transcript_path":null, "hook_event_name":"SessionStart"});
    hook(&root, input.clone());
    input["hook_event_name"] = json!("PostToolUse");
    input["tool_name"] = json!("apply_patch");
    input["tool_input"] = json!({"command":"*** Begin Patch\n*** Delete File: removed.rs\n*** End Patch"});
    hook(&root, input.clone());
    input["hook_event_name"] = json!("SessionEnd");
    hook(&root, input);
    assert!(!root.join("sessions/integration.json").exists());
    let events = query(&root, "events");
    assert_eq!(events.as_array().unwrap().len(), 1);
    assert_eq!(events[0]["operation"], "delete");
    let files = query(&root, "files");
    assert_eq!(files.as_array().unwrap().len(), 1);
    assert_eq!(files[0]["exists"], false);
    fs::write(root.join("activity/codex/integration/corrupt.json"), "{").unwrap();
    let partial = Command::new(env!("CARGO_BIN_EXE_peekback"))
        .args(["activity", "events", "--agent", "codex", "--session", "integration"])
        .env("PEEKBACK_STATE_DIR", &root)
        .output()
        .unwrap();
    assert!(partial.status.success());
    assert!(String::from_utf8_lossy(&partial.stderr).contains("incomplete activity history"));
    assert_eq!(serde_json::from_slice::<Value>(&partial.stdout).unwrap().as_array().unwrap().len(), 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn maintenance_commands_preview_then_apply_and_pruned_sessions_stay_closed() {
    let root = std::env::temp_dir().join(format!("peekback-maintenance-cli-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let input = json!({"session_id":"integration", "cwd":root, "transcript_path":null,
        "hook_event_name":"PostToolUse", "tool_name":"apply_patch",
        "tool_input":{"command":"*** Begin Patch\n*** Add File: example.rs\n+text\n*** End Patch"}});
    hook(&root, input.clone());
    hook(&root, input.clone());
    let run = |args: &[&str]| {
        let output =
            Command::new(env!("CARGO_BIN_EXE_peekback")).args(args).env("PEEKBACK_STATE_DIR", &root).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        output
    };
    let preview = run(&["activity", "compact", "--agent", "codex", "--session", "integration"]);
    let report: Value = serde_json::from_slice(&preview.stdout).unwrap();
    assert_eq!(report["dry_run"], true);
    assert_eq!(report["batches_before"], 2);
    assert!(!root.join("activity/codex/integration/.checkpoint").exists());
    run(&["activity", "compact", "--agent", "codex", "--session", "integration", "--apply"]);
    assert_eq!(query(&root, "events").as_array().unwrap().len(), 2);
    // Retention keeps events at the cutoff second and rejects future cutoffs.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs().to_string();
    let future = (now.parse::<u64>().unwrap() * 1000).to_string();
    let retain = |before: &str, apply: bool| {
        let mut args = vec!["activity", "retain", "--agent", "codex", "--session", "integration", "--before", before];
        args.extend(apply.then_some("--apply"));
        Command::new(env!("CARGO_BIN_EXE_peekback")).args(args).env("PEEKBACK_STATE_DIR", &root).output().unwrap()
    };
    let rejected = retain(&future, true);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("retention cutoff is in the future"));
    assert!(retain(&now, false).status.success());
    assert_eq!(query(&root, "events").as_array().unwrap().len(), 2);
    assert!(retain(&now, true).status.success());
    assert!(query(&root, "events").as_array().unwrap().is_empty());
    let record = root.join("sessions/integration.json");
    let mut session: Value = serde_json::from_slice(&fs::read(&record).unwrap()).unwrap();
    session["last_active_at"] = json!(1);
    fs::write(&record, session.to_string()).unwrap();
    run(&["status", "--prune"]);
    assert!(!record.exists());
    hook(&root, input);
    assert!(!record.exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn terminal_listing_handles_live_and_retained_sessions_without_starting_viewer() {
    let root = std::env::temp_dir().join(format!("peekback-browser-cli-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("notes.md"), "# Notes").unwrap();
    let mut input = json!({"session_id":"integration", "cwd":root, "hook_event_name":"PostToolUse",
        "tool_name":"apply_patch", "tool_input":{"command":"*** Begin Patch\n*** Add File: notes.md\n+# Notes\n*** Delete File: old.rs\n*** End Patch"}});
    hook(&root, input.clone());
    let run = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_peekback"))
            .args(args)
            .env("PEEKBACK_STATE_DIR", &root)
            .env("CODEX_THREAD_ID", "integration")
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(!output.stdout.contains(&27), "piped output must not contain terminal escapes");
        output
    };
    let live = run(&["browse"]);
    let text = String::from_utf8(live.stdout).unwrap();
    assert!(text.contains("notes.md"));
    assert!(text.contains("old.rs"));
    assert!(text.contains("missing"));
    assert!(text.contains("tool"));
    input["hook_event_name"] = json!("SessionEnd");
    hook(&root, input);
    fs::write(root.join("activity/codex/integration/broken.json"), "{").unwrap();
    let ended = run(&["browse", "--agent", "codex", "--session", "integration", "--list"]);
    assert!(String::from_utf8_lossy(&ended.stdout).contains("notes.md"));
    let warnings = String::from_utf8_lossy(&ended.stderr);
    assert!(warnings.contains("broken.json"));
    assert!(!root.join("daemon.log").exists());
    let other = run(&["browse", "--agent", "claude", "--session", "integration"]);
    assert!(String::from_utf8_lossy(&other.stdout).contains("No observed files"));
    fs::remove_dir_all(root).unwrap();
}
