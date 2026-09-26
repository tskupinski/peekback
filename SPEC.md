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
| `crates/session-activity/` | Agent-independent event schemas, path normalization, storage, scans, reconciliation, compaction and retention |
| `src/harness/` | One module per agent harness: identity, session variables, setup, decoding its hooks into lifecycle observations, tool and transcript adapters, private directories |
| `src/setup.rs`, `src/record.rs` | Agent hook configuration; recording observations as turns, file events and registry entries |
| `src/registry.rs`, `src/lifecycle.rs` | Live sessions, terminal identity, lifecycle locks and end markers |
| `src/capture.rs`, `src/capture_jobs.rs`, `src/activity.rs`, `src/discovery.rs` | Capture policy and retry jobs, JSON queries, Markdown filtering |
| `src/browse.rs` | Interactive terminal browser and plain listing |
| `src/client.rs`, `src/protocol.rs`, `src/daemon/` | Viewer IPC, native window, file watchers, global hotkey |
| `src/mux/`, `src/send.rs` | Multiplexer adapters and verified delivery selection |
| `src/terminal.rs`, `src/theme.rs` | Terminal application operations and appearance |
| `src/turn_history.rs` | Durable turn intervals and original roots for overlap detection |
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
hook environment. Peekback recognizes supported Claude file tools and
Codex `apply_patch` operations; shell commands and Codex transcripts are not
parsed for file effects. Files written through shell commands are found by
the turn capture described under File evidence.

### Harness boundary

Each supported agent harness is a module under `src/harness/`, and every
`match` on a harness lives in `src/harness/mod.rs`. A harness module provides
its name, executables, session environment variables, setup file, a decoder
from its hook payloads to lifecycle observations (session started or
compacted, prompt submitted, tool used, turn ended, session ended), its tool
mapping, and optionally a transcript reader and private directories. The
lifecycle core in `src/record.rs` consumes only observations. A payload
without a usable cwd still closes its turn but does not rewrite the registry
entry. Session environment variables are read Codex first, since Codex
started from Claude Code's Bash tool inherits `CLAUDE_CODE_SESSION_ID`.

The boundary assumes a harness runs like Claude Code and Codex:

1. One agent process, in one terminal pane, owns one session. The hook
   records the nearest non-shell ancestor as the agent; liveness, pane
   ownership and sending all depend on it.
2. A pane shows one session at a time.
3. The integration runs inside the agent's process tree, with its
   environment, so the pane and the agent process can be captured.
4. Sending pastes into a TUI composer that accepts bracketed paste.
5. Subagents are recorded with the main session, not as sessions of their own.

A harness that breaks one of these needs a design change, not only a module.

Hooks maintain two different kinds of state:

- The live registry holds session IDs, agent, cwd, optional transcript path,
  timestamps, the start of the open turn, and terminal metadata used for
  preview selection and sending.
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
  turns/<AGENT>.<SESSION_ID>.json
  pending-captures/<AGENT>/<SESSION_ID>/<JOB>.json
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
A resumed session keeps everything it captured before. Each non-compaction
`SessionStart` creates a new incarnation, so an old viewer cannot silently
send to a resumed session even if the native session ID is unchanged.

Hooks record the agent process, the nearest non-shell ancestor of the hook,
with its start time against PID reuse. A registered session whose agent has
exited is not live: listings, session resolution and the hotkey skip it, as
they skip ended sessions. Overlap detection still uses its recorded turns.
A later hook from a resumed process records the new agent. Entries from before process tracking count as
live. This is verified for Claude Code, where hooks run through a shell child
of the `claude` process; Codex is not yet verified.

`peekback status --prune` closes entries whose agent has exited and entries
with no hook or transcript activity in 24 hours. It first captures a turn the
agent left open, as `SessionEnd` would have.
The live registry still uses native session IDs and rejects registration of
an ID already owned by another agent; the retained store uses both agent and
session ID. Hooks provide no reliable process generation, so an old start/end
hook cannot always be distinguished from one for a resumed session.

Session resolution prefers an explicit ID, then an explicit tmux pane (looked
up on the tmux server in the caller's `TMUX`, else on any server when the id
is unique), then
`CODEX_THREAD_ID`, `CODEX_SESSION_ID`, or `CLAUDE_CODE_SESSION_ID` from the
calling process, then the most recently active live session. Retained browsing
can select an ended session by agent and ID, or all sessions without a live
registry entry.

