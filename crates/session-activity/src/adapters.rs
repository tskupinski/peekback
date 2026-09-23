use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;

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
