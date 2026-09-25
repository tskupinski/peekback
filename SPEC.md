# Peekback architecture

This document describes the current implementation. For installation and keys,
see [README.md](README.md); for publication and validation evidence, see
[RELEASING.md](RELEASING.md). The original implementation milestones are preserved
in Git history.

Peekback has two consumers of session file activity: a terminal browser for all
file types and a native Markdown viewer with a selection-to-prompt loop. The
independent `session-activity` crate provides shared normalization, retained
storage, scanning, and queries. The application chooses how to collect and
present that evidence.

## Package boundaries

| Component | Responsibility |
| --- | --- |
| `crates/session-activity/` | Agent adapters, event schemas, path normalization, storage, scans, reconciliation, compaction and retention |
| `src/setup.rs`, `src/hooks.rs` | Agent hook configuration and collection |
| `src/registry.rs`, `src/lifecycle.rs` | Live sessions, terminal identity, lifecycle locks and end markers |
| `src/activity.rs`, `src/discovery.rs` | Application scan roots, JSON queries, Markdown filtering |
| `src/browse.rs` | Interactive terminal browser and plain listing |
| `src/client.rs`, `src/protocol.rs`, `src/daemon/` | Viewer IPC, native window, file watchers, global hotkey |
| `src/send.rs`, `src/theme.rs` | Terminal paste backends and terminal appearance |
| `assets/` | Embedded Markdown rendering and viewer interactions |

Both packages are published on crates.io with independent versions. Peekback
uses a versioned workspace dependency on `session-activity`. The app supports
Apple Silicon macOS; the library's storage and maintenance are validated on
macOS and Linux and do not depend on the GUI or application registry.

## Hook setup and collection

`peekback setup` merges hooks into Claude's settings.json and Codex's hooks.json,
respecting `CLAUDE_CONFIG_DIR` and `CODEX_HOME`. It detects installed agents or
accepts an explicit selection. The installed command calls the absolute path
to `peekback activity record --agent claude|codex`; no checkout or helper script
is required. `hooks/register.sh` and the JSON snippets remain compatibility
options for manual installations.

Setup preserves unrelated settings, replaces only its own marked entries,
backs up changed files, and uses atomic file replacement. `--dry-run` previews
the result. Malformed configurations and symlinked settings are refused.
Codex hook trust remains a user decision in `/hooks`; setup does not override it.

Both agents register `SessionStart`, `UserPromptSubmit`, `PostToolUse`, `Stop`,
and `SessionEnd`. Claude additionally registers `PostToolUseFailure`. The
collector receives JSON on stdin and captures terminal identifiers from the
hook environment. The library recognizes supported Claude file tools and
Codex `apply_patch` operations; shell commands and Codex transcripts are not
parsed for file effects.

Hooks maintain two different kinds of state:

- The live registry holds session IDs, agent, cwd, optional transcript path,
  timestamps, and terminal metadata used for preview selection and sending.
- The activity store holds versioned file observations keyed by agent and
  session ID. It stores metadata and evidence, not prompts or document bodies.

## State and session lifecycle

The default state directory is `~/.local/state/peekback`. `PEEKBACK_STATE_DIR`
overrides it for the app and hooks; all participating processes must use the
same value.

```text
peekback/
  sessions/<SESSION_ID>.json
  lifecycle/<SESSION_ID>.lock
  lifecycle/<AGENT>.<SESSION_ID>.ended
  activity/<AGENT>/<SESSION_ID>/
  daemon.sock
  daemon.lock
  daemon.log
```

Per-session lifecycle locks serialize registry updates and pruning. Registry
files are written through unique temporary files, synced, and renamed. A
`SessionEnd` publishes an end marker before importing available transcript and
legacy observations and removing the live entry. Existing retained activity
survives session exit and pruning. A late tool hook can append observations
without reopening a closed session; an explicit `SessionStart` reopens it.

`peekback status --prune` closes entries with no hook or transcript activity in
24 hours. Pruning does not perform the transcript import done by `SessionEnd`.
The live registry still uses native session IDs and rejects registration of
an ID already owned by another agent; the retained store uses both agent and
session ID. Hooks provide no reliable process generation, so an old start/end
hook cannot always be distinguished from one for a resumed session.

Session resolution prefers an explicit ID, then an explicit tmux pane, then
`CODEX_THREAD_ID`, `CODEX_SESSION_ID`, or `CLAUDE_CODE_SESSION_ID` from the
calling process, then the most recently active live session. Retained browsing
can select an ended session by agent and ID, or all sessions without a live
registry entry.

