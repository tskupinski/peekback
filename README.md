# peekback

A rendered Markdown viewer for Claude Code sessions, with a way back: select
text in the viewer and copy it, send it to the session's prompt, or attach a
note and send that.

You peek at what the agent wrote. What you mark up goes back.

peekback is a small Rust daemon with a native web view. It knows which Claude
Code sessions are live, lists the Markdown each one wrote, renders it with
real typesetting, Mermaid, KaTeX and syntax highlighting, and pastes what you
select back into the session's prompt. It sits over your terminal window like
a popup, takes its colors from your terminal, and is driven from the keyboard
with vim-style keys. macOS only for now.

## Install

Requirements: Rust 1.87 or newer, `jq`, macOS. The vendored front end is
committed, so no Node toolchain is needed.

```sh
git clone <this repo> && cd peekback
cargo build --release
cp target/release/peekback ~/bin/        # or anywhere on your PATH
```

### Wire the hook into Claude Code

peekback learns about sessions from a Claude Code hook. Add the entries from
`hooks/settings-snippet.json` to `~/.claude/settings.json`, replacing the
placeholder path with the absolute path to `hooks/register.sh` in your
clone. The same script goes on four events: `SessionStart`,
`UserPromptSubmit`, `Stop`, and `SessionEnd`.

Claude Code picks the change up without a restart. Sessions started before
the hook was wired will not appear until they send another prompt.

Check it worked:

```sh
peekback status
```

You should see your live sessions, the terminal each one runs in, and the
send backend peekback would use for it.

## Use

Press `Cmd+Shift+M` from anywhere. The viewer appears over the right half of
your terminal window, showing the newest Markdown file of your most recently
active session, and takes keyboard focus. Press `Esc` to drop it and return
to the terminal.

From inside a Claude Code prompt, `! peekback show` opens the viewer on that
exact session without focusing it. `peekback show path/to/file.md` shows a
specific file.

### Keys

| Key | Action |
| --- | --- |
| `j` / `k` | move the block cursor, with counts like `5j` |
| `d` / `u` | half page down / up |
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
| `Space s` | session picker |
| `Space c` | pending comments (`Enter` jumps, `Ctrl-D` removes) |
| `:` | command line: `:doc`, `:session`, `:send`, `:c note`, `:sendall`, `:sidebar`, `:q`, `:help` |
| `Tab` | toggle the sidebar |
| `?` | key overlay |
| `Esc` | cancel, clear search, or leave the viewer |

Text selected with the mouse gets a small toolbar with the same Copy, Send
and Comment actions.

Nothing is ever submitted for you. Every send lands in the prompt as a paste
and waits for you to press Enter.

### Commands

```
peekback show [FILE] [--session ID] [--pane %N]   show a document
peekback send [--session ID] [--pane %N] < text   paste text into a prompt
peekback hide                                     hide the viewer
peekback status [--prune]                         sessions, backends, daemon
peekback quit                                     stop the daemon
```

Session resolution for `show` and `send`: `--session`, else the session in
the given tmux pane, else the session this shell runs inside, else the most
recently active one.

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
session transcript, its scratchpad, the project's memory directory, and
Markdown under the project modified since the session started. See
`SPEC.md` for the full design.

## License

Apache License 2.0. See `LICENSE`. The embedded front-end libraries carry
their own licenses, listed in `NOTICE`.