## File evidence and capture

A file event records its session, path, operation, outcome, source, timestamp,
cwd, optional tool call ID, optional rename origin, and the sessions that were
working concurrently when a scan observed it. Outcomes can be `succeeded`,
`failed`, or `unknown`; observing a tool request alone does not prove a
successful write.

The library reconciles retries and known outcomes while preserving their
sources. Conflicting known outcomes remain visible. Files are grouped by
lexically normalized path; rename origins and destinations both appear.
Symlink aliases are not unified. Scan observations are never promoted to
attributed writes.

Ownership is decided once, when evidence is captured, and stored. Views only
read the store: nothing is inferred from the filesystem at display time, so a
session's files do not change when someone else edits them later, and a
resumed session, the terminal browser and `browse --all-sessions` all show the
same list. The viewer's Scratchpad scope is the one live listing,
because only its own session writes there.

### Sources

- **Hooks**, live. `PostToolUse` and `PostToolUseFailure` for Claude file
  tools and Codex `apply_patch` are appended as they happen, from the main
  agent and from subagents when the agent version reports their tool calls.
- **Claude transcripts**, backfill. The main transcript and every subagent
  transcript under `<transcript stem>/subagents/` are parsed at each capture,
  and events not already stored are appended. Hook coverage of subagent tool
  calls varies between Claude Code versions and agent types, so transcripts
  are what makes subagent writes reliable. Parsing is best-effort and can be
  incomplete without a diagnostic report.
- **Turn scans**, for effects no tool reports, such as shell redirection,
  scripts, `cp` and `mv`. Stored as `observed` with a scan source.
- **Legacy registry** paths from older registry versions.

### Turns

A turn opens at `UserPromptSubmit`, whose time the registry keeps, and closes
at `Stop`, ending then. A turn no `Stop` closed is abandoned: interrupted with
Esc, since Claude sends no `Stop` for that, quit mid-turn, or left open by an
agent that died. The next `UserPromptSubmit`, `SessionEnd`, a `SessionStart`
other than compaction, or pruning closes it, and it ends at the agent's last
activity rather than at that hook, which can come hours later: the newest
assistant message or tool result in the last 512 KiB of the Claude transcript,
or the session's last hook without one. A new prompt is neither kind of
record, so it cannot stretch the turn. Compaction can happen inside a turn and
keeps it open. Before a close updates or removes the live entry, it saves a
pending capture with the previous session's cwd, transcript path and turn
bounds. Capture and transcript backfill run under the lifecycle lock; a resume
in another directory cannot change the closed turn's scan roots.

Jobs are removed only after both history and file evidence are stored. Later
prompt, stop, start and end hooks retry them idempotently; tool hooks, which
come many times a turn, do not. A job counts its failures, and after three
only `peekback activity retry --agent <AGENT> --session <ID>` retries it,
which also works after session exit. An unreadable job is renamed to
`.corrupt` and reported once. If a job cannot be saved at all, the hook
still closes the turn or session first, then captures from the same context,
since a capture can wait on the store's lock past the hook's timeout. Pruning
does the same. A retry preserves context,
not a filesystem snapshot: files changed again before retry may no longer
provide evidence for the original turn.

Capture roots:

| Root | Depth | Accepted modification time |
| --- | --- | --- |
| Session cwd | 4 | within the turn, with 2 seconds of slack |
| Claude memory directory | 1 | within the turn, with 2 seconds of slack |
| Claude scratchpad | unlimited | any; the path belongs to this session alone |

The roots share a 20,000-entry budget. Hidden and build directories, symlinked
directories, and other checkouts below the root are skipped: a directory with
its own `.git` directory, or with a `.git` file pointing into `worktrees/` (a
linked worktree). A submodule's `.git` file points into `modules/`, so
submodules stay part of the project. The cwd scan is skipped for `/` and the
home directory.
Budget exhaustion and read failures are reported on the hook's stderr; depth
limits are policy and are not reported. A hook interrupted during capture
leaves the job available for retry. Transcript parsing and retained-history
reads still scale with history size; incremental parsing and indexing remain
future performance work.

A modification time only shows the last change, which is why the decision is
made and stored at the end of the turn: a later edit outside any turn does not
remove the file from the session.

Hook-reported writes appear immediately. Shell-written files appear when the
turn closes.

