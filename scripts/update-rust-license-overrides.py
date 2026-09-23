#!/usr/bin/env python3
"""Fetch omitted license files at the revisions recorded in Cargo packages.

Run deliberately after updating Cargo.lock; review resulting source URLs and
notices. Normal checks and packaging use the committed copies offline.
"""
import hashlib
import json
from pathlib import Path
import re
import subprocess
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
DEST = ROOT / "licenses/rust-overrides"
SOURCES = {
    "https://github.com/madsmtm/objc2": ["LICENSE.md"],
    "https://github.com/Michael-F-Bryan/include_dir": ["LICENSE"],
    "https://github.com/etemesi254/zune-image": ["LICENSE.md", "LICENSE-ZLIB"],
}
NOTICE = re.compile(r"^(licen[cs]e|copying|copyright|notice|unlicense)([.\-_]|$)", re.I)
TEXT_REVISION = "31ba1a50e5397e00a304dbadc76531740e89ee48"


def main():
    metadata = json.loads(subprocess.check_output([
        "cargo", "metadata", "--format-version", "1", "--locked", "--offline",
        "--filter-platform", "aarch64-apple-darwin",
    ], cwd=ROOT))
    active = {node["id"] for node in metadata["resolve"]["nodes"]}
    DEST.mkdir(parents=True, exist_ok=True)
    records = {}
    cache = {}
    for package in metadata["packages"]:
        if package["id"] not in active or package["source"] is None:
            continue
        source = Path(package["manifest_path"]).parent
        if any(p.is_file() and NOTICE.match(p.name) for p in source.rglob("*")) or package.get("license_file"):
            continue
        repository = package["repository"]
        if package["name"] in ["zune-core", "zune-jpeg"]:
            repository = "https://github.com/etemesi254/zune-image"
        if repository not in SOURCES:
            raise ValueError(f"Review a license source for {package['name']}: {repository}")
        revision = json.loads((source / ".cargo_vcs_info.json").read_text())["git"]["sha1"]
        key = f"{package['name']}-{package['version']}"
        record = {"repository": repository, "revision": revision, "files": {}}
        for filename in SOURCES[repository]:
            url = repository.replace("github.com", "raw.githubusercontent.com") + f"/{revision}/{filename}"
            if url not in cache:
                request = urllib.request.Request(url, headers={"User-Agent": "peekback-license-collector"})
                with urllib.request.urlopen(request, timeout=60) as response:
                    cache[url] = response.read()
            data = cache[url]
            path = DEST / key / filename
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
            record["files"][filename] = {"url": url, "sha256": hashlib.sha256(data).hexdigest()}
        records[key] = record
    (DEST / "sources.json").write_text(json.dumps(records, indent=2, sort_keys=True) + "\n")
    print(f"Collected omitted licenses for {len(records)} packages at their source revisions.")
    # Some upstream notices name/link the terms without reproducing them.
    expressions = [p["license"] or "" for p in metadata["packages"] if p["id"] in active and p["source"]]
    frontend = json.loads((ROOT / "licenses/frontend/manifest.json").read_text())
    expressions.extend(p["license"] for p in frontend["packages"] if isinstance(p["license"], str))
    identifiers = set(re.findall(r"[A-Za-z0-9][A-Za-z0-9.+-]*", " ".join(expressions))) - {"AND", "OR", "WITH"}
    texts = ROOT / "licenses/texts"
    texts.mkdir(exist_ok=True)
    sources = {}
    for identifier in sorted(identifiers):
        url = f"https://raw.githubusercontent.com/spdx/license-list-data/{TEXT_REVISION}/text/{identifier}.txt"
        with urllib.request.urlopen(url, timeout=60) as response:
            data = response.read()
        (texts / f"{identifier}.txt").write_bytes(data)
        sources[identifier] = {"url": url, "sha256": hashlib.sha256(data).hexdigest()}
    (texts / "sources.json").write_text(json.dumps(sources, indent=2, sort_keys=True) + "\n")
    print(f"Collected {len(sources)} canonical license/exception texts from pinned SPDX sources.")


if __name__ == "__main__":
    main()
