# peekback

A rendered Markdown viewer for Claude Code and Codex CLI sessions, with a way back: select
text in the viewer and copy it, send it to the session's prompt, or attach a
note and send that.

You peek at what the agent wrote. What you mark up goes back.

peekback is a small Rust daemon with a native web view. It knows which Claude
Code and Codex sessions are live, lists the Markdown each one wrote, renders it with
real typesetting, Mermaid, KaTeX and syntax highlighting, and pastes what you
select back into the session's prompt. It sits over your terminal window like
a popup, takes its colors from your terminal, and is driven from the keyboard
with vim-style keys. macOS only for now.

## Install

Apple Silicon macOS 14 or newer, Rust 1.87+, and Xcode command-line tools are
required. The frontend is embedded; Node.js and separate hook scripts are not
needed. Intel, Linux, and Windows are not supported by the app in this release.

After the first crates.io publication:

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

### Before publication / installing from a checkout

From this repository, the equivalent installation works now:

```sh
cargo install --path . --locked
peekback setup
```

The public `cargo install peekback` command will only work after both crates
are published. Maintainer instructions are in [RELEASING.md](RELEASING.md).

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

To upgrade, rerun `cargo install peekback --locked`, then `peekback setup` and
`peekback quit` so the next preview starts the new daemon. Review changed Codex
hooks through `/hooks` if prompted. Rerun setup if you move the executable.

### Optional prebuilt archive

For users without Rust, the unsigned preview archive remains available when a
[GitHub Release](https://github.com/tskupinski/peekback/releases) is published.
Download it and SHA256SUMS, then:

```sh
shasum -a 256 -c SHA256SUMS
tar -xzf peekback-0.1.0-aarch64-apple-darwin.tar.gz
cd peekback-0.1.0-aarch64-apple-darwin
./install.sh
peekback setup
```

The installer uses `~/.local/share/peekback` with a binary symlink in
`~/.local/bin`; add that directory to PATH if needed. The downloaded binary is
not Developer ID signed or notarized and may be blocked by macOS. Cargo builds
locally. See [RELEASING.md](RELEASING.md) for the tested configurations.

## Use

After a session appears in `peekback status`, run `peekback show` once. This
starts the daemon and registers the global hotkey; installing hooks or running
`status` alone does not start it. `peekback browse` also starts the viewer when
you press `p` on a Markdown file. If no session has produced Markdown yet, use
`peekback show README.md` to open a specific document.

While the daemon is running, press `Cmd+Shift+M` from anywhere. The viewer appears over the right half of
your terminal window, showing the newest Markdown file of your most recently
active session, and takes keyboard focus. Press `Esc` to drop it and return
to the terminal.

The viewer uses normal window stacking: switching to another app lets that
app cover it. It comes to the front when explicitly shown or focused.

From inside a Claude Code prompt, `! peekback show` opens the viewer on that
exact session without focusing it. `peekback show path/to/file.md` shows a
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
| `Tab` in document picker | switch between current session and all stored sessions |
| `Space s` | session picker |
| `Space c` | pending comments (`Enter` jumps, `Ctrl-D` removes) |
| `:` | command line: `:doc`, `:session`, `:send`, `:c note`, `:sendall`, `:sidebar`, `:q`, `:help` |
| `Tab` | toggle the sidebar |
| `?` | key overlay |
| `Esc` | cancel, clear search, or leave the viewer |

Text selected with the mouse gets a small toolbar with the same Copy, Send
and Comment actions.

The document picker has **Current session** and **All sessions** scopes. All
sessions reads retained tracker history for both agents, including ended
sessions. It groups identical paths, sorts by latest activity, and shows the
agent/session origins; filter by path or session ID. It keeps the viewer's
existing rules: existing Markdown files, excluding reads and failed operations.
It refreshes when opened; `Ctrl-R` refreshes while open. Incomplete history is
reported in the picker. Selecting a file from this scope opens a standalone
preview with no send-back target, since a file can belong to several sessions.

Nothing is ever submitted for you. Every send lands in the prompt as a paste
and waits for you to press Enter.

### Commands

```
peekback setup [--agent claude|codex|all] [--dry-run]  configure hooks
peekback show [FILE] [--session ID] [--pane %N]   show a document
peekback browse [--session ID] [--candidates]    browse files in the terminal
peekback send [--session ID] [--pane %N] < text   paste text into a prompt
peekback hide                                     hide the viewer
peekback status [--prune]                         sessions, backends, daemon
peekback quit                                     stop the daemon
```

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
side in wide terminals; `Tab` switches panes in narrower terminals. It uses the
same `session-activity` library as the Markdown viewer. Browsing does not start
the viewer daemon; press `p` on a Markdown file to open its rendered preview.
Live sessions keep the connection for sending selections back. Files from ended
sessions open as standalone documents.

`--all-sessions` browses retained history without requiring any live session.
It merges paths across Claude Code and Codex, preserves session provenance in
the evidence view (`e`) and plain listing, and supports filtering by session ID.
`r` reloads stored history. Previewing from this mode opens without a send-back
target. It cannot be combined with session/pane selectors or `--candidates`;
it reads the tracker rather than scanning every old project.

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
cutoff are retained. A later command cannot lower an existing cutoff.

Maintenance refuses damaged history. It publishes a synced checkpoint before
removing old batches, so interrupted cleanup does not duplicate events. Readers,
writers, and maintenance coordinate through advisory locks on Unix. Replace
older Peekback binaries and restart the daemon (`peekback quit`) before applying
maintenance: older versions cannot read compacted history. Queries still read
the full retained history; compaction reduces file count, not memory use.

The separately released [`session-activity` library](https://github.com/tskupinski/peekback/tree/master/crates/session-activity)
owns event normalization, storage, scanning, and file queries. It has no
dependency on Peekback's viewer, Markdown renderer, or terminal integration.
The first release publishes both crates on crates.io, with an optional unsigned
macOS archive for users without Rust.
Rust consumers can depend on `session-activity = "0.1"` after its crates.io
publication; the app continues using the versioned workspace dependency.

Run all checks with `bash scripts/check.sh` (Python 3.9+ and Node.js are needed
for development checks). `cargo test --workspace --locked` runs the Rust tests.
The terminal smoke test uses isolated state and a fake viewer socket.
See [RELEASING.md](RELEASING.md) for packaging and fresh-install verification.

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
```

The daemon reads the file when it starts; run `peekback quit` after editing.

### How text reaches the prompt

peekback probes, in order: tmux (using the pane and server socket recorded
by the hook), WezTerm and Kitty remote control, macOS keystroke injection
(clipboard plus Cmd+V into the terminal app, needs the Accessibility
permission), and finally the clipboard with a toast asking you to paste.
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

A single daemon per user holds one window. `show` talks to it over a Unix
socket and starts it if needed. The page is served to the web view over a
custom `peekback://` scheme from assets embedded in the binary, so nothing
listens on a TCP port and it works offline. Documents are discovered from the
Claude session transcript, its scratchpad, the project's memory directory,
Codex file-edit hooks, and Markdown under the project modified since the
session started. See
`SPEC.md` for the full design.

## License

Apache License 2.0. See `LICENSE`. The embedded front-end libraries carry
their own licenses, listed in `NOTICE`.
