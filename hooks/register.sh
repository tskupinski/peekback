#!/bin/bash
# Compatibility entry point for existing Claude Code and Codex hook configs.
# PEEKBACK_BIN can select a particular build; otherwise use PATH or this clone.
set -u
agent="${1:-claude}"
case "$agent" in claude | codex) ;; *) exit 0 ;; esac

binary="${PEEKBACK_BIN:-}"
repo="$(cd "$(dirname "$0")/.." && pwd)"
if [ -z "$binary" ] && [ -x "$repo/bin/peekback" ]; then
    binary="$repo/bin/peekback"
fi
if [ -z "$binary" ]; then
    binary="$(command -v peekback || true)"
fi
if [ -z "$binary" ]; then
    for candidate in "$repo/target/release/peekback" "$repo/target/debug/peekback"; do
        if [ -x "$candidate" ]; then binary="$candidate"; break; fi
    done
fi
if [ -z "$binary" ]; then
    echo "peekback: build or install peekback before using hooks/register.sh" >&2
    exit 0
fi
"$binary" activity record --agent "$agent" || echo "peekback: session activity could not be recorded" >&2
# Tracking failures must not block the agent.
exit 0
