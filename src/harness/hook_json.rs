//! The command-hook protocol Claude Code defined and Codex follows: the agent
//! runs a command per event and writes one JSON payload to its stdin.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::Value;

use super::Harness;
use crate::record::{Event, Observation, Place, Start};

/// `None` only for a payload that names no valid session or an event
/// Peekback does not follow. A payload without a usable place still decodes,
/// so its turn boundary is not lost.
pub fn decode(harness: Harness, input: &Value, now: i64) -> Result<Option<Observation>> {
    let Some(session_id) = input["session_id"].as_str().filter(|id| session_activity::valid_id(id)) else {
        return Ok(None);
    };
    let place = place(input);
    let event = match input["hook_event_name"].as_str() {
        Some("SessionStart") if input["source"] == "compact" => Event::SessionStarted(Start::Compaction),
        Some("SessionStart") => Event::SessionStarted(Start::New),
        Some("UserPromptSubmit") => Event::PromptSubmitted,
        Some("PostToolUse" | "PostToolUseFailure") if place.is_some() => {
            Event::ToolUsed(session_activity::hook_events(harness.into(), input, now)?)
        }
        Some("PostToolUse" | "PostToolUseFailure") => Event::ToolUsed(Vec::new()),
        Some("Stop") => Event::TurnEnded,
        Some("SessionEnd") => Event::SessionEnded,
        _ => return Ok(None),
    };
    Ok(Some(Observation { harness, session_id: session_id.into(), event, place }))
}

fn place(input: &Value) -> Option<Place> {
    let cwd = input["cwd"].as_str().map(Path::new).filter(|cwd| cwd.is_absolute())?;
    let transcript = match &input["transcript_path"] {
        Value::Null => None,
        Value::String(path) if path.is_empty() => None,
        Value::String(path) => Some(PathBuf::from(path)),
        _ => return None,
    };
    Some(Place { cwd: cwd.to_owned(), transcript })
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use session_activity::Operation;

    use super::*;

    fn decoded(harness: Harness, extra: Value) -> Option<Observation> {
        let mut input = json!({
            "session_id": "s-1",
            "hook_event_name": "Stop",
            "cwd": "/work",
            "transcript_path": "/work/t.jsonl",
        });
        input.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        decode(harness, &input, 100).unwrap()
    }

    fn event(extra: Value) -> Option<Event> {
        decoded(Harness::Claude, extra).map(|observation| observation.event)
    }

    fn place_of(extra: Value) -> Option<Place> {
        decoded(Harness::Claude, extra).unwrap().place
    }

    #[test]
    fn hook_names_map_to_lifecycle_events() {
        assert_eq!(event(json!({"hook_event_name": "SessionStart"})), Some(Event::SessionStarted(Start::New)));
        assert_eq!(
            event(json!({"hook_event_name": "SessionStart", "source": "resume"})),
            Some(Event::SessionStarted(Start::New))
        );
        assert_eq!(
            event(json!({"hook_event_name": "SessionStart", "source": "compact"})),
            Some(Event::SessionStarted(Start::Compaction))
        );
        assert_eq!(event(json!({"hook_event_name": "UserPromptSubmit"})), Some(Event::PromptSubmitted));
        assert_eq!(event(json!({"hook_event_name": "Stop"})), Some(Event::TurnEnded));
        assert_eq!(event(json!({"hook_event_name": "SessionEnd"})), Some(Event::SessionEnded));
    }

    #[test]
    fn unsupported_events_and_invalid_sessions_are_not_observed() {
        assert_eq!(event(json!({"hook_event_name": "Notification"})), None);
        assert_eq!(event(json!({"hook_event_name": null})), None);
        assert_eq!(event(json!({"session_id": "../escape"})), None);
        assert_eq!(event(json!({"session_id": ""})), None);
        assert_eq!(event(json!({"session_id": null})), None);
    }

    #[test]
    fn a_payload_without_a_usable_place_still_decodes() {
        assert_eq!(place_of(json!({"cwd": "relative"})), None);
        assert_eq!(place_of(json!({"cwd": null})), None);
        assert_eq!(place_of(json!({"transcript_path": 7})), None);
        assert_eq!(event(json!({"cwd": "relative"})), Some(Event::TurnEnded));
    }

    #[test]
    fn an_empty_or_missing_transcript_is_absent() {
        let without = Some(Place { cwd: "/work".into(), transcript: None });
        assert_eq!(place_of(json!({"transcript_path": ""})), without);
        assert_eq!(place_of(json!({"transcript_path": null})), without);
        assert_eq!(place_of(json!({})), Some(Place { cwd: "/work".into(), transcript: Some("/work/t.jsonl".into()) }));
    }

    #[test]
    fn tools_become_file_events_and_other_tools_none() {
        let write =
            json!({"hook_event_name": "PostToolUse", "tool_name": "Write", "tool_input": {"file_path": "a.md"}});
        let Some(Event::ToolUsed(events)) = event(write) else { panic!("not a tool event") };
        assert_eq!(events.len(), 1);
        assert_eq!((events[0].path.as_path(), events[0].operation), (Path::new("/work/a.md"), Operation::Write));
        let bash = json!({"hook_event_name": "PostToolUse", "tool_name": "Bash", "tool_input": {"command": "ls"}});
        assert_eq!(event(bash), Some(Event::ToolUsed(Vec::new())));
        let unplaced = json!({"hook_event_name": "PostToolUse", "tool_name": "Write", "cwd": "relative"});
        assert_eq!(event(unplaced), Some(Event::ToolUsed(Vec::new())));
    }

    #[test]
    fn failed_tools_are_observed_for_every_harness() {
        for harness in Harness::ALL {
            let failed = decoded(harness, json!({"hook_event_name": "PostToolUseFailure", "tool_name": "Bash"}));
            assert_eq!(failed.map(|observation| observation.event), Some(Event::ToolUsed(Vec::new())));
        }
    }
}
