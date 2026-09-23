"""Verify a release archive, install in a spaced path, and exercise both hooks."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile


def check(archive):
    expected = next(line.split()[0] for line in archive.with_name("SHA256SUMS").read_text().splitlines()
                    if line.split()[1] == archive.name)
    assert hashlib.sha256(archive.read_bytes()).hexdigest() == expected
    with tempfile.TemporaryDirectory(prefix="peekback-install-test-") as temporary:
        root = Path(temporary)
        with tarfile.open(archive) as tar:
            for member in tar.getmembers():
                relative = Path(member.name)
                assert not relative.is_absolute() and ".." not in relative.parts
                path = root / relative
                if member.isdir():
                    path.mkdir(parents=True, exist_ok=True)
                else:
                    assert member.isfile(), f"unexpected archive member: {member.name}"
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_bytes(tar.extractfile(member).read())
                    path.chmod(member.mode & 0o777)
        extracted = root / archive.name.removesuffix(".tar.gz")
        install_dir, bin_dir, state = root / "installed app", root / "user bin", root / "state"
        env = dict(os.environ, PEEKBACK_INSTALL_DIR=str(install_dir), PEEKBACK_BIN_DIR=str(bin_dir), PEEKBACK_STATE_DIR=str(state))
        env.pop("PEEKBACK_BIN", None)
        subprocess.run(["bash", str(extracted / "install.sh")], env=env, check=True, capture_output=True)
        binary = str(bin_dir / "peekback")
        build = json.loads((install_dir / "BUILD.json").read_text())
        assert subprocess.check_output([binary, "--version"], env=env, text=True).strip() == f"peekback {build['version']}"
        assert (install_dir / "licenses/rust/index.json").is_file()
        assert (install_dir / "licenses/frontend/manifest.json").is_file()
        # A stale binary earlier on PATH must not override the archive's binary.
        stale_bin = root / "stale-bin"
        stale_bin.mkdir()
        (stale_bin / "peekback").write_text("#!/bin/sh\nexit 42\n")
        (stale_bin / "peekback").chmod(0o755)
        env["PATH"] = str(stale_bin) + os.pathsep + env["PATH"]
        for agent, snippet in [("claude", "settings-snippet.json"), ("codex", "codex-snippet.json")]:
            hooks = json.loads((install_dir / "hooks" / snippet).read_text())["hooks"]
            session_id = f"release-{agent}"
            payload = dict(session_id=session_id, cwd=str(root), transcript_path=None)
            for event in ["SessionStart", "PostToolUse", "SessionEnd"]:
                payload["hook_event_name"] = event
                if event == "PostToolUse":
                    if agent == "claude":
                        payload.update(tool_name="Write", tool_input={"file_path": str(root / "note.md"), "content": "# Note"})
                    else:
                        payload.update(tool_name="apply_patch", tool_input={"command": "*** Begin Patch\n*** Add File: note.md\n+# Note\n*** End Patch"})
                command = hooks[event][0]["hooks"][0]["command"].replace("/absolute/path/to/peekback", str(install_dir))
                result = subprocess.run(["bash", "-c", command], input=json.dumps(payload), text=True, env=env, capture_output=True, check=True)
                assert not result.stderr, result.stderr
                assert (state / "sessions" / f"{session_id}.json").exists() == (event != "SessionEnd")
            files = json.loads(subprocess.check_output([binary, "activity", "files", "--agent", agent, "--session", session_id], env=env))
            assert len(files) == 1 and files[0]["path"] == str(root / "note.md")
        assert not (state / "daemon.log").exists()
    print("Release archive passed: checksum, version, isolated install, spaced paths, both hook lifecycles, retained history.")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("usage: python3 tests/release_archive.py ARCHIVE.tar.gz")
    check(Path(sys.argv[1]).resolve())
