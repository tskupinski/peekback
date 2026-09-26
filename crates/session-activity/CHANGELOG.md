# Changelog

## Unreleased

Breaking: the library no longer knows any particular agent. Stored data is
unchanged.

- Replace the `Agent` enum with `AgentId`, a validated namespace the caller
  chooses. It serializes as the same string, so existing history and keys
  read unchanged.
- `Store::sessions` and `Store::read_all` list every valid agent namespace
  directory instead of a fixed set; an invalid namespace name is a warning.
- Remove the Claude Code and Codex adapters (`hook_events`,
  `transcript_events`, `subagent_transcripts`, `transcript_last_activity`).
  Callers decode their agent's hooks and transcripts into `FileEvent`s.

## 0.2.0 - 2026-09-25

Breaking: the event schema and public API change. Events are written with
schema 2, which 0.1.x rejects; schema 1 history remains readable.

- Write events with schema 2, which adds `FileEvent::concurrent`: the other
  sessions that were working where a scan observed the file. Schema 1 events
  remain readable.
- Add `Store::append_new`, which appends only events not already retained,
  under one exclusive lock.
- Add `subagent_transcripts` to find Claude subagent transcripts beside a main
  transcript.
- Skip other checkouts in scans: directories with their own `.git` directory
  or a `.git` file pointing into `worktrees/`, which usually have sessions of
  their own. Submodules stay included.
- Give `ScanRoot` an inclusive `until` bound next to `since`.
- Add `transcript_last_activity`, and `FileActivity::scan_only` and
  `FileActivity::possibly_shared`.

## 0.1.1 — 2026-09-24

- Reject retention cutoffs in the future. A cutoff cannot be lowered, so one
  given in milliseconds would silently stop recording the session.
- Refuse maintenance, in preview and apply alike, when a session directory
  holds a `.json` file that is not a valid batch name. Such a file could be
  packed while staying visible to readers, duplicating its events.

## 0.1.0 — 2026-09-23

First standalone library release, developed alongside Peekback and published
on [crates.io](https://crates.io/crates/session-activity/0.1.0).

- Enumerate retained sessions and read history across both agent namespaces,
  with per-session warnings and support for compacted/retained histories.
- Normalize Claude Code file tools/failure hooks, Claude transcript results,
  and Codex apply_patch operations.
- Retain namespaced, versioned file events with outcomes, provenance, call IDs,
  rename origins, and deleted paths.
- Query file evidence and reconcile retries without treating scans as proven writes.
- Read healthy history with explicit batch warnings, or require complete reads.
- Report bounded scan completeness and filesystem errors.
- Preview/apply crash-aware compaction and fixed timestamp retention on Unix.

Requires Rust 1.87+. Storage and maintenance target macOS/Linux. Arbitrary shell
commands and Codex transcripts are not parsed; there is no complete filesystem
audit or historical content snapshot.
