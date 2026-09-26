use std::fs;
use std::io::Write;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        // The clock alone is not unique: macOS reports whole microseconds, and
        // tests start in parallel threads of one process.
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let fixture = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("peekback-setup-{}-{fixture}-{nonce}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn setup(&self, binary: &Path, args: &[&str]) -> Output {
        Command::new(binary)
            .arg("setup")
            .args(args)
            .env("CLAUDE_CONFIG_DIR", self.0.join("claude"))
            .env("CODEX_HOME", self.0.join("codex"))
            .env("PATH", "")
            .output()
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn binary() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_peekback"))
}

fn success(output: Output) -> String {
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn setup_dry_run_does_not_create_directories_and_validates_all_inputs_first() {
    let f = Fixture::new();
    let text = success(f.setup(binary(), &["--all", "--dry-run"]));
    assert!(text.contains("would update"));
    assert_eq!(fs::read_dir(&f.0).unwrap().count(), 0);
    assert!(!f.setup(binary(), &[]).status.success());
    fs::create_dir(f.0.join("codex")).unwrap();
    fs::write(f.0.join("codex/hooks.json"), "broken").unwrap();
    assert!(!f.setup(binary(), &["--all"]).status.success());
    assert!(!f.0.join("claude").exists());
    assert_eq!(fs::read_to_string(f.0.join("codex/hooks.json")).unwrap(), "broken");
}

#[test]
fn setup_preserves_settings_is_repeatable_and_hooks_work_without_scripts_or_path() {
    let f = Fixture::new();
    let installed = f.0.join("peekback ' $dollar `name` (space)");
    fs::copy(binary(), &installed).unwrap();
    for (dir, filename) in [("claude", "settings.json"), ("codex", "hooks.json")] {
        fs::create_dir(f.0.join(dir)).unwrap();
        let original = br#"{"theme":"dark","hooks":{"SessionStart":[{"matcher":"custom","hooks":[{"type":"command","command":"echo user-hook"}]}],"PreToolUse":[]}}"#;
        let path = f.0.join(dir).join(filename);
        fs::write(&path, original).unwrap();
    }
    success(f.setup(&installed, &[])); // Both detected from custom config directories.
    for (agent, filename) in [("claude", "settings.json"), ("codex", "hooks.json")] {
        let path = f.0.join(agent).join(filename);
        let first = fs::read(&path).unwrap();
        let settings: Value = serde_json::from_slice(&first).unwrap();
        assert_eq!(settings["theme"], "dark");
        assert_eq!(settings["hooks"]["SessionStart"][0]["hooks"][0]["command"], "echo user-hook");
        assert_eq!(settings["hooks"]["PreToolUse"], json!([]));
        let backups: Vec<_> = fs::read_dir(f.0.join(agent))
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().contains("peekback-backup"))
            .collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(backups[0].metadata().unwrap().permissions().mode() & 0o777, 0o600);
        let backup: Value = serde_json::from_slice(&fs::read(backups[0].path()).unwrap()).unwrap();
        assert_eq!(backup["hooks"]["SessionStart"].as_array().unwrap().len(), 1);
        success(f.setup(&installed, &["--agent", agent]));
        assert_eq!(fs::read(&path).unwrap(), first);
        assert_eq!(fs::read_dir(f.0.join(agent)).unwrap().count(), 3); // config, backup, lock

        let mut input = json!({"session_id": format!("{agent}-session"), "cwd": f.0,
            "tool_name": if agent == "claude" { "Write" } else { "apply_patch" },
            "tool_input": {"file_path": "note.md", "command": "*** Begin Patch\n*** Add File: note.md\n+# Note\n*** End Patch"}});
        for event in ["SessionStart", "PostToolUse", "SessionEnd"] {
            input["hook_event_name"] = json!(event);
            let groups = settings["hooks"][event].as_array().unwrap();
            let command = groups.last().unwrap()["hooks"][0]["command"].as_str().unwrap();
            let mut child = Command::new("/bin/sh")
                .args(["-c", command])
                .env("PATH", "")
                .env("PEEKBACK_STATE_DIR", f.0.join("state"))
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            child.stdin.take().unwrap().write_all(input.to_string().as_bytes()).unwrap();
            let out = child.wait_with_output().unwrap();
            assert!(out.status.success());
            assert!(out.stdout.is_empty() && out.stderr.is_empty(), "{out:?}");
        }
        let out = Command::new(&installed)
            .args(["activity", "files", "--agent", agent, "--session", &format!("{agent}-session")])
            .env("PEEKBACK_STATE_DIR", f.0.join("state"))
            .output()
            .unwrap();
        let files: Value = serde_json::from_str(&success(out)).unwrap();
        assert_eq!(files.as_array().unwrap().len(), 1);
        assert!(!f.0.join(format!("state/sessions/{agent}-session.json")).exists());
    }
    // Moving the installation updates our commands and preserves user handlers.
    let moved = f.0.join("moved-peekback");
    fs::rename(&installed, &moved).unwrap();
    success(f.setup(&moved, &["--all"]));
    let settings: Value = serde_json::from_slice(&fs::read(f.0.join("codex/hooks.json")).unwrap()).unwrap();
    assert_eq!(settings["hooks"]["SessionStart"].as_array().unwrap().len(), 2);
    assert!(settings["hooks"]["SessionStart"][1]["hooks"][0]["command"].as_str().unwrap().contains("moved-peekback"));
    assert!(!f.0.join("state/daemon.log").exists());
}

#[test]
fn setup_refuses_symlinks_and_invalid_hook_shapes_without_overwriting_them() {
    let f = Fixture::new();
    fs::create_dir(f.0.join("claude")).unwrap();
    let path = f.0.join("claude/settings.json");
    for value in ["[]", "{\"hooks\":null}", "{\"hooks\":{\"SessionStart\":{}}}", "{\"hooks\":{\"Stop\":[{}]}}"] {
        fs::write(&path, value).unwrap();
        assert!(!f.setup(binary(), &["--agent", "claude"]).status.success());
        assert_eq!(fs::read_to_string(&path).unwrap(), value);
    }
    fs::remove_file(&path).unwrap();
    let target = f.0.join("linked-settings.json");
    fs::write(&target, "{}").unwrap();
    symlink(&target, &path).unwrap();
    assert!(!f.setup(binary(), &["--agent", "claude"]).status.success());
    assert!(path.is_symlink());
    assert_eq!(fs::read_to_string(target).unwrap(), "{}");
}
