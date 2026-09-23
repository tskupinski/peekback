#!/usr/bin/env python3
"""Collect notices for the locked macOS dependency graph, without network access.

Includes build/proc-macro dependencies conservatively. Missing upstream notices
must be supplied in licenses/rust-overrides with recorded source provenance.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[1]
NOTICE = re.compile(r"^(licen[cs]e|copying|copyright|notice|unlicense)([.\-_]|$)", re.I)


def collect(destination, target):
    texts = ROOT / "licenses/texts"
    for identifier, record in json.loads((texts / "sources.json").read_text()).items():
        if hashlib.sha256((texts / f"{identifier}.txt").read_bytes()).hexdigest() != record["sha256"]:
            raise ValueError(f"license text checksum mismatch: {identifier}")
    metadata = json.loads(subprocess.check_output([
        "cargo", "metadata", "--format-version", "1", "--locked", "--offline", "--filter-platform", target,
    ], cwd=ROOT))
    active = {node["id"] for node in metadata["resolve"]["nodes"]}
    provenance = json.loads((ROOT / "licenses/rust-overrides/sources.json").read_text())
    records = []
    for package in sorted(metadata["packages"], key=lambda p: (p["name"], p["version"])):
        if package["id"] not in active or package["source"] is None:
            continue
        key = f"{package['name']}-{package['version']}"
        source = Path(package["manifest_path"]).parent
        # Also retain notices for bundled C code, fonts, etc. within crates.
        files = [path for path in source.rglob("*") if path.is_file() and NOTICE.match(path.name)]
        if package.get("license_file"):
            files.append(source / package["license_file"])
        override = provenance.get(key)
        if not files and override:
            vcs = json.loads((source / ".cargo_vcs_info.json").read_text())
            if vcs["git"]["sha1"] != override["revision"]:
                raise ValueError(f"license override revision mismatch: {key}")
            source = ROOT / "licenses/rust-overrides" / key
            files = [source / name for name in override["files"]]
            for path in files:
                if hashlib.sha256(path.read_bytes()).hexdigest() != override["files"][path.name]["sha256"]:
                    raise ValueError(f"license override checksum mismatch: {key}/{path.name}")
        if not files:
            raise ValueError(f"Missing license text for {key}; add a pinned upstream override")
        for path in sorted(set(files)):
            relative = path.relative_to(source)
            dest = destination / key / relative
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(path, dest)
        records.append({"package": key, "license": package["license"], "repository": package["repository"],
                        "notices": [str(path.relative_to(source)) for path in sorted(set(files))]})
        if package["license"] == "MPL-2.0":
            # Ship the unmodified crate source as well as its license notice.
            shutil.copytree(Path(package["manifest_path"]).parent, destination.parent / "sources" / key,
                            ignore=shutil.ignore_patterns(".cargo-ok"), dirs_exist_ok=True)
    destination.mkdir(parents=True, exist_ok=True)
    (destination / "index.json").write_text(json.dumps({"target": target, "packages": records}, indent=2) + "\n")
    print(f"Collected Rust notices for {len(records)} locked packages.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("destination", type=Path)
    parser.add_argument("--target", default="aarch64-apple-darwin")
    args = parser.parse_args()
    collect(args.destination, args.target)