## File evidence and discovery

A file event records its session, path, operation, outcome, source, timestamp,
cwd, optional tool call ID, and optional rename origin. Outcomes can be
`succeeded`, `failed`, or `unknown`; observing a tool request alone does not
prove a successful write.

The library reconciles retries and known outcomes while preserving their
sources. Conflicting known outcomes remain visible. Files are grouped by
lexically normalized path; rename origins and destinations both appear.
Symlink aliases are not unified. Scan observations remain candidates and are
never promoted to attributed writes.

Current-session queries combine retained hooks with available Claude transcript
and legacy registry observations. Optional candidate discovery scans:

- The Claude scratchpad associated with its transcript's project directory.
- The Claude project's memory directory, one level deep.
- The session cwd, four levels deep, for files modified since session start.

The roots share a 20,000-entry budget. Hidden/build directories and symlink
directories are skipped; the app avoids project scans of `/` or the home
directory. Depth limits, budget exhaustion, and read failures produce warnings.
Scans are heuristic: files another process changed during the session may be
included. Candidates are computed at query time and are not retained by these
queries. Claude transcript parsing is best-effort and can be incomplete without
a diagnostic report.

The terminal browser includes all file types, reads, failed operations, missing
paths, and rename origins. Scans are opt-in with `--candidates`. The viewer
includes candidates for a live session but filters to existing `.md` or
`.markdown` files, excluding reads and failed operations after reconciliation.
Both views sort by latest observed activity.

All sessions mode reads retained history only. It neither parses all old
transcripts nor scans old projects. It merges identical paths while keeping
agent/session provenance. Consequently, a candidate visible in a live session
may not appear in All sessions. An all-session Markdown preview never infers a
send-back target from the file: it stays in the session the viewer or browser
was opened from, and is standalone when there is none.

## Storage and maintenance

The store writes immutable JSON batches with validated session identity,
absolute paths, and schema versions. Files are synced and atomically renamed;
Unix directory syncing strengthens crash durability within filesystem
guarantees. Partial temporary files are ignored.

Reads can return healthy events with warnings for invalid, oversized, or
unreadable batches. Strict reads reject partial history. An invalid authoritative
checkpoint is a hard error for its session; `Store::read_all` reports that
session's error and continues with other sessions.

Compaction and timestamp retention are preview-only unless `--apply` is passed.
Maintenance refuses damaged history, writes synced pack files, publishes a
checkpoint, then removes covered batches. The checkpoint prevents duplicated
history after interrupted cleanup. Retention stores a monotonic cutoff so
older observations cannot be reimported. Unix readers/appenders use shared
advisory locks and maintenance uses an exclusive lock.