### Concurrent sessions

Two sessions working in the same directory at the same time cannot be told
apart by the filesystem. A scan event lists sessions whose project or memory
root contains the path and whose turn covers its modification time.
Completed turns and their original roots are stored atomically in
`turns/<agent>.<session>.json`, independently of the live registry. They are
published before an ended session disappears, survive pruning and resuming,
and are retained without automatic expiry so long-running turns can still
find overlap. Activity-event retention does not remove this turn metadata.
Damaged histories are left intact and reported as incomplete overlap evidence;
file capture and session lifecycle updates continue.

Open turns come from registered sessions. They reach the present while the
agent is alive and was active in the last 10 minutes; otherwise they end at
its last activity. Views mark scan-only files with overlap as possibly written
by another session. Separate worktrees avoid overlap because each session scans
its own directory and nested checkouts are skipped.

### Views

The terminal browser includes all file types, reads, failed operations, missing
paths, and rename origins, and labels scan evidence. The viewer filters to
existing `.md` or `.markdown` files, excluding reads and failed operations after
reconciliation, and marks scan evidence. Both sort by latest observed activity
and refresh from the store when the registry changes.

The terminal browser's all-sessions mode reads the same stored history for
every session, merging identical paths while keeping agent/session provenance.
Its Markdown previews never infer a send-back target from the file: they stay
in the session the browser runs inside, and are standalone when there is
none.

## Storage and maintenance

The store writes immutable JSON batches with validated session identity,
absolute paths, and schema versions. Schema 2 adds the concurrent sessions of
scan events; schema 1 batches remain readable and have none. Files are synced and atomically renamed;
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
[library guide](https://github.com/tskupinski/peekback/tree/main/crates/session-activity)
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
thread. `daemon/controller.rs` coordinates state and effects;
`daemon/messages.rs` defines the protocol and `daemon/state.rs` defines session
and view identity. Four bounded workers handle file reads, discovery, listings
and theme extraction. A separate serial worker handles sends and clipboard
operations. The UI thread never waits for these operations.

Every view has a generation and a namespaced session identity with incarnation
and process identity. Actions carry that context; stale actions are rejected.
Listings also carry request IDs, so an older result cannot replace a newer
request, including within the same session. Files may be opened only from the
matching accepted list. Registry changes are coalesced; a five-second refresh
also catches process exits and activity maintenance without a registry event.

The socket accepts at most 16 concurrent connections, requests up to 64 KiB,
and uses absolute read deadlines and bounded response waits. Each request
carries the wall-clock time its client stops waiting, set at connection; the
daemon converts it to its own clock when it reads the request, keeps a margin
for the reply, and applies nothing after it, however long it was stopped or
busy in between. Requests without one get two seconds from being read. Page messages are capped at 4 MiB.

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
or hides it when already focused. When hidden, it opens the session in the pane of the most recently used client
of each registered session's tmux server, newest first. If no registered
session is there but an agent is in that pane's foreground, named by the
`argv[0]` of the terminal's foreground process group (Codex registers only at
its first prompt), it opens an empty view for that pane, which switches to the
pane's session once that agent registers one; the default tmux server is asked
too, for when nothing has registered yet. Otherwise it opens the most recently
active session. A tmux server with no attached client has no active pane. The hotkey exists only while the
daemon runs.

Showing a session always makes it current, including its documents and send target. A session without Markdown
renders an empty state instead of keeping the previous session's document, and opens its first document when a
registry change lists one.

Document loading shares the terminal browser's nonblocking regular-file check.
The native viewer rejects files over 4 MiB and loads them on workers. A watcher
reloads the current document, observing both a symlink and its resolved target.
Changes are debounced and stale reads are discarded; an intentionally emptied
file replaces the previous content. If it is deleted, the last rendered
content remains with a banner. Registry changes update the current session's
documents. The viewer shows one session at a time; the hotkey or `show`
changes it. The document picker has Current session,
Scratchpad and Bookmarks scopes, cycled with Tab and Shift-Tab. Scratchpad and
Bookmarks refresh on opening and with Ctrl-R, and their warnings appear in the
picker.

Scratchpad lists Markdown under the Claude Code session's scratchpad, read at
display time: the directory belongs to that session alone, so a live listing
cannot pick up another session's files. It uses the bookmark walk and limits,
sorted newest first and labelled relative to the scratchpad. Sessions without
a scratchpad, such as Codex sessions, and standalone documents show that there
is none. Opening a scratchpad file keeps the live session.

