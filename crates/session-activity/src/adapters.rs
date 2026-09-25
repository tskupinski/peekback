use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{Result, ensure};
use serde_json::Value;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::{Agent, FileEvent, Operation, Outcome, SessionKey, Source, resolve_path};

/// Normalize supported PostToolUse payloads. Other tools (including arbitrary
/// shell commands) yield no file events. The caller records lifecycle metadata.
pub fn hook_events(agent: Agent, input: &Value, timestamp: i64) -> Result<Vec<FileEvent>> {
    let session = SessionKey { agent, session_id: input["session_id"].as_str().unwrap_or_default().into() };
    session.validate()?;
    let cwd = Path::new(input["cwd"].as_str().unwrap_or_default());
    ensure!(cwd.is_absolute(), "hook cwd must be absolute");
    let failed = agent == Agent::Claude && input["hook_event_name"] == "PostToolUseFailure";
    if input["hook_event_name"] != "PostToolUse" && !failed {
        return Ok(Vec::new());
    }
    let mut events = tool_events(
        &session,
        cwd,
        input["tool_name"].as_str().unwrap_or_default(),
        &input["tool_input"],
        timestamp,
        Source::Hook,
    );
    for event in &mut events {
        event.tool_call_id = input["tool_use_id"].as_str().map(str::to_owned);
        event.outcome = if failed { Outcome::Failed } else { outcome(&input["tool_response"]) };
    }
    Ok(events)
}

fn outcome(response: &Value) -> Outcome {
    let flags = [
        response["is_error"].as_bool().map(|v| !v),
        response["isError"].as_bool().map(|v| !v),
        response["success"].as_bool(),
    ];
    if flags.contains(&Some(false)) {
        return Outcome::Failed;
    }
    if flags.contains(&Some(true)) {
        return Outcome::Succeeded;
    }
    // Output formats vary between agent versions; absence of an explicit
    // success indicator is not proof that an attempted edit succeeded.
    Outcome::Unknown
}

fn tool_events(session: &SessionKey, cwd: &Path, tool: &str, input: &Value, at: i64, source: Source) -> Vec<FileEvent> {
    if session.agent == Agent::Codex && tool == "apply_patch" {
        return patch_events(session, cwd, input["command"].as_str().unwrap_or_default(), at, source);
    }
    if session.agent != Agent::Claude {
        return Vec::new();
    }
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

/// Claude keeps each subagent's transcript next to the main one, in
/// `<transcript stem>/subagents/`. Sorted for stable results.
pub fn subagent_transcripts(transcript: &Path) -> Vec<PathBuf> {
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

/// Transcript tail read for the last agent activity; enough for many records
/// while keeping the read cheap on transcripts of any size.
const ACTIVITY_TAIL_BYTES: u64 = 512 * 1024;

/// When the agent last did something, in Unix seconds: the newest assistant
/// message or tool result near the end of a Claude transcript. A new prompt is
/// neither, so this still dates an interrupted turn once the next prompt has
/// been written. `None` when the tail holds no such record.
pub fn transcript_last_activity(transcript: &Path) -> Option<i64> {
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

/// Match Claude tool requests to results by call ID. Requests without results
/// have unknown outcomes. Codex transcripts remain outside this adapter.
pub fn transcript_events(session: &SessionKey, cwd: &Path, transcript: &Path) -> Vec<FileEvent> {
    if session.agent != Agent::Claude {
        return Vec::new();
    }
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
