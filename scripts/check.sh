#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
cargo fmt --all -- --check
cargo clippy --workspace --locked --all-targets -- -D warnings
cargo test --workspace --locked
cargo build --locked
node --check assets/app.js
node tests/picker.js
bash -n hooks/register.sh scripts/*.sh
python3 scripts/vendor-licenses.py --check
python3 -m unittest discover -s tests -p 'test_*.py'
python3 tests/browser_pty.py
