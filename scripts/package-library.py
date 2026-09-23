#!/usr/bin/env python3
"""Verify the standalone crate and create a local release artifact; never uploads."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def run(*args, **kwargs):
    subprocess.run(args, cwd=ROOT, check=True, **kwargs)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--allow-dirty", action="store_true", help="package uncommitted changes for local review")
    parser.add_argument("--offline", action="store_true", help="use only cached Cargo packages")
    args = parser.parse_args()
    metadata = json.loads(subprocess.check_output([
        "cargo", "metadata", "--no-deps", "--format-version", "1", "--locked", "--offline",
    ], cwd=ROOT))
    version = next(p["version"] for p in metadata["packages"] if p["name"] == "session-activity")
    flags = ["--locked"] + (["--offline"] if args.offline else [])
    run("cargo", "package", "-p", "session-activity", *flags, *(["--allow-dirty"] if args.allow_dirty else []))
    name = f"session-activity-{version}"
    target = Path(metadata["target_directory"])
    archive = target / "package" / f"{name}.crate"
    with tempfile.TemporaryDirectory(prefix="session-activity-package-") as temporary:
        root = Path(temporary)
        with tarfile.open(archive) as tar:
            names = {member.name for member in tar.getmembers()}
            for required in ["Cargo.toml", "README.md", "LICENSE", "NOTICE", "CHANGELOG.md", "src/lib.rs", "tests/public_api.rs", "examples/inspect.rs"]:
                if f"{name}/{required}" not in names:
                    raise ValueError(f"missing package file: {required}")
            for member in tar.getmembers():
                relative = Path(member.name)
                if relative.is_absolute() or ".." in relative.parts or relative.parts[0] != name:
                    raise ValueError(f"unsafe package path: {member.name}")
                if len(relative.parts) > 1 and relative.parts[1] in ["assets", "hooks", "target", ".github"]:
                    raise ValueError(f"unexpected application content: {member.name}")
                path = root / relative
                if member.isdir():
                    path.mkdir(parents=True, exist_ok=True)
                elif member.isfile():
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_bytes(tar.extractfile(member).read())
                else:
                    raise ValueError(f"unexpected package member: {member.name}")
        # Outside the repository: tests cannot accidentally see its workspace,
        # frontend assets, hook scripts, or an unshipped source file.
        manifest = str(root / name / "Cargo.toml")
        env = dict(os.environ, CARGO_TARGET_DIR=str(target / "library-check"))
        run("cargo", "test", "--manifest-path", manifest, *flags, env=env)
        run("cargo", "clippy", "--manifest-path", manifest, *flags, "--all-targets", "--", "-D", "warnings", env=env)
        run("cargo", "doc", "--manifest-path", manifest, *flags, "--no-deps",
            env=dict(env, RUSTDOCFLAGS="-D warnings"))
        empty_store = str(root / "empty-store")
        result = subprocess.check_output([
            "cargo", "run", "--quiet", "--manifest-path", manifest, *flags, "--example", "inspect", "--",
            empty_store, "codex", "example-session",
        ], env=env, cwd=temporary, text=True)
        if json.loads(result) != [] or Path(empty_store).exists():
            raise ValueError("read-only consumer example did not behave as expected")
    destination = ROOT / "dist/library"
    destination.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(archive, destination / archive.name)
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    (destination / "SHA256SUMS").write_text(f"{digest}  {archive.name}\n")
    print(f"Verified standalone library: {destination / archive.name}; not uploaded.")


if __name__ == "__main__":
    main()
