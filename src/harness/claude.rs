//! Claude Code.

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::Value;
use session_activity::{FileEvent, Operation, Outcome, SessionKey, Source};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use super::ConfigFile;
use crate::registry::Session;

pub const NAME: &str = "Claude Code";
pub const SLUG: &str = "claude";
pub const EXECUTABLES: &[&str] = &["claude"];
pub const SESSION_ENV: &[&str] = &["CLAUDE_CODE_SESSION_ID"];
pub const CONFIG: ConfigFile = ConfigFile {
    dir_env: "CLAUDE_CONFIG_DIR",
    default_dir: ".claude",
    file: "settings.json",
    template: include_str!("../../hooks/settings-snippet.json"),
    after_setup: "Start or resume Claude Code; review /hooks if prompted.",
};

/// Transcript tail read for the last agent activity; enough for many records
/// while keeping the read cheap on transcripts of any size.
const ACTIVITY_TAIL_BYTES: u64 = 512 * 1024;

/// `~/.claude/projects/<slug>/`, the directory holding the transcript.
fn project_dir(session: &Session) -> Option<&Path> {
    session.transcript_path.as_deref()?.parent()
}

pub fn memory_dir(session: &Session) -> Option<PathBuf> {
    project_dir(session).map(|dir| dir.join("memory"))
}

pub fn scratchpad_dir(session: &Session) -> Option<PathBuf> {
    let slug = project_dir(session)?.file_name()?;
    let uid = unsafe { libc::getuid() };
    Some(PathBuf::from(format!("/private/tmp/claude-{uid}")).join(slug).join(&session.session_id).join("scratchpad"))
}

pub fn tool_events(
    session: &SessionKey,
    cwd: &Path,
    tool: &str,
    input: &Value,
    at: i64,
    source: Source,
) -> Vec<FileEvent> {
    let operation = match tool {
        "Read" => Operation::Read,
        "Write" => Operation::Write, // Write can create or replace a file.
        "Edit" | "MultiEdit" => Operation::Modify,
        _ => return Vec::new(),
    };
    input["file_path"]
        .as_str()
        .filter(|p| !p.is_empty())
        .map(|p| vec![FileEvent::new(session, cwd, Path::new(p), at, operation, source)])
        .unwrap_or_default()
}

/// The main transcript and those of its subagents, which run without hooks.
pub fn transcript_events(key: &SessionKey, cwd: &Path, transcript: &Path) -> Vec<FileEvent> {
    std::iter::once(transcript.to_path_buf())
        .chain(subagent_transcripts(transcript))
        .flat_map(|transcript| transcript_file_events(key, cwd, &transcript))
        .collect()
}

/// Each subagent's transcript sits next to the main one, in
/// `<transcript stem>/subagents/`. Sorted for stable results.
fn subagent_transcripts(transcript: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(transcript.with_extension("").join("subagents")) else {
        return Vec::new();
    };
    let mut paths: Vec<_> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "jsonl") && path.is_file())
        .collect();
    paths.sort();
    paths
}

/// Tool requests matched to their results by call ID. Requests without results
/// have unknown outcomes. Best-effort: an unreadable transcript or malformed
/// records yield what could be read.
fn transcript_file_events(session: &SessionKey, cwd: &Path, transcript: &Path) -> Vec<FileEvent> {
    let Ok(file) = fs::File::open(transcript) else {
        return Vec::new();
    };
    let mut events = Vec::new();
    let mut results = HashMap::new();
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if !line.contains("\"tool_use\"") && !line.contains("\"tool_result\"") {
            continue;
        }
        let Ok(record) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if record["type"] == "user" {
            if let Some(content) = record["message"]["content"].as_array() {
                for item in content.iter().filter(|item| item["type"] == "tool_result") {
                    if let Some(id) = item["tool_use_id"].as_str() {
                        let result = match item.get("is_error") {
                            Some(Value::Bool(true)) => Outcome::Failed,
                            Some(Value::Bool(false)) | None => Outcome::Succeeded,
                            _ => Outcome::Unknown,
                        };
                        results
                            .entry(id.to_owned())
                            .and_modify(|old| {
                                if *old != result {
                                    *old = Outcome::Unknown;
                                }
                            })
                            .or_insert(result);
                    }
                }
            }
            continue;
        }
        if record["type"] != "assistant" {
            continue;
        }
        let at = record["timestamp"]
            .as_str()
            .and_then(|ts| OffsetDateTime::parse(ts, &Rfc3339).ok())
            .map(|t| t.unix_timestamp())
            .unwrap_or(0);
        let Some(content) = record["message"]["content"].as_array() else {
            continue;
        };
        let cwd = record["cwd"].as_str().map(Path::new).filter(|p| p.is_absolute()).unwrap_or(cwd);
        for item in content.iter().filter(|item| item["type"] == "tool_use") {
            let mut found = tool_events(
                session,
                cwd,
                item["name"].as_str().unwrap_or_default(),
                &item["input"],
                at,
                Source::Transcript,
            );
            for event in &mut found {
                event.tool_call_id = item["id"].as_str().map(str::to_owned);
            }
            events.extend(found);
        }
    }
    for event in &mut events {
        if let Some(result) = event.tool_call_id.as_ref().and_then(|id| results.get(id)) {
            event.outcome = *result;
        }
    }
    events
}

