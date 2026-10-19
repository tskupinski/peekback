# Changelog

## 0.3.0 - 2026-10-19

Read what a session wrote, comment on it and send the comments back without
leaving the keyboard. Requires session-activity 0.3.0. Breaking: `peekback
browse` is now `peekback debug browse` and `--all-sessions` is gone, the
WezTerm and Kitty backends are removed, the viewer's sidebar is gone, and
`setup --all` replaces `--agent all`.

### Viewer

- Start the viewer when a session starts or resumes, so the hotkey works after
  install and after a reboot without running `peekback show` first. `peekback
  quit` stops it until the next session starts, and `setup` now ends by
  pointing at the hotkey.
- Show one session at a time. The sidebar is gone, with its session and
  document lists, the session picker (`Space s`, `:session`), `]s` / `[s`,
  `Tab` and `:sidebar`. The document picker (`Ctrl-P`) and the comments picker
  (`Space c`) list the same things from the keyboard. To look at another
  session, press the hotkey in its pane or run `peekback show --session ID`.
- Replace terminal color detection with built-in light and dark themes, in
  Peekback's own palette. `theme = "auto"` follows the system; `"light"` and
  `"dark"` pin a theme and the window's appearance. Terminal palettes failed
  text contrast in most themes. `theme = "system"` still works and means
  `auto`.
- Follow links from the keyboard with `gx`: the one in the cursor's block, or
  a list to pick from when the block has several. Links were mouse-only.
- Let the help overlay scroll, keep the status message readable when space is
  short, and clear a search when the viewer changes session.

### Comments

- Write comments in a note box that opens under the commented block, with
  several lines (`Shift-Enter`), instead of a one-line input at the bottom
  edge. `C` edits and `x` removes the comment on the cursor's block; comments
  could only be removed before. An empty note adds nothing, and emptying a
  comment removes it.
- `u` undoes removing a comment or discarding a written note, and each says so.
  `u` was a second half-page-up key.
- Mark each comment with one bar in the left margin beside its blocks, in
  comments' own color, instead of a badge under every block and a tinted
  background. A range comment no longer looks like several, and adding one no
  longer moves the text.
- Step through comments with `]c` / `[c`, which show each note in the status
  line, so notes can be read without hovering. The status line shows how many
  are pending, and how many changed, with `S send`.
- Review before sending: `S` opens a sheet with the target session and exactly
  what will be pasted, scrolled with `j` / `k`; `Enter` sends and `Esc` closes.
- Keep comments with their session. A resume keeps them, even after the agent
  exited while you were elsewhere; opening another session parks them, and
  coming back restores them with a notice. Previously a resume left them in
  memory where they could never be shown or sent again.
- Follow commented text that moves, including when the agent adds another
  copy of it. A comment whose text changed or was removed stays reachable with
  `]c` and is sent with a note saying so. Previously such comments vanished.
- Comments leave once handed off, pasted or copied to the clipboard when a
  paste is not possible, so they are never sent twice; `u` brings the batch
  back if the paste never reached the prompt.

### Sending

- Tell the agent where each comment and sent selection is, as a `path#L12-18`
  reference before the quote, and put a blank line between each quote and its
  note, so the note is not read as part of the quote.
- Sending (`s`) or copying (`y`) a selection includes the comments inside it,
  without repeating their quotes.
- Send to the pane the agent actually runs in when multiplexers are nested.
  A terminal window opened from tmux inherits `TMUX_PANE`, so text went to the
  tmux pane instead, and the hotkey and `--pane` could open it for the tmux
  pane's own session. A pane on another tty than the agent's is no longer
  recorded.
- Remove the WezTerm and Kitty backends. They were never exercised on a real
  install and could not verify that a pane belongs to the agent, so automatic
  sends through them already used the clipboard. tmux is the supported
  multiplexer; other terminals use the clipboard, or `backend = "keystroke"`.
  A config naming `wezterm` or `kitty` reports that and uses `auto`.
- Sessions recorded by an earlier version send through keystroke or the
  clipboard until their next prompt or tool call re-records the pane.

### Sessions and panes

- Open the hotkey on the tmux pane you are actually in. The daemon inherited
  the pane of whichever terminal started it, and tmux answered every "which
  pane is active" query for that pane's window, so the hotkey kept opening a
  session from that window. The active pane is now the one of the tmux
  server's most recently used client, and a server with no client attached
  has none.
