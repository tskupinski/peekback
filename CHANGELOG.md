# Changelog

## 0.1.1 — 2026-09-24

Published on [crates.io](https://crates.io/crates/peekback/0.1.1). Updates the
package README, which still described the crates as unpublished.

- Highlight blocks with pending comments and show comment counts and notes on
  hover. Keep selections attached to their original document while composing.
- Keep the viewer on its embedded page: block navigation, new windows and
  dropped items, ignore page messages from other origins, and route Mermaid's
  SVG links through the link handler.
- Never paste into a terminal whose agent has exited; copy instead. Strip
  control characters from sent text, refuse multi-line tmux pastes into panes
  without bracketed paste, and use a separate tmux buffer per send.
- Fix Kitty sends, which passed `--bracketed-paste` without a value.
- Stay in the session the viewer was opened for when opening a file from All
  sessions. Current session keeps listing its files, and sends still go to it.
  Previously the viewer dropped the session, emptying Current session.
  `peekback browse --all-sessions` likewise keeps the session it runs inside.
- Reject retention cutoffs in the future and refuse maintenance when a
  session's history holds unexpected files (session-activity).

## 0.1.0 — 2026-09-23

Published on [crates.io](https://crates.io/crates/peekback/0.1.0).

- Use normal window stacking so the viewer does not stay above other apps.
- Find retained Markdown across all sessions in the viewer picker; browse all
  file types with `peekback browse --all-sessions`. Merge paths while preserving
  provenance, report partial history, and open without guessing a send target.
- Install with Cargo; run `peekback setup` to configure detected agents or
  select one with `--agent`. Setup preserves other settings, backs up changes,
  and supports `--dry-run`; installed hooks need no separate shell script.
- Discover files from local Claude Code and Codex CLI sessions, retaining
  metadata and evidence after sessions end.
- Browse all observed file types in an interactive terminal, with filtering,
  current-text previews, evidence details, and Markdown preview handoff.
- Render Markdown in a native macOS web view with Mermaid, KaTeX, and syntax
  highlighting. Copy selections or paste them back into a live session.
- Query session activity as JSON; preview and apply history compaction or
  explicit timestamp retention.
- Preserve healthy history when batches are damaged, report incomplete scans,
  and prevent late hooks from reopening ended sessions.
- Bound daemon replies, including stalled or partial replies. Add `--version`
  and support installation/hook paths containing spaces.
- Publish `session-activity` as a standalone Rust library with its own API docs,
  consumer example, package verification, and independent release tags.

The app supports Apple Silicon and macOS 14 or newer. Optional archives are not
Developer ID signed or notarized. Intel, Linux, Windows, and Codex desktop/IDE
composer integration are outside this app release. See [RELEASING.md](RELEASING.md)
for validation evidence and the manual acceptance checklist.