There is no automatic retention limit. Reads still load retained history into
memory; compaction reduces file count, not memory use. All-session reads lock
each session separately and do not provide a globally atomic snapshot. See the
[library guide](https://github.com/tskupinski/peekback/tree/master/crates/session-activity)
for detailed API and storage compatibility guarantees.

## Terminal browser

`peekback browse` uses crossterm for the file list, current-text preview, evidence,
and warning views. Wide terminals show two panes; narrow terminals show the
focused pane. Filtering and refresh are local interactions. Piped output or
`--list` produces a plain table. Preview text is current UTF-8 content capped at
256 KiB, not a historical snapshot; missing, binary, and special files show an
explanation.

The browser does not start the viewer until `p` is pressed on Markdown. Live
session previews retain their send target. Ended-session previews open
standalone; all-session previews keep the live session the browser runs
inside, found from the agent's environment variables, and are standalone
without one. `q`, Esc, and Ctrl-C exit; Esc while editing a filter
only finishes filter editing.

## Viewer daemon and IPC

One daemon per state directory holds one native window and an exclusive lock
on daemon.lock. `show` connects over a Unix socket and starts the daemon if
needed. Stale sockets are replaced by the lock owner. The spawned daemon keeps
the caller's PATH but removes session-selection environment variables.

CLI IPC is newline-delimited JSON with one request and response per connection:
`show`, `status`, `hide`, or `quit`. Client requests use a three-second deadline
and a 1 MiB reply limit. Once connected, a delayed reply does not cause the
request to be replayed or another daemon to be started. `send` uses the terminal
backend from the CLI and does not require the daemon.

Tao owns the main-thread event loop. Socket and watcher threads pass messages
through an event proxy; UI state and JavaScript evaluation stay on the main
thread. All-session picker queries run on a worker, with at most one active
query, and return documents plus warnings. Bookmark listing also runs on a
worker. The page may open a tracked path or bookmark only from the daemon's
most recent list.

Wry hosts the embedded page at `peekback://app/`. The page sends messages through
`window.ipc.postMessage`; Rust pushes updates through JavaScript evaluation.
Rendered documents are untrusted: the webview may not navigate away from
`peekback://app/`, open windows, or load dropped items, and page messages from
any other origin are ignored. Links, including Mermaid's SVG links, go through
the page's click handler, which opens web links externally.
There is no HTTP server or async runtime. Embedded rendering assets work offline.

## Viewer behavior

Markdown-it renders blocks with source-line mappings; Mermaid, KaTeX, and
highlight.js render diagrams, math, and code. Raw HTML in Markdown is disabled,
Mermaid uses strict security, and KaTeX trust is off. Rendering uses embedded
assets under a Content Security Policy. Links open through a separate daemon
message rather than navigating the web view away from the app.

The window uses normal stacking. Right, left, and over placements are
borderless and positioned relative to the terminal; free placement is decorated.
The app uses macOS accessory activation, without a Dock icon. `show` and the
terminal browser's `p` focus the window; `show --no-focus` only brings it
forward, and show requests from older clients without the field do the same. The global hotkey focuses it,
or hides it when already focused. When hidden, it opens the session in the active pane of each registered
session's tmux server, newest first, else the most recently active session. The hotkey exists only while the
daemon runs.

Showing a session always makes it current, including its documents and send target. A session without Markdown
renders an empty state instead of keeping the previous session's document, and opens its first document when a
registry change lists one.

A watcher reloads the current document. If it is deleted, the last rendered
content remains with a banner. Registry changes update the live session list
and current-session documents. The document picker has Current session, All
sessions and Bookmarks scopes, cycled with Tab and Shift-Tab. All sessions and
Bookmarks refresh on opening and with Ctrl-R; partial-history and bookmark
warnings appear in the picker.

Bookmarks come from the `bookmarks` config list, reread for every request.
Entries starting with `/` or `~` are absolute; others resolve against the
viewer's session cwd and are skipped without a session. An entry is a file, a
directory (Markdown recursively) or a glob matched with globset, walking only
from the deepest literal directory. Walks skip hidden names, do not follow
directory symlinks, stop at depth 16 and 20,000 visited entries, and list at
most 200 files in config order without duplicates; the page is told how many
matched. Missing files are omitted silently; invalid globs and explicit
non-Markdown files produce warnings. Opening a bookmark keeps the live session,
like All sessions.

Keyboard and mouse selections map back to Markdown. Copy writes to the
clipboard; Send pastes a blockquote; Comment accumulates notes for a combined
paste. Nothing submits a prompt. Comments are not persisted. Esc and `:q` hide
the viewer; `peekback quit` stops the daemon and unregisters the hotkey.

The config file is `~/.config/peekback/config.toml`, read at daemon startup.
Automatic themes read iTerm2 preferences or Ghostty configuration; unknown
terminals use built-in system light/dark palettes. Terminal colors and monospace
fonts style code and chrome while body text keeps the system font.

## Sending to the terminal

The automatic backend order is tmux, WezTerm, Kitty, macOS keystroke injection,
then clipboard. A configured backend can be pinned. tmux uses the recorded
socket and pane; WezTerm and Kitty use their remote-control interfaces. The
keystroke backend needs Accessibility permission and pastes into the terminal
app's focused split/tab. Clipboard fallback asks the user to paste manually.

The viewer re-reads the live session before sending so it can reject an ended
target. Hooks record the agent process (the nearest non-shell ancestor, with
its start time against PID reuse); when it has exited, the automatic backend
becomes the clipboard and a pinned one refuses. Sent text loses control
characters except tab and newline. tmux sends multi-line text only to panes
with bracketed paste enabled, and Kitty always brackets. The send target is the
session the viewer was opened for, including while it shows a file from another
session. A standalone document has no send target. Sends report their backend
or error and never press Enter. Terminal remote-control availability and focus
can still affect delivery; WezTerm and Kitty remain unverified on real installs.
Codex desktop and IDE composers are outside the supported integration.

## Deliberate limits

This is observed file activity, not a complete filesystem audit. Unsupported
tools and arbitrary shell effects may be missed. Documents always show current
contents, not what a session saw at the time. Historical content snapshots,
viewer editing, persistent comments, MCP-based comment exchange, and automatic
prompt submission are outside the current implementation.