- Look up `--pane` on the caller's tmux server. Pane ids repeat across tmux
  servers, so it could open a session from another server.
- Open the hotkey on a fresh Codex or Claude Code pane even before its agent
  registers a session. Codex creates one only at its first prompt, so the
  hotkey opened another session; it now shows an empty view for that pane and
  switches to its session once it starts.

### Command line

- Move the terminal browser and the activity queries under `peekback debug`
  (`debug browse`, `debug files`, `debug events`, `debug compact`, `debug
  retain`), for finding out why a file is or is not listed. The hooks'
  `peekback activity record` keeps working and is hidden from help.
  `browse --all-sessions` is gone: it showed every session's files, which the
  viewer never does.
- `peekback setup --all` replaces `--agent all`.

## 0.2.0 - 2026-09-25

Session files are now decided when each turn ends and stored, instead of
being guessed from the filesystem whenever a view opens. Requires
session-activity 0.2.0. Breaking: `--candidates` is removed, and history this
version writes cannot be read by Peekback 0.1.x, which skips it with warnings.

- Switch the viewer to a session even before it writes Markdown. It showed an
  error and kept the previous session's document, file list and send target, so
  a fresh session looked like it held the old session's files and sends went to
  the old session. The viewer now shows an empty state and opens the session's
  first document when it appears.
- Open the hotkey on the session in the tmux pane you last used, falling back
  to the most recently active session.
- Decide which files belong to a session when each turn ends, and store it.
  Previously every view scanned the project for files modified since the
  session started, so a session listed files other sessions and your editor
  changed in the same directory, a file you edited after the agent wrote it
  could drop out, and resuming an old session widened the window to days.
  Now the files a turn's scan finds are kept with the session, so they stay
  listed after later edits and after resuming, and `browse --all-sessions`
  shows them too.
- Capture files written by subagents from their transcripts, since some Claude
  Code versions do not report subagent tool calls to hooks.
- Mark files found only by a scan, and those found while another session was
  working in the same directory, in the viewer and terminal browser.
- Skip other git checkouts when scanning, so a session in a repository does
  not pick up files from worktrees or separate clones inside it. Submodules
  stay part of the project.
- End an interrupted or abandoned turn at the agent's last activity instead of
  at the next prompt, so a session left idle after Esc neither claims files
  written meanwhile nor marks other sessions' files as shared. Resuming after a
  crash and `status --prune` now capture the turn the dead agent left open.
- Remove `--candidates` from `browse` and `activity files`; every view now
  reads stored history.
- Replace the picker's All sessions scope with Scratchpad, which lists the
  Markdown in the current Claude Code session's scratchpad, newest first, live
  when the scope opens. All sessions mixed every session's history into one
  list; `peekback browse --all-sessions` still browses it in the terminal.
- Treat a session whose agent process has exited as ended, so an agent killed
  without `SessionEnd` no longer lingers in the session list, the hotkey
  fallback and session resolution. `peekback status --prune` now removes such
  entries right away instead of after 24 hours idle.

- List the viewer in Cmd+Tab and the Dock while it is shown, so it is one
  keystroke away from the terminal. A hidden viewer stays out of both.

## 0.1.1 — 2026-09-24

Updates the package README, which still described the crates as unpublished.

- Highlight blocks with pending comments and show comment counts and notes on
  hover. Keep selections attached to their original document while composing.
- Keep the viewer on its embedded page: block navigation, new windows and
  dropped items, ignore page messages from other origins, and route Mermaid's
  SVG links through the link handler.
- When a tracked agent process has exited, use the clipboard instead of pasting
  into its terminal. Strip control characters from sent text, refuse multi-line
  tmux pastes into panes without bracketed paste, and use a separate tmux buffer
  per send.
- Fix Kitty sends, which passed `--bracketed-paste` without a value.
- Stay in the session the viewer was opened for when opening a file from All
  sessions. Current session keeps listing its files, and sends still go to it.
  Previously the viewer dropped the session, emptying Current session.
  `peekback browse --all-sessions` likewise keeps the session it runs inside.
- Add a Bookmarks scope to the document picker, configured with `bookmarks` in
  `config.toml`: files, folders or globs, absolute or relative to the session's
  project. It rereads the config on every open, and opening a bookmark keeps
  the viewer's session as the send target.
- Focus the viewer when `peekback show` or `p` in the terminal browser opens
  it, as the hotkey does. `show --no-focus` keeps the previous behavior.
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
