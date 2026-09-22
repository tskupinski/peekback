# peekback

A rendered Markdown viewer for Claude Code sessions, with a way back: select
text in the viewer and copy it, send it to the session's prompt, or attach a
note and send that.

You peek at what the agent wrote. What you mark up goes back.

## Problem

Claude Code sessions produce a lot of Markdown: plans, reports, scratchpad
notes, memory files. Reading it in the terminal means reading raw syntax, and
anything beyond prose (Mermaid diagrams, equations, tables) is unreadable.
Reacting to it means retyping or hand-copying the passage you want to talk
about.

Existing tools cover one side each. Terminal viewers (glow, markdown-reader,
mdv) render inside cells and fall over on diagrams and equations, especially
under tmux. Claude Code Artifacts have a full comment loop but live in the
cloud and only for published pages. Nothing is local, session-aware, and
independent of which terminal the session runs in.

## Scope of v1

In:

- One native window showing one document, rendered with real typesetting,
  Mermaid, KaTeX, and syntax highlighting. Live reload on file change.
- Session awareness: the window knows which Claude Code sessions are live,
  which one it is showing, and lists the Markdown files that session wrote.
- Terminal-agnostic triggers: a global hotkey opens the viewer for the most
  recently active session; `peekback show` does the same from any shell.
  Terminal-specific keybindings are optional sharpening, not a requirement.
- Keyboard-first, vim-like: a block cursor, visual selection over blocks,
  search, and a command line. Mouse selection works too, for sub-block
  precision.
- Selection actions: Copy, Send to prompt, Comment. Comments accumulate in a
  panel and are sent as one prompt. A block selection sends the exact
  Markdown source of those blocks.
- Send works in any terminal, through the best available backend: terminal
  IPC where the terminal has one, keystroke injection otherwise, clipboard as
  the floor.

Out, deliberately:

- Comment persistence, threads, re-anchoring after edits.
- Any agent-facing API (MCP, hooks that push comments to the agent).
- Editing the document in the viewer.
- Anything that submits a prompt on the user's behalf. Every send lands in the
  input and waits for Enter.
- Linux and Windows keystroke injection. IPC and clipboard backends are
  portable; the injection backend is macOS only in v1.

## Why a native web view and not a TUI

Mermaid and KaTeX are JavaScript libraries with no faithful ports. Terminal
tools that show them either embed a JS engine or screenshot headless Chrome,
then push images through the Kitty graphics protocol, which tmux strips unless
passthrough is enabled and which does not scroll or clip reliably even then.
Text selection in a TUI competes with terminal mouse modes and is meaningless
across a rendered image.

A web engine gives rendering, selection, and copy for free. Using a system
WKWebView from a Rust binary (wry) instead of a browser tab keeps startup under
a second, avoids tab clutter, and lets the window sit beside the terminal and
toggle from a hotkey. Performance is not a differentiator either way; a long
document renders in tens of milliseconds.

## Why not depend on tmux

tmux would give three things for free: an exact "which session is in this
pane" answer, a keybinding, and `paste-buffer` to inject text. Each has a
terminal-agnostic replacement, and the replacements are better designs, not
just more portable ones:

- Session identity comes from Claude Code hooks, which fire per session in
  every terminal. Recording "last active" on each prompt makes "the session I
  was just using" resolvable with no terminal cooperation at all.
- A global hotkey needs no terminal config and works even when the viewer is
  triggered from somewhere other than the terminal.
- Text injection is the one place where terminals differ. It is modelled as a
  backend with a probe order, so tmux, Kitty, and WezTerm remote control are
  used where present and the loop still closes everywhere else.

tmux remains supported as one backend and one optional keybinding.

## Environment facts the design relies on

Verified on the development machine (macOS, iTerm2 and Ghostty, tmux 3.5a,
Claude Code 2.1.x):

- A command run inside a session (through the `!` prefix or a hook) sees
  `CLAUDE_CODE_SESSION_ID`. The Claude process itself does not carry the
  session id in its environment, only its children do.
- Claude Code hooks receive JSON on stdin with `session_id`,
  `transcript_path`, `cwd`, and `hook_event_name`. SessionStart,
  UserPromptSubmit, Stop, and SessionEnd all fire with this shape.
