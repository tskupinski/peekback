#!/bin/bash
# Distributed in the release archive. Does not edit agent settings or start a daemon.
set -euo pipefail
source_dir="$(cd "$(dirname "$0")" && pwd)"
install_dir="${PEEKBACK_INSTALL_DIR:-$HOME/.local/share/peekback}"
bin_dir="${PEEKBACK_BIN_DIR:-$HOME/.local/bin}"
case "$install_dir:$bin_dir" in /*:/*) ;; *) echo "Install directories must be absolute paths" >&2; exit 1 ;; esac
if [ ! -x "$source_dir/bin/peekback" ]; then
    echo "Run install.sh from the extracted release archive" >&2
    exit 1
fi
if [ -d "$bin_dir/peekback" ] && [ ! -L "$bin_dir/peekback" ]; then
    echo "Cannot replace directory $bin_dir/peekback" >&2
    exit 1
fi
mkdir -p "$install_dir/bin" "$install_dir/hooks" "$bin_dir"
if [ "$(cd "$install_dir" && pwd)" = "$source_dir" ]; then
    echo "Choose an installation directory different from the extracted archive" >&2
    exit 1
fi
install -m 755 "$source_dir/bin/peekback" "$install_dir/bin/peekback"
install -m 755 "$source_dir/hooks/register.sh" "$install_dir/hooks/register.sh"
cp "$source_dir/hooks/"*.json "$install_dir/hooks/"
cp -R "$source_dir/licenses" "$install_dir/"
cp "$source_dir/README.md" "$source_dir/LICENSE" "$source_dir/NOTICE" \
    "$source_dir/CHANGELOG.md" "$source_dir/RELEASING.md" "$source_dir/BUILD.json" "$install_dir/"
ln -sfn "$install_dir/bin/peekback" "$bin_dir/peekback"
"$bin_dir/peekback" --version
echo "Installed. Add $bin_dir to PATH if needed."
echo "Run peekback setup to configure Claude Code / Codex hooks."
echo "After a session registers, run peekback show once to start the viewer/hotkey."
echo "When upgrading, run peekback quit so the next preview starts the new daemon."
