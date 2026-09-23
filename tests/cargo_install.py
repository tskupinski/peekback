#!/usr/bin/env python3
"""Check Cargo's file list and a source install before the first registry release."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--offline", action="store_true")
    args = parser.parse_args()
    flags = ["--offline"] if args.offline else []
    names = set(subprocess.check_output([
        "cargo", "package", "-p", "peekback", "--list", "--allow-dirty", *flags,
    ], cwd=ROOT, text=True).splitlines())
    required = {"Cargo.toml", "Cargo.lock", "LICENSE", "NOTICE", "src/main.rs", "src/setup.rs"}
    for directory in ["assets", "hooks", "licenses"]:
        required.update(str(p.relative_to(ROOT)) for p in (ROOT / directory).rglob("*") if p.is_file())
    if required - names:
        raise ValueError(f"Cargo package is missing: {sorted(required - names)}")
    if any(name.split("/")[0] in {"target", "dist", ".github", "crates"} for name in names):
        raise ValueError("Cargo package includes unrelated build/workspace content")
    with tempfile.TemporaryDirectory(prefix="peekback-cargo-") as temporary:
        root = Path(temporary)
        install = root / "install with spaces"
        env = dict(os.environ, MACOSX_DEPLOYMENT_TARGET="14.0")
        subprocess.run([
            "cargo", "install", "--path", str(ROOT), "--root", str(install),
            "--locked", "--target", "aarch64-apple-darwin", *flags,
        ], cwd=ROOT, env=env, check=True)
        binary = install / "bin/peekback"
        env.update(CODEX_HOME=str(root / "codex"), CLAUDE_CONFIG_DIR=str(root / "claude"),
                   PEEKBACK_STATE_DIR=str(root / "state"), PATH="")
        def run(*arguments, **kwargs):
            return subprocess.check_output([str(binary), *arguments], env=env, cwd=root, text=True, **kwargs)
        assert run("--version").startswith("peekback ")
        run("setup", "--agent", "all", "--dry-run")
        assert not (root / "codex").exists() and not (root / "claude").exists()
        run("setup", "--agent", "all")
        for agent, filename in [("claude", "settings.json"), ("codex", "hooks.json")]:
            path = root / agent / filename
            first = path.read_bytes()
            run("setup", "--agent", agent)
            assert path.read_bytes() == first
            settings = json.loads(first)
            command = settings["hooks"]["PostToolUse"][0]["hooks"][0]["command"]
            payload = {"session_id": f"{agent}-install", "cwd": str(root), "hook_event_name": "PostToolUse",
                       "tool_name": "Write" if agent == "claude" else "apply_patch",
                       "tool_input": {"file_path": "note.md", "command": "*** Begin Patch\n*** Add File: note.md\n+# Note\n*** End Patch"}}
            subprocess.run(["/bin/sh", "-c", command], input=json.dumps(payload), text=True,
                           env=env, cwd=root, check=True)
            files = json.loads(run("activity", "files", "--agent", agent, "--session", f"{agent}-install"))
            assert len(files) == 1 and files[0]["path"] == str(root / "note.md")
        assert not (root / "state/daemon.log").exists()
    print("Cargo source install passed: package file list, isolated install, setup, both agents, no helper scripts.")
    print("Registry packaging/publish verification still requires session-activity to be published first.")


if __name__ == "__main__":
    main()
