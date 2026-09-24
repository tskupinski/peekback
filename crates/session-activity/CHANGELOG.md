# Changelog

## 0.1.1 — 2026-09-24

Published on [crates.io](https://crates.io/crates/session-activity/0.1.1).

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