- A child of the session inherits the terminal's identifying variables:
  `__CFBundleIdentifier` (the terminal app's bundle id on macOS),
  `TERM_PROGRAM`, and where present `TMUX_PANE`, `KITTY_WINDOW_ID` with
  `KITTY_LISTEN_ON`, `WEZTERM_PANE`, `ITERM_SESSION_ID`. Under tmux these come
  from the tmux server's environment, so they describe the terminal the
  server was started from, which can differ from the one currently attached.
- Session transcripts live at
  `~/.claude/projects/<project-slug>/<session-id>.jsonl`, where the slug is the
  project path with `/` replaced by `-`.
- Session scratchpads live at
  `/private/tmp/claude-501/<project-slug>/<session-id>/scratchpad`.
- Memory files live at `~/.claude/projects/<project-slug>/memory/*.md`.
- tmux 3.5a supports `load-buffer` plus `paste-buffer -p`, which pastes into a
  pane with bracketed paste. `$TMUX` holds the server socket path, and pane
  ids are unique only within one server.
- Claude Code 2.1.278 writes files through the `Write`, `Edit`, and
  `NotebookEdit` tools, and through shell commands in `Bash` (heredocs,
  `sed -i`, scripts). There is no `MultiEdit`. A session in auto mode is told
  to prefer Bash for file changes, so Markdown written through Bash is the
  common case, not an edge case.
- wry serves a page through a custom URL scheme handler
  (`with_custom_protocol`, `peekback://` on macOS), receives messages from
  the page through `with_ipc_handler` (`window.ipc.postMessage`), and pushes
  to the page through `evaluate_script`. A WKWebView cannot talk to a Unix
  socket, so this is the only in-process transport that needs no TCP port.

To verify in milestone 3, before building on them:

- Claude Code treats a bracketed paste containing newlines as a single paste
  and does not submit. This is the crux of every send backend.
- `wezterm cli send-text --pane-id` and `kitten @ send-text --match id:` do
  the equivalent of tmux `paste-buffer -p`. Neither terminal is installed on
  the development machine, so these backends ship behind the tmux and
  keystroke ones and are tested when a machine with them is available.
- Posting Cmd+V through CGEvent to an activated terminal app pastes with
  bracketed paste in iTerm2 and Ghostty. Requires the Accessibility
  permission for the peekback binary. macOS ties that grant to the code
  signature; an ad-hoc signed binary changes identity on every build, so dev
  builds are signed with a self-signed certificate created once in Keychain
  to keep the grant across rebuilds.
- `claude --resume` fires SessionStart again for the same session id. The
  hook must treat that as an update, not a fresh entry, and keep
  `started_at`.

## Components

### 1. Session registry (Claude Code hooks)

One hook script, `hooks/register.sh`, wired to four events, writes one file
per session:

    ~/.local/state/peekback/sessions/<session-id>.json
    {
      "session_id", "transcript_path", "cwd",
      "started_at", "last_active_at",
      "terminal": {
        "bundle_id",        // __CFBundleIdentifier
        "term_program",     // TERM_PROGRAM
        "tmux_pane",        // TMUX_PANE, if set
        "tmux_socket",      // socket path parsed from $TMUX, if set
        "kitty_window",     // KITTY_WINDOW_ID, if set
        "kitty_listen_on",  // KITTY_LISTEN_ON, if set
        "wezterm_pane"      // WEZTERM_PANE, if set
      }
    }

- SessionStart creates the entry with both timestamps set to now, or, if an
  entry already exists (a resumed session), refreshes everything except
  `started_at`.
- UserPromptSubmit and Stop update `last_active_at`.
- SessionEnd removes the entry.

Every write goes to `<session-id>.json.tmp` followed by `mv`, so a reader
never sees a half-written file. The session id, transcript path, and cwd come
from the hook's stdin JSON; the terminal block comes from the hook's
environment. Every field in the terminal block is optional. A session that dies without SessionEnd leaves a stale
entry; `last_active_at` sorts it to the bottom, and `peekback status --prune`
removes entries whose transcript has not changed in 24 hours.

This is the only piece that needs to be configured in Claude Code settings.
It is a few lines of shell and ships with a documented settings snippet.

### 2. CLI and daemon (one Rust binary, `peekback`)

Subcommands:

- `peekback show [FILE] [--session ID] [--pane ID]`
  Resolve the session: explicit `--session`, else the registry entry whose
  `tmux_pane` matches `--pane`, else `CLAUDE_CODE_SESSION_ID` from the
  environment, else the registry entry with the newest `last_active_at`.
  Flags beat the environment so a script run from inside one session can
  target another. Start
  the daemon if not running. Set the current document to FILE, or to the
  session's most recently written Markdown file. Bring the window forward.
- `peekback send [--session ID] [--pane ID] < text`
  Paste stdin into the session's prompt through the send backends. Resolves
  the session with the same chain as `show`. Used from scripts; the viewer
  window sends through the daemon directly and always targets the session it
  is showing.
- `peekback daemon`
  Run the window, the socket server, the watchers, and the global hotkey in
  the foreground. Normally started implicitly by `show`.
- `peekback status [--prune]`
  Print registry entries, current document, daemon state, which send backend
  each session would use, and which backend tools (`tmux`, `wezterm`,
  `kitten`) are found on the daemon's PATH.

The daemon is a single instance per user, holding: the current document path,
the session it belongs to, a file watcher on the current document, and a
directory watcher on the registry. It registers the global hotkey and, on
press, behaves as `peekback show` with no arguments.

Daemon lifecycle:

- State lives under `~/.local/state/peekback/`: `daemon.sock`, `daemon.lock`,
  `daemon.log`, and `sessions/`.
- On start the daemon takes an exclusive `flock` on `daemon.lock`. If the lock
  is held, another instance is running and this one exits. Otherwise it
  unlinks any stale `daemon.sock` and binds a fresh one. The lock, not the
  socket file, is the source of truth for "running".
- `show` and `send` connect to the socket. A connection error means not
  running; a stale socket file with nobody listening gives the same error, so
  no existence check is made. The CLI then spawns `peekback daemon` detached
  in its own session with stdio redirected to `daemon.log`, and retries the
  connection for up to three seconds. Two CLIs racing both spawn; the lock
  makes the second daemon exit immediately.
- The daemon inherits the environment of the shell that spawned it, including
  PATH. That is the shell where the user has `tmux` and friends installed, so
  backend tools resolve normally. `status` reports what it found so a
  mismatch is visible.

Two transports, one in-process and one over the socket:

- **Socket protocol** (CLI to daemon): newline-delimited JSON over the Unix
  socket, one request and one response per connection. Requests: `show`
  (session id, optional path), `send` (session id, text), `status`. No HTTP
  server.
- **Page transport** (webview to daemon, in-process): the page and its assets
  are served through wry's custom protocol at `peekback://app/`. The page
  sends `ready`, `switch` (session id, optional path), `send` (text), and
  `copy` (text) through `window.ipc.postMessage`. The daemon pushes `render`
  (document source, path, session, document list), `sessions` (live session
  list), `banner`, and `toast` through `evaluate_script`. No SSE and no TCP
  port; the page never opens a network connection.

Threading: tao owns the main thread and the event loop. The socket acceptor,
the document watcher, and the registry watcher each run on their own thread
and hand work to the event loop through an `EventLoopProxy` user event. The
global hotkey delivers on the same loop. Every mutation of daemon state and
every `evaluate_script` call happens on the main thread, so there is no shared
mutable state between threads beyond the proxy channel. No async runtime.

### 3. Send backends

A backend takes a registry entry and text, and returns which backend ran or
an error. The daemon probes in order and uses the first that applies:

1. **tmux**: entry has `tmux_pane` and `tmux_socket`, and
   `tmux -S <socket> display-message -p -t <pane>` confirms the pane exists on
   that server. Then `tmux -S <socket> load-buffer -b peekback -` and
   `tmux -S <socket> paste-buffer -p -b peekback -t <pane> -d`. Without the
   socket, a pane id could match a different pane on another server, so a
   missing `tmux_socket` disables this backend rather than defaulting.
2. **wezterm**: entry has `wezterm_pane` and `wezterm` is on PATH.
   `wezterm cli send-text --pane-id <id>` with the text on stdin.
3. **kitty**: entry has `kitty_window` and `kitty_listen_on`.
   `kitten @ --to <listen_on> send-text --match id:<id>` with the text on
   stdin.
4. **keystroke** (macOS): entry has `bundle_id`. Save the clipboard, put the
   text on it, activate the app by bundle id, post Cmd+V, restore the
   clipboard after a short delay. Requires Accessibility; if not granted, the
   backend reports that and the probe falls through.
5. **clipboard**: put the text on the clipboard and report it. The window
   shows a toast: copied, paste it into the prompt.

The user can pin a backend in `~/.config/peekback/config.toml`, for instance
to force `clipboard` when the injection is unwelcome:

    hotkey = "Cmd+Shift+M"      # global hotkey; empty string disables it
    backend = "auto"            # or tmux, wezterm, kitty, keystroke, clipboard
    placement = "right"         # right, left, over, or free
    split = 0.5                 # share of the terminal's width for right/left
    theme = "auto"              # or system: keep the page's own palettes
    # font = "JetBrains Mono"   # override the terminal font
    # font_size = 13

All keys are optional and these are the defaults. `peekback status` shows
the probe result per session so misconfiguration is visible before a send.

### 4. Viewer window

A wry/tao window hosting one page, served by the daemon. Rendering is entirely
client-side:

- markdown-it with GFM tables, task lists, footnotes.
- Mermaid for fenced `mermaid` blocks.
- KaTeX auto-render for `$...$` and `$$...$$`.
- highlight.js for fenced code.
- Block elements carry `data-source-line` from markdown-it token maps so a
  selection can be mapped back to source lines later.

All JS and CSS assets are vendored and embedded in the binary. No CDN, works
offline.

The window is a popup that borrows the terminal's space, not an app of its
own. With `placement` other than `free` it has no title bar, floats above
the terminal until dismissed, follows the user to whatever Space they are
on, and on every show it moves onto the terminal window: the right or left
`split` of it, or all of it for `over`. The terminal window is found through
the bundle id in the registry, which needs no permission. The daemon runs
with the Accessory activation policy, so there is no dock icon and no
menu bar switch. `free` gives an ordinary decorated window the user places.

Theme: with `theme = "auto"` the daemon reads the colors and monospace font
of the terminal the session runs in and pushes them to the page. iTerm2 is
read from its preferences plist, using the profile named by `ITERM_PROFILE`
in the registry; Ghostty from its config file and the theme file it names.
The page maps them onto its variables: background and foreground as they
are, ANSI blue as the accent, ANSI bright black as muted, and the code
highlighting classes onto the sixteen ANSI colors, so a snippet looks the
same in the viewer as in the terminal. Body text keeps the system
proportional font on purpose: the viewer should read as an extension of the
terminal, not an imitation of one. The terminal font goes on code, the
status line, the command line, and the picker. Unknown terminals fall back
to the page's own light and dark palettes, which follow the system.

Layout: the document fills the window. A status line along the bottom
shows the mode, the session and document, and messages. A sidebar with
sessions, documents, and pending comments can be toggled on; it is off by
default because the popup is narrow and the picker covers the same ground.
Light and dark follow the system.

- **Sessions**: live sessions, most recently active first, showing the
  project directory name and how long ago it was active. The current one is
  highlighted; clicking another switches the window to that session's newest
  document. This replaces a separate picker. The list updates live: the
  daemon watches the registry directory and pushes `sessions` on any change.
- **Documents**: the current session's documents, newest first, current one
  highlighted.
- **Comments**: the pending comments list.

A status line along the bottom shows the mode, the current document, and
the pending comment count, in the manner of vim.

Keyboard model, active whenever the viewer window has focus. The unit is the
block: every paragraph, heading, list item, code block, table, and blockquote
carries `data-source-line` from markdown-it, so a block selection maps to an
exact source line range.

- **Normal.** `j`/`k` move the block cursor, drawn as a bar in the left
  margin of the current block, with counts like `5j` and `12G`. `d`/`u`
  half page, `gg`/`G` top and bottom, `zz`/`zt`/`zb` scroll the cursor
  block to the center, top, or bottom. `]]`/`[[` next and previous heading.
  `]d`/`[d` cycle documents, `]s`/`[s` cycle sessions. `y`, `s`, `c` act on
  the block under the cursor. `Tab` toggles the sidebar. `?` shows a key
  overlay. `Esc` or `:q` leaves: hides the window and returns focus to the
  application the user came from.
- **Picker.** `Space d` (or `Ctrl-P`) and `Space s` open an fzf-style
  overlay over documents or sessions: type to filter, `Ctrl-N`/`Ctrl-P` or
  arrows to move, `Enter` to open, `Esc` to close. `:doc` and `:session`
  without an argument open the same picker. This is the primary way to move
  between documents and sessions; the sidebar is off by default.
- **Visual.** `v` anchors a selection at the cursor and `j`/`k` extend it
  over blocks. `y` copies, `s` sends, `c` comments; each returns to normal.
  `Esc` cancels. The selection survives opening the command line or the
  note input, so `:c note` acts on it, as `:'<,'>` would in vim.
- **Comment.** `c` opens a one-line note input for the block or selection;
  `Enter` adds the quote and note to the pending list, `Esc` abandons.
  `Space c` opens the pending comments in the picker: `Enter` jumps to the
  quoted block, `Ctrl-D` removes one. `S` or `:sendall` sends them all as
  one prompt and clears the list only after the backend reports success.
  The status line shows the pending count.
- **Search.** `/` opens a search field; matches highlight as you type,
  `Enter` moves the cursor to the first match, `n`/`N` step through them.
- **Command.** `:` opens a command line with completion over documents and
  sessions. Commands: `:send`, `:c <note>` to comment on the current
  selection with the note, `:sendall`, `:doc [name]`, `:session [name]`,
  `:sidebar`, `:q`, `:help`.

Mouse selection: when text is selected with the mouse, a small floating bar
offers the same three actions. A mouse selection sends rendered text, since
it can cut through the middle of a block.

- **Copy** puts the selection on the clipboard. A block selection copies the
  Markdown source; a mouse selection copies plain text.
- **Send** pastes the selection into the session prompt as a blockquote.
- **Comment** opens a one-line input; on Enter the quote and note are added to
  the pending comments list.

Pending comments panel: each entry shows the quote and the note, can be
removed, and a single **Send all** button pastes every pending comment as one
prompt, then clears the list. Comments are not persisted; closing the window
loses them, which is acceptable at this scope.

A toast area reports the result of each send: which backend ran, or the error
if none could.

### 5. Triggers

Default, needing no terminal configuration:

- Global hotkey, registered by the daemon. Default `Cmd+Shift+M`, set in
  `config.toml`. Means "enter the viewer": shows the window on the most
  recently active session if it was hidden, and gives it keyboard focus.
  Leaving is `Esc` or `:q` inside the viewer, which hides the window and
  hands focus back to the previous application.
- `peekback show` from any shell, including `! peekback show` inside a Claude
  Code prompt, which resolves the session exactly through the environment.

Optional, documented for users who want exact pane resolution from a
keybinding:

    # tmux.conf
    bind-key P run-shell -b "peekback show --pane '#{pane_id}'"

    # ghostty config: type the command into the focused terminal
    keybind = cmd+shift+p=text:! peekback show\n

The `!` route leaves a line in the transcript each time, which is why the
hotkey is the default.

## Document discovery

For a session, the document list is built from three sources, deduplicated,
newest first by the time the session last touched the file:

1. The transcript JSONL: every assistant `tool_use` block named `Write` or
   `Edit` whose `file_path` ends in `.md`, in order of appearance. This is the
   authoritative "what this session produced".

   Record shape, verified against a real transcript: one JSON object per line
   with top-level `type` (`"assistant"` for these), `timestamp` (ISO 8601),
   `sessionId`, `cwd`, and `message` with `role` and a `content` array. Tool
   calls are `content` items with `type: "tool_use"`, `name`, and `input`;
   for `Write` the input has `file_path` and `content`, for `Edit` it has
   `file_path`, `old_string`, `new_string`. Lines of other types
   (`attachment`, `user`, and so on) have no `message` or a non-object one and
   are skipped. Paths in `file_path` are absolute.
2. `*.md` under the session's scratchpad directory.
3. `*.md` in the project's memory directory.
4. `*.md` under the session's `cwd`, modified after the session's
   `started_at`, to catch files written through Bash (heredocs, `sed -i`,
   scripts) that never appear as a `Write` or `Edit` tool call. The walk
   skips `.git`, `node_modules`, `target`, and hidden directories, and stops
   at depth 4. This source is a heuristic and can include files the user
   edited by hand during the session, which is acceptable: they are still
   files the user may want to look at.

Files that no longer exist are dropped. The transcript is re-read on each
`show` and each sidebar switch, not watched.

## Paste formats

Send selection:

    > first line of the selection
    > second line

Send all comments:

    Comments on `relative/path/to/file.md`:

    > quoted passage
    the note

    > another quoted passage
    another note

The relative path is relative to the session's `cwd` when the file is under
it, otherwise absolute.

## Behaviour details

- Single daemon, single window. `show` from a second session switches the
  window to that session's document; the sidebar follows.
- Live reload replaces the rendered document in place and keeps scroll
  position when the change is below the viewport.
- If the current document is deleted, the window shows the last rendered
  content with a banner, and the next `show` replaces it.
- If the registry is empty, `show` fails with a message that names the hook
  to install. If it has entries but none match `--session` or `--pane`, the
  message lists what is registered.
- If every backend fails, `send` returns the last error and the window shows
  it; nothing is silently dropped. The clipboard backend cannot fail in
  practice, so this means the clipboard itself is unavailable.
- Window focus: `show` brings the window forward but does not steal keyboard
  focus from the terminal, since it is the agent-triggered path. The hotkey
  and a sidebar click do focus the viewer. IPC backends (tmux, wezterm,
  kitty) leave focus in the viewer after Send; `Esc` returns to the
  terminal. The keystroke backend necessarily
  activates the terminal, so after Send the terminal is frontmost with the
  text in the prompt; this is stated in the toast.
- The keystroke backend targets whichever split or tab is focused in the
  terminal app. That is nearly always the one the user came from. The toast
  makes a wrong landing visible immediately.

## Stack

- Rust. wry and tao for the window, global-hotkey for the trigger, notify for
  file and directory watching, std `UnixListener` with serde_json for the
  socket protocol, serde for the registry and transcript, arboard for the
  clipboard, core-graphics for the macOS keystroke backend. No HTTP server
  and no async runtime.
- Vendored front end: markdown-it, mermaid, katex, highlight.js, embedded with
  `include_bytes!`. One HTML file, one CSS file, one JS file of our own.
- Shell for the hook.
- Distribution: a single binary in `~/bin`, later a Homebrew tap.

## Milestones

1. Daemon plus window rendering a fixed file with live reload, no session
   awareness. Proves wry, the custom protocol and IPC transport, the asset
   pipeline, the daemon lifecycle (lock, socket, spawn from `show`), and the
   threading model: watcher and socket threads feeding the tao event loop.
2. Registry hook, `show` with session resolution, transcript discovery,
   sidebar with sessions and documents, global hotkey. Proves the trigger end
   to end without any terminal configuration.
3. Selection and send. Verify bracketed paste behaviour in Claude Code. Mode
   state machine, block cursor, visual mode, search, command line with `:q`,
   `:doc`, `:session`, `:send`, `:help`. Mouse selection toolbar. Copy and
   Send with source-line mapping. Backends: clipboard, tmux, keystroke (with
   the dev signing certificate). wezterm and kitty backends written to spec
   but marked untested until run on a real install.
4. Comments: `c`, `:c <note>`, the pending comments panel, `:sendall`.
5. `status --prune`, optional keybinding docs, README.

## Open questions

- Whether `show` should also accept a directory and default to its newest
  `.md`, for use outside any session.
- Whether the keystroke backend should verify the frontmost window title
  contains something recognisable before posting Cmd+V, to reduce wrong
  landings, or whether the toast is enough.
- Whether the hook should also record the terminal's window or tab id from
  `ITERM_SESSION_ID` for a future iTerm2 backend through its Python API.

## Later, if the loop proves itself

- Persist comments in a sidecar next to the document, anchored by quote and
  context, re-anchored after edits.
- An MCP server on the daemon so an agent can list, reply to, and resolve
  comments directly.
- Threads and a "sent to agent" state per comment.
- A UserPromptSubmit hook that attaches pending comments as additional
  context to the next prompt, as an alternative to injection that needs no
  terminal cooperation at all. Left out of v1 because the user would not see
  the text in the prompt before sending.
