# peekback

Browse files touched by Claude Code and Codex CLI sessions in your terminal,
then open Markdown in a native preview. Select text in the viewer and copy it,
send it to the session's prompt, or attach a note and send that.

You peek at what the agent wrote. What you mark up goes back.

The terminal browser covers all file types and keeps observed history after
sessions end. The Markdown viewer adds Mermaid, KaTeX, syntax highlighting,
live reload, and vim-style navigation. It takes its colors from your terminal
and uses normal window stacking. The app supports Apple Silicon macOS;
the separately published [session-activity library](https://crates.io/crates/session-activity)
also supports Linux.

[Cargo package](https://crates.io/crates/peekback) ·
[Library API](https://docs.rs/session-activity) ·
[Changelog](CHANGELOG.md)

## Install

Apple Silicon macOS 14 or newer, Rust 1.87+, and Xcode command-line tools are
required. The frontend is embedded; Node.js and separate hook scripts are not
needed. Intel, Linux, and Windows are not supported by the app in this release.

```sh
cargo install peekback --locked
peekback setup
```

Cargo installs the binary in `~/.cargo/bin` by default; ensure it is on PATH.
`setup` detects Claude Code and Codex from their configuration directories or
executables. To choose an agent explicitly:

```sh
peekback setup --agent codex
peekback setup --agent claude
```

Start or resume your agent. **In Codex, open `/hooks` and review and trust the
Peekback hooks.** Then send a prompt and run:

```sh
peekback browse
```

Press `p` on a Markdown file to open its rendered preview.
Use `peekback browse --all-sessions` to browse retained files across sessions.

### Install from a checkout

To build the current repository version:

```sh
git clone https://github.com/tskupinski/peekback.git
cd peekback
cargo install --path . --locked
peekback setup
```

Maintainer instructions are in [RELEASING.md](RELEASING.md).

### Setup details

`peekback setup --dry-run` shows the target files and hook commands without
writing. `--agent all` configures both agents even before they are detected.

Setup merges hooks into `~/.claude/settings.json` and `~/.codex/hooks.json`,
respecting `CLAUDE_CONFIG_DIR` and `CODEX_HOME`. It preserves unrelated settings
and hooks, updates its own entries on repeat runs, and uses the absolute path
to the installed binary. No shell script, clone, or agent-shell PATH is needed.
Changed files get a private, uniquely named `.peekback-backup-...` copy next to
them before an atomic replacement. Invalid JSON or unexpected hook shapes
cause an error. Symlinked settings files are left untouched; configure those
manually. Close settings editors while running setup.

If you previously installed `hooks/register.sh` entries manually, remove those
old Peekback entries when switching to setup; manually installed hooks are
preserved like other user hooks. Project-local or inline TOML hooks are also
left untouched. Existing hook-disable settings and Codex trust choices are
preserved. See the [Codex hook documentation](https://learn.chatgpt.com/docs/hooks)
and [Claude hook reference](https://code.claude.com/docs/en/hooks).

Hooks register sessions, retain observed file activity, and remove live entries
when sessions end. `peekback status` shows registered sessions and their terminal
backends. Project Markdown modified since session start is also discovered;
Codex shell commands and transcripts are not parsed. Use
`peekback show /absolute/path/to/file.md --session ID` for files outside the scan.
Codex desktop/IDE composer integration is not supported.

### Upgrade

Send or copy pending comments before stopping the daemon; they are kept only
in memory.

```sh
cargo install peekback --locked
peekback setup
peekback quit
```

The next `peekback show` or `p` in the terminal browser starts the new daemon
and restores the global hotkey. Review changed Codex hooks through `/hooks`
if prompted. Rerun setup if you move the executable.

If an upgrade seems to have no effect, run `type -a peekback` and
`peekback --version`. An older copy in `~/bin` or `~/.local/bin` may come before
Cargo's binary on PATH. Put your Cargo bin directory first, rerun setup with
that binary, and restart the daemon. See [Cargo's installation reference](https://doc.rust-lang.org/cargo/commands/cargo-install.html)
for custom install locations.

### Prebuilt archives

The current release is distributed through Cargo. No prebuilt GitHub release
is published yet. Maintainers can build an optional unsigned Apple Silicon
archive using [RELEASING.md](RELEASING.md).

## Use

After a session appears in `peekback status`, run `peekback show` once. This
starts the daemon and registers the global hotkey; installing hooks or running
`status` alone does not start it. `peekback browse` also starts the viewer when
you press `p` on a Markdown file. A session that has not produced Markdown yet
shows an empty viewer that opens its first document when one appears; use
`peekback show README.md` to open a specific document meanwhile.

While the daemon is running, press `Cmd+Shift+M` from anywhere. The viewer appears over the right half of
your terminal window, showing the newest Markdown file of the session in the
tmux pane you last used, else of your most recently active session, and takes
keyboard focus. Press `Esc` to drop it and return
to the terminal.

The viewer uses normal window stacking: switching to another app lets that
app cover it. It comes to the front when explicitly shown or focused.

From inside a Claude Code prompt, `! peekback show` opens the viewer on that
exact session and focuses it; `Esc` returns to the terminal. Add `--no-focus`
to bring it forward without taking keyboard focus, for scripts or agents that
should not interrupt your typing. `peekback show path/to/file.md` shows a
specific file.

For Codex CLI, use the global hotkey, a tmux binding, or `peekback show
--session ID` from another shell. When Codex runs `peekback show` itself,
Peekback uses `CODEX_THREAD_ID` (or `CODEX_SESSION_ID`) to select that session.

### Keys

| Key | Action |
| --- | --- |
| `j` / `k` | move the block cursor, with counts like `5j` |
| `Ctrl-D` / `Ctrl-U` | half page down / up (also `d` / `u`) |
| `Ctrl-F` / `Ctrl-B` | full page down / up |
| `gg` / `G` | top / bottom |
| `zz` / `zt` / `zb` | scroll the cursor block to center / top / bottom |
| `]]` / `[[` | next / previous heading |
| `]d` / `[d` | next / previous document |
| `]s` / `[s` | next / previous session |
| `v` | visual mode, extend with `j` / `k` |
| `y` | copy the block or selection as Markdown |
| `s` | send the block or selection to the prompt as a blockquote |
| `c` | comment: add a note to the block or selection |
| `S` | send all pending comments as one prompt |
| `/` | search, then `n` / `N` |
| `Space d` or `Ctrl-P` | document picker |
| `Tab` / `Shift-Tab` in document picker | cycle Current session, All sessions, and Bookmarks |
| `Space s` | session picker |
| `Space c` | pending comments (`Enter` jumps, `Ctrl-D` removes) |
| `:` | command line: `:doc`, `:session`, `:send`, `:c note`, `:sendall`, `:sidebar`, `:q`, `:help` |
| `Tab` | toggle the sidebar |
| `?` | key overlay |
| `Esc` | cancel, clear search, or leave the viewer |

Text selected with the mouse gets a small toolbar with the same Copy, Send
and Comment actions.

Blocks with pending comments are highlighted and show a comment-count badge.
Hover over a marked block to read its notes, or use `Space c` to open the pending
comments picker. Markers disappear when comments are removed or successfully
sent. If live edits change the text at a comment's saved lines, its marker is
hidden; the original quote and note remain in the pending list. Comments stay
in memory until the daemon stops.

The document picker has **Current session**, **All sessions** and
**Bookmarks** scopes; `Tab` and `Shift-Tab` cycle through them. All
sessions reads retained tracker history for both agents, including ended
sessions. It groups identical paths, sorts by latest activity, and shows the
agent/session origins; filter by path or session ID. It keeps the viewer's
existing rules: existing Markdown files, excluding reads and failed operations.
The All sessions scope refreshes when opened; `Ctrl-R` refreshes it while open.
Incomplete history is reported in the picker. Selecting a file from this scope
keeps the viewer in the session it was opened for: Current session still lists
that session's files, and selections and comments are still sent to it. A file
never picks the session itself, since it can belong to several. A viewer opened
without a session shows the file standalone, with no send-back target.

**Bookmarks** lists Markdown you always want within reach, such as your global
`CLAUDE.md` or shared snippets, configured with `bookmarks` (see
[Configuration](#configuration)). Opening a bookmark also keeps the viewer's
session, so you can ask that agent to edit the file. The list is rebuilt from
the config each time the scope opens, and `Ctrl-R` refreshes it.

Nothing is ever submitted for you. Every send lands in the prompt as a paste
and waits for you to press Enter. Control characters are stripped before
sending. If the tracked agent process is no longer running, automatic sending
falls back to the clipboard; a pinned paste backend refuses the send. Entries
created before process tracking retain their previous behavior until fresh
hook activity records the agent's identity.

### Commands

```
peekback setup [--agent claude|codex|all] [--dry-run]  configure hooks
peekback show [FILE] [--session ID] [--pane %N] [--no-focus]  show a document
peekback browse [--session ID] [--candidates]        browse session files
peekback browse --all-sessions [--list]              browse retained history
peekback send [--session ID] [--pane %N] < text       paste text into a prompt
peekback hide                                       hide the viewer
peekback status [--prune]                            sessions, backends, daemon
peekback quit                                       stop the daemon
```

`peekback --help` and `peekback COMMAND --help` list all options.
`:q` inside the viewer hides its window; `peekback quit` stops the daemon
and unregisters the global hotkey.

Session resolution for `show`, `send`, and `browse`: `--session`, else the session in
the given tmux pane, else the session this shell runs inside, else the most
recently active one.

`PEEKBACK_STATE_DIR` optionally overrides `~/.local/state/peekback`. Set it
consistently for both the agent hooks and Peekback when using an isolated
registry and daemon.

### Browse files in the terminal

```sh
peekback browse
peekback browse --session SESSION_ID --candidates
peekback browse --agent codex --session ENDED_SESSION_ID
peekback browse --list
peekback browse --all-sessions
peekback browse --all-sessions --list
```

The interactive browser shows all file types, including reads, failed operations,
missing files, and rename origins. The file list and text preview appear side by
side in wide terminals; narrow terminals show the focused pane. `Tab` switches
between the list and preview in either layout. It uses the
same `session-activity` library as the Markdown viewer. Browsing does not start
the viewer daemon; press `p` on a Markdown file to open its rendered preview.
Live sessions keep the connection for sending selections back. Files from ended
sessions open as standalone documents. With `--all-sessions`, previews stay in
the live session the browser runs inside, if any, and are standalone otherwise.

`--all-sessions` browses retained history without requiring any live session.
It merges paths across Claude Code and Codex, preserves session provenance in
the evidence view (`e`) and plain listing, and supports filtering by session ID.
`r` reloads stored history. Previews keep the browser's originating live session
as their send-back target when one is available. It cannot be combined with
session/pane selectors or `--candidates`; it reads the tracker rather than
scanning every old project.
Files found only by a live filesystem scan may therefore be absent from All
sessions. Installing hooks does not automatically import every past session.

| Key | Action |
| --- | --- |
| `j` / `k`, arrows | Move through files or scroll the focused preview |
| `Tab` | Switch between file list and preview |
| `/` | Filter paths as you type; `Enter` or `Esc` finishes editing |
| `c` | Clear the filter |
| `p` | Open the selected `.md` or `.markdown` file in Peekback |
| `t` / `e` / `w` | Show current text, event evidence, or scan/storage warnings |
| `r` | Refresh the file list and current preview |
| `PgUp` / `PgDn`, `g` / `G` | Page or jump to the beginning/end of the focused pane |
| `q`, `Esc`, `Ctrl-C` | Exit (while editing a filter, `Esc` only finishes editing) |

`tool` means a hook or transcript observed an operation, not necessarily a
successful write; `e` shows the recorded operation and outcome. `candidate`
means scan evidence only. Candidates are opt-in with `--candidates`; warnings
are available through `w`. The preview reads current UTF-8 text, up to 256 KiB;
it is not a historical snapshot. Binary, missing, and special files show an
explanation instead. Refresh is manual with `r`.

Use `--agent claude|codex --session ID` to browse retained history after a session
ends. `--list`, or piping the command, produces a plain table without terminal
controls. For structured output, use `peekback activity files` below.

### Query session file activity

The collector also works without opening the viewer. Queries return JSON and
include all file types, deleted paths, rename origins and destinations, and
the evidence behind each file:

```sh
peekback activity files --agent codex --session SESSION_ID
peekback activity events --agent claude --session SESSION_ID
peekback activity files --agent codex --session SESSION_ID --candidates
```

`events` returns the retained event history. Damaged batches are skipped with
an explicit warning on stderr (or in the daemon log), allowing healthy history
to remain available. `files` groups events by path, collapses retries with the
same tool call ID and evidence source, reconciles known outcomes, and
also uses Claude transcripts and legacy records while a session is registered.
`--candidates` adds the bounded filesystem scan for a registered session;
its results are labelled as scan observations. Peekback's viewer includes
candidates and filters the result to existing Markdown files. Budget exhaustion,
depth limits, and filesystem read errors are reported on stderr or in the daemon
log, so an incomplete scan is visible.

Hook events are retained under `~/.local/state/peekback/activity/AGENT/SESSION_ID/`
after the session exits or is pruned. No automatic retention limit is applied.
Events contain file metadata, never prompts, file bodies, or raw tool
output. Outcomes are `succeeded`, `failed`, or `unknown`; an unfamiliar tool
result remains `unknown`. Claude transcript results are matched to requests
by tool call ID. Shell commands aren't parsed for file operations.

Registry updates and pruning are serialized per session. A durable end marker
keeps late tool hooks from reopening an ended or pruned session while still
retaining their file events. An explicit `SessionStart` reopens the session.
Hooks do not provide a reliable process generation, so an old start/end hook
cannot always be distinguished from one belonging to a resumed session.

### Maintain file history

Preview compaction or retention for one session:

```sh
peekback activity compact --agent codex --session SESSION_ID
peekback activity retain --agent codex --session SESSION_ID --before UNIX_SECONDS
```

Both commands return a JSON report; add `--apply` to commit. Compaction packs
history into fewer files while preserving every raw event. Retention removes
events strictly older than the Unix timestamp in seconds and persists that
cutoff, preventing old transcript events from being reimported. Events at the
cutoff are retained. A later command cannot lower an existing cutoff, so
cutoffs in the future are rejected.

Maintenance refuses damaged history and unexpected files in a session's
history directory. It publishes a synced checkpoint before
removing old batches, so interrupted cleanup does not duplicate events. Readers,
writers, and maintenance coordinate through advisory locks on Unix. All readers
must support checkpoints before applying maintenance; development builds from
before this feature cannot read compacted history. Peekback 0.1.0 supports it.
Queries still read the full retained history; compaction reduces file count,
not memory use.

## Rust library

The separately published [`session-activity` library](https://crates.io/crates/session-activity)
owns event normalization, storage, scanning, and file queries. It has no
dependency on Peekback's viewer, Markdown renderer, or terminal integration.

```sh
cargo add session-activity@0.1
```

See the [library guide](https://github.com/tskupinski/peekback/tree/master/crates/session-activity)
for examples, evidence semantics, storage guarantees, and limitations, or the
[API reference](https://docs.rs/session-activity). Using the library does not
install Peekback or configure agent hooks.

## Configuration

`~/.config/peekback/config.toml`, every key optional:

```toml
hotkey = "Cmd+Shift+M"   # empty string disables it
backend = "auto"         # or tmux, wezterm, kitty, keystroke, clipboard
placement = "right"      # right, left, over, or free
split = 0.5              # share of the terminal's width for right / left
theme = "auto"           # or system: keep the page's own light / dark palettes
# font = "JetBrains Mono"
# font_size = 13
bookmarks = [
  "~/.claude/CLAUDE.md",   # starts with ~ or /: the same file everywhere
  "~/notes/snippets/",     # a folder: every Markdown file in it, recursively
  "CLAUDE.md",             # anything else: relative to the session's project
  "docs/**/*.md",          # globs: *, **, ?, [abc] and {a,b}
]
```

The daemon reads the file when it starts; run `peekback quit` after editing.
`bookmarks` is the exception and is read whenever the Bookmarks scope opens.

Bookmarks list only existing Markdown files, in config order, without
duplicates. A relative entry is skipped when the viewer has no session, and a
missing file is simply left out, so `CLAUDE.md` works in projects that lack
one. Folders and globs skip hidden files and folders and do not follow
symlinked folders; symlinked files are followed. The list stops at 200 files,
and entries that cannot be listed, such as an invalid glob, show a warning in
the picker.

### How text reaches the prompt

peekback probes, in order: tmux (using the pane and server socket recorded
by the hook), WezTerm and Kitty remote control, macOS keystroke injection
(clipboard plus Cmd+V into the terminal app, needs the Accessibility
permission), and finally the clipboard with a toast asking you to paste.
tmux refuses multi-line text when the program in the pane has not enabled
bracketed paste, because tmux would type each newline as Enter.
`peekback status` shows the result of the probe per session. WezTerm and
Kitty are implemented to their documented interfaces but have not been
exercised on a real install yet.

### Theme

With `theme = "auto"`, colors and the monospace font come from the terminal
the session runs in. iTerm2 is read from its preferences, Ghostty from its
config and theme file. Body text keeps the system font; the terminal font
goes on code and the chrome. Unknown terminals get the built-in light and
dark palettes, following the system.

### Optional terminal keybindings

The hotkey needs no terminal setup. For exact pane resolution from tmux:

```tmux
bind-key P run-shell -b "peekback show --pane '#{pane_id}'"
```

## How it works

A single daemon per state directory holds one window. `show` talks to it over a Unix
socket and starts it if needed. The page is served to the web view over a
custom `peekback://` scheme from assets embedded in the binary, so nothing
listens on a TCP port and it works offline. Documents are discovered from the
Claude session transcript, its scratchpad, the project's memory directory,
Codex file-edit hooks, and Markdown under the project modified since the
session started. See [SPEC.md](SPEC.md) for the architecture and tracking limits.

## Development

From a checkout on Apple Silicon macOS:

```sh
cargo build --locked
bash scripts/check.sh
```

The repository pins Rust 1.87.0. Full checks also need Python 3.9+ and Node.js;
these are not runtime dependencies. Checks cover Rust tests and Clippy,
frontend picker behavior, license manifests, and an isolated terminal smoke
test with a fake viewer socket. On Linux, work on the library with
`cargo test -p session-activity --locked`.

See [RELEASING.md](RELEASING.md) for Cargo publication, standalone library
verification, and optional archive packaging.

## License

Apache License 2.0. See [LICENSE](LICENSE). The embedded front-end libraries carry
their own licenses, listed in [NOTICE](NOTICE).
