#!/bin/bash
# Claude Code hook that keeps the peekback session registry current.
# Wire it to SessionStart, UserPromptSubmit, Stop, and SessionEnd.
set -u

command -v jq >/dev/null 2>&1 || exit 0

input="$(cat)"
session_id="$(printf '%s' "$input" | jq -r '.session_id // empty')"
[ -n "$session_id" ] || exit 0
event="$(printf '%s' "$input" | jq -r '.hook_event_name // empty')"

dir="$HOME/.local/state/peekback/sessions"
file="$dir/$session_id.json"

if [ "$event" = "SessionEnd" ]; then
	rm -f "$file"
	exit 0
fi

mkdir -p "$dir"
now="$(date +%s)"
started_at="$now"
if [ -f "$file" ]; then
	started_at="$(jq -r ".started_at // $now" "$file" 2>/dev/null || echo "$now")"
fi

# $TMUX is "<socket path>,<server pid>,<session index>".
tmux_socket="${TMUX:+${TMUX%%,*}}"

printf '%s' "$input" | jq \
	--arg started_at "$started_at" \
	--arg now "$now" \
	--arg bundle_id "${__CFBundleIdentifier:-}" \
	--arg term_program "${TERM_PROGRAM:-}" \
	--arg tmux_pane "${TMUX_PANE:-}" \
	--arg tmux_socket "$tmux_socket" \
	--arg kitty_window "${KITTY_WINDOW_ID:-}" \
	--arg kitty_listen_on "${KITTY_LISTEN_ON:-}" \
	--arg wezterm_pane "${WEZTERM_PANE:-}" \
	'{
		session_id: .session_id,
		transcript_path: .transcript_path,
		cwd: .cwd,
		started_at: ($started_at | tonumber),
		last_active_at: ($now | tonumber),
		terminal: ({
			bundle_id: $bundle_id,
			term_program: $term_program,
			tmux_pane: $tmux_pane,
			tmux_socket: $tmux_socket,
			kitty_window: $kitty_window,
			kitty_listen_on: $kitty_listen_on,
			wezterm_pane: $wezterm_pane
		} | with_entries(select(.value != "")))
	}' > "$file.tmp" && mv "$file.tmp" "$file"