/// When the agent last did something, in Unix seconds: the newest assistant
/// message or tool result near the end of the transcript. A new prompt is
/// neither, so this still dates an interrupted turn once the next prompt has
/// been written. `None` when the tail holds no such record.
pub fn last_activity(transcript: &Path) -> Option<i64> {
    let mut file = fs::File::open(transcript).ok()?;
    let length = file.metadata().ok()?.len();
    let start = length.saturating_sub(ACTIVITY_TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut tail = Vec::new();
    file.read_to_end(&mut tail).ok()?;
    let text = String::from_utf8_lossy(&tail);
    // A tail that starts mid-file starts mid-record.
    let lines = text.lines().skip(usize::from(start > 0));
    lines
        .filter(|line| line.contains("\"assistant\"") || line.contains("\"tool_result\""))
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|record| {
            record["type"] == "assistant"
                || (record["type"] == "user"
                    && record["message"]["content"]
                        .as_array()
                        .is_some_and(|items| items.iter().any(|item| item["type"] == "tool_result")))
        })
        .filter_map(|record| {
            let at = OffsetDateTime::parse(record["timestamp"].as_str()?, &Rfc3339).ok()?;
            Some(at.unix_timestamp())
        })
        .max()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::harness::Harness;

    struct Temp(PathBuf);

    impl Temp {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!("peekback-claude-{name}-{}", std::process::id()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn key() -> SessionKey {
        Harness::Claude.key("s-1")
    }

    #[test]
    fn transcripts_resolve_relative_paths_and_keep_unknown_outcomes() {
        let temp = Temp::new("relative");
        let path = temp.0.join("transcript.jsonl");
        let record = json!({"type":"assistant", "timestamp":"2026-09-23T00:00:00Z", "message":{"content":[
            {"type":"tool_use", "id":"call-1", "name":"Write", "input":{"file_path":"src/main.rs"}}
        ]}});
        fs::write(&path, format!("not json\n{record}\n{{")).unwrap();
        let events = transcript_events(&key(), &temp.0, &path);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].path, temp.0.join("src/main.rs"));
        assert_eq!(events[0].outcome, Outcome::Unknown);
        assert_eq!(events[0].source, Source::Transcript);
    }

    #[test]
    fn transcript_results_resolve_outcomes_by_call_id_and_record_cwd() {
        let temp = Temp::new("results");
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
        let events = transcript_events(&key(), &temp.0, &path);
        assert_eq!(events[0].path, Path::new("/changed-directory/plan.md"));
        assert_eq!(events[0].outcome, Outcome::Failed);
        assert_eq!(events[1].outcome, Outcome::Succeeded);
        assert_eq!(events[2].outcome, Outcome::Unknown);
    }

    #[test]
    fn subagent_transcripts_sit_beside_the_main_transcript() {
        let temp = Temp::new("subagents");
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
    fn last_activity_is_the_newest_agent_record_and_ignores_prompts() {
        let temp = Temp::new("activity");
        let path = temp.0.join("transcript.jsonl");
        let record = |kind: &str, at: &str, content: Value| {
            json!({"type": kind, "timestamp": at, "message": {"content": content}}).to_string()
        };
        let lines = [
            record("assistant", "2026-09-25T10:00:00Z", json!([{"type":"text", "text":"working"}])),
            record("user", "2026-09-25T10:05:00Z", json!([{"type":"tool_result", "tool_use_id":"t1"}])),
            record("user", "2026-09-25T12:00:00Z", json!("the next prompt, hours later")),
            json!({"type":"attachment", "timestamp":"2026-09-25T12:00:01Z"}).to_string(),
        ];
        fs::write(&path, lines.join("\n")).unwrap();
        let expected = OffsetDateTime::parse("2026-09-25T10:05:00Z", &Rfc3339).unwrap().unix_timestamp();
        assert_eq!(last_activity(&path), Some(expected));
        let padded = format!("{}\n{}", "x".repeat(600 * 1024), lines.join("\n"));
        fs::write(&path, padded).unwrap();
        assert_eq!(last_activity(&path), Some(expected)); // Only the tail is read.
        assert_eq!(last_activity(&temp.0.join("missing.jsonl")), None);
    }
}
