#!/usr/bin/env python3
"""Build a reviewable Apple Silicon preview archive; never publishes it."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[1]
TARGET = "aarch64-apple-darwin"


def output(*args):
    return subprocess.check_output(args, cwd=ROOT, text=True).strip()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--allow-dirty", action="store_true", help="build a local candidate with dirty=true in BUILD.json")
    args = parser.parse_args()
    if platform.system() != "Darwin" or platform.machine() != "arm64":
        parser.error("this release target requires an Apple Silicon Mac")
    dirty = bool(output("git", "status", "--porcelain"))
    if dirty and not args.allow_dirty:
        parser.error("commit the release inputs first, or use --allow-dirty for a local candidate")
    subprocess.run(["python3", "scripts/vendor-licenses.py", "--check"], cwd=ROOT, check=True)
    metadata = json.loads(output("cargo", "metadata", "--no-deps", "--format-version", "1", "--locked", "--offline"))
    version = next(p["version"] for p in metadata["packages"] if p["name"] == "peekback")
    env = dict(os.environ, MACOSX_DEPLOYMENT_TARGET="14.0")
    subprocess.run(["cargo", "build", "--release", "--locked", "--target", TARGET], cwd=ROOT, env=env, check=True)
    binary = Path(metadata["target_directory"]) / TARGET / "release/peekback"
    if output(str(binary), "--version") != f"peekback {version}":
        raise ValueError("built binary version does not match Cargo.toml")
    name = f"peekback-{version}-{TARGET}"
    dist = ROOT / "dist"
    dist.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="peekback-package-") as temporary:
        staged = Path(temporary) / name
        (staged / "bin").mkdir(parents=True)
        shutil.copy2(binary, staged / "bin/peekback")
        shutil.copytree(ROOT / "hooks", staged / "hooks")
        shutil.copytree(ROOT / "licenses", staged / "licenses")
        subprocess.run(["python3", "scripts/rust-licenses.py", str(staged / "licenses/rust"), "--target", TARGET], cwd=ROOT, check=True)
        for filename in ["README.md", "LICENSE", "NOTICE", "CHANGELOG.md", "RELEASING.md"]:
            shutil.copyfile(ROOT / filename, staged / filename)
        shutil.copyfile(ROOT / "scripts/install.sh", staged / "install.sh")
        (staged / "install.sh").chmod(0o755)
        build = {"version": version, "git_revision": output("git", "rev-parse", "HEAD"), "dirty": dirty,
                 "target": TARGET, "macos_deployment_target": "14.0", "rustc": output("rustc", "--version"),
                 "signing": "ad-hoc; not Developer ID signed or notarized",
                 "cargo_lock_sha256": hashlib.sha256((ROOT / "Cargo.lock").read_bytes()).hexdigest()}
        (staged / "BUILD.json").write_text(json.dumps(build, indent=2) + "\n")
        archive = dist / f"{name}.tar.gz"
        temporary_archive = dist / f".{name}.tar.gz.tmp"
        with tarfile.open(temporary_archive, "w:gz") as tar:
            tar.add(staged, arcname=name)
        temporary_archive.replace(archive)
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    (dist / "SHA256SUMS").write_text(f"{digest}  {archive.name}\n")
    print(f"Created {archive} (dirty={dirty}); inspect and test before publishing.")


if __name__ == "__main__":
    main()
