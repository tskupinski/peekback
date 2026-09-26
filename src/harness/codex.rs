//! Codex CLI.

use std::path::Path;

use serde_json::Value;
use session_activity::{FileEvent, Operation, SessionKey, Source, resolve_path};

use super::ConfigFile;

pub const NAME: &str = "Codex";
pub const SLUG: &str = "codex";
pub const EXECUTABLES: &[&str] = &["codex"];
pub const SESSION_ENV: &[&str] = &["CODEX_THREAD_ID", "CODEX_SESSION_ID"];
pub const CONFIG: ConfigFile = ConfigFile {
    dir_env: "CODEX_HOME",
    default_dir: ".codex",
    file: "hooks.json",
    template: include_str!("../../hooks/codex-snippet.json"),
    after_setup: "Start or resume Codex, then use /hooks to review and trust the Peekback hooks.",
};

/// Codex edits files only through `apply_patch`; its headers name each file.
pub fn tool_events(
    session: &SessionKey,
    cwd: &Path,
    tool: &str,
    input: &Value,
    at: i64,
    source: Source,
) -> Vec<FileEvent> {
    if tool != "apply_patch" {
        return Vec::new();
    }
    patch_events(session, cwd, input["command"].as_str().unwrap_or_default(), at, source)
}

fn patch_events(session: &SessionKey, cwd: &Path, patch: &str, at: i64, source: Source) -> Vec<FileEvent> {
    let lines: Vec<_> = patch.trim().lines().collect();
    if lines.first() != Some(&"*** Begin Patch") || lines.last() != Some(&"*** End Patch") {
        return Vec::new();
    }
    let mut events: Vec<FileEvent> = Vec::new();
    for line in lines {
        if let Some(path) = line.strip_prefix("*** Move to: ") {
            if let Some(previous) = events.last_mut().filter(|e| e.operation == Operation::Modify && !path.is_empty()) {
                previous.previous_path = Some(previous.path.clone());
                previous.path = resolve_path(cwd, Path::new(path));
                previous.operation = Operation::Rename;
            }
            continue;
        }
        for (prefix, operation) in [
            ("*** Add File: ", Operation::Create),
            ("*** Update File: ", Operation::Modify),
            ("*** Delete File: ", Operation::Delete),
        ] {
            if let Some(path) = line.strip_prefix(prefix).filter(|p| !p.is_empty()) {
                events.push(FileEvent::new(session, cwd, Path::new(path), at, operation, source));
            }
        }
    }
    events
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::harness::Harness;

    #[test]
    fn patch_headers_become_creates_renames_and_deletes() {
        let patch = "*** Begin Patch\n*** Add File: ./src/main.rs\n+// code\n+*** Add File: fake.md\n*** Update File: old name.md\n*** Move to: new name.md\n@@\n-old\n+new\n*** Delete File: deleted.txt\n*** End Patch";
        let key = Harness::Codex.key("s-1");
        let events =
            tool_events(&key, Path::new("/project"), "apply_patch", &json!({"command": patch}), 42, Source::Hook);
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].path, Path::new("/project/src/main.rs"));
        assert_eq!(events[0].operation, Operation::Create);
        assert_eq!(events[1].operation, Operation::Rename);
        assert_eq!(events[1].previous_path.as_deref(), Some(Path::new("/project/old name.md")));
        assert_eq!(events[1].path, Path::new("/project/new name.md"));
        assert_eq!(events[2].operation, Operation::Delete);
        assert_eq!(session_activity::files(events).len(), 4);
    }

    #[test]
    fn only_a_complete_patch_is_read() {
        let key = Harness::Codex.key("s-1");
        let cwd = Path::new("/project");
        let unterminated = json!({"command": "*** Begin Patch\n*** Add File: a.md"});
        assert!(tool_events(&key, cwd, "apply_patch", &unterminated, 1, Source::Hook).is_empty());
        let shell = json!({"command": "*** Begin Patch\n*** Add File: a.md\n*** End Patch"});
        assert!(tool_events(&key, cwd, "shell", &shell, 1, Source::Hook).is_empty());
    }
}