Bookmarks come from the `bookmarks` config list, reread for every request.
Entries starting with `/` or `~` are absolute; others resolve against the
viewer's session cwd and are skipped without a session. An entry is a file, a
directory (Markdown recursively) or a glob matched with globset, walking only
from the deepest literal directory. Walks skip hidden names, do not follow
directory symlinks, stop at depth 16 and 20,000 visited entries, and list at
most 200 files in config order without duplicates; the page is told how many
matched. Missing files are omitted silently; invalid globs and explicit
non-Markdown files produce warnings. Opening a bookmark keeps the live session,
like Scratchpad.

Keyboard and mouse selections map back to Markdown. Copy writes to the
clipboard; Send pastes a blockquote; Comment accumulates notes for a combined
paste. Drafts belong to their session incarnation and outgoing groups use
canonical file paths, with labels calculated for the delivery target. Other
sessions' drafts are preserved separately. Send results carry request IDs;
only a matching successful paste removes that batch. Clipboard fallback keeps
the draft. Selection actions are disabled while block mappings are rendering.
Nothing submits a prompt. Comments are not persisted. Esc and `:q` hide
the viewer; `peekback quit` stops the daemon and unregisters the hotkey.

The config file is `~/.config/peekback/config.toml`. Window placement and hotkeys
are read at daemon startup; bookmarks refresh their configuration on request,
themes on opening a view, and delivery preferences on sending.
Automatic themes read iTerm2 preferences or Ghostty configuration; unknown
terminals use built-in system light/dark palettes. Terminal colors and monospace
fonts style code and chrome while body text keeps the system font. Font and
size overrides also work with system colors and unsupported terminals. Theme
subprocesses use the same bounded runner as multiplexer commands.

## Sending to the terminal

Hooks record candidate panes from the environment without contacting terminal
servers. Pane and focus resolution excludes known TTY mismatches; unknown
ownership remains a candidate for display, never verification for delivery. Each adapter owns capture, inspection, focus discovery, sending,
and the environment variables its commands must clear. Inspection distinguishes
an available pane (with optional TTY evidence), an absent pane, and an unknown
result such as a timeout. Focus policy caches results by adapter and server.
tmux is the only adapter; outside it, sends use the clipboard and the hotkey
opens the most recently active session. Panes of adapters an older version
recorded, such as WezTerm and Kitty, are skipped when a session is read.
Terminal application placement and explicit keystroke paste live separately in
`terminal.rs`.

Automatic selection requires a matching TTY from both the live agent process
and the addressed pane, and the agent must be running or sleeping in that
terminal's foreground process group. Suspended and background agents do not
qualify. It returns that exact destination and its TTY evidence,
then rechecks both immediately before sending. If none can be verified, it
copies to the clipboard. A pinned multiplexer also requires verification and
reports an error when it cannot establish ownership.

`backend = "keystroke"` is an explicit opt-in: it requires macOS Accessibility
permission and pastes into the terminal application's currently focused split
or tab, without verifying it belongs to the session. Automatic selection never
uses this backend. Clipboard can also be selected explicitly.

The viewer re-reads the live session before sending and requires the same
session incarnation and process that the action targeted. Sent text loses control
characters except tab and newline. tmux sends multi-line text only when the
pane has bracketed paste enabled. Adapter commands drain input/output with a
two-second deadline and bounded output; errors after delivery starts are
reported without retrying through another backend. The send target remains
the viewer's session even when it displays another session's file. Standalone
documents have no send target. Codex desktop and IDE composers remain outside
the supported integration. Verification and delivery are separate operations;
terminal APIs do not provide an atomic ownership-check-and-paste transaction.

## Deliberate limits

This is observed file activity, not a complete filesystem audit. Shell effects
outside the scanned roots, background jobs that finish after their turn, and
shell writes after an agent's last recorded activity in an abandoned turn are
missed. Without a Claude transcript, as for Codex, an abandoned turn ends at
the session's last hook, so its shell writes can be missed. Simultaneous
shell writes by two sessions in one directory are attributed to both. Documents always show current
contents, not what a session saw at the time. Historical content snapshots,
viewer editing, persistent comments, MCP-based comment exchange, and automatic
prompt submission are outside the current implementation.
