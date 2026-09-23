#!/usr/bin/env python3
"""Collect frontend notices from pinned npm archives, or verify them offline.

Mermaid's source map names the bundled package versions. markdown-it's map
names its dependencies; its release-revision package-lock supplies versions.
No downloaded code is executed. npm archive integrity is checked before use.
"""
import argparse
import base64
from concurrent.futures import ThreadPoolExecutor
import hashlib
import io
import json
from pathlib import Path, PurePosixPath
import re
import shutil
import tarfile
import tempfile
import urllib.request
import urllib.error

ROOT = Path(__file__).resolve().parents[1]
DEST = ROOT / "licenses/frontend"
ALIASES = {"highlight.js": "@highlightjs/cdn-assets"}
NOTICE = re.compile(r"^(licen[cs]e|copying|copyright|notice|authors|ofl)([.\-_]|$)", re.I)


def download(url):
    request = urllib.request.Request(url, headers={"User-Agent": "peekback-license-collector"})
    with urllib.request.urlopen(request, timeout=60) as response:
        return response.read()


def sha(data):
    return hashlib.sha256(data).hexdigest()


def versions():
    return dict(line.split() for line in (ROOT / "assets/vendor/VERSIONS").read_text().splitlines())


def package(name, version):
    metadata_url = f"https://registry.npmjs.org/{name}/{version}"
    metadata = json.loads(download(metadata_url))
    blob = download(metadata["dist"]["tarball"])
    algorithm, digest = metadata["dist"]["integrity"].split("-", 1)
    if algorithm != "sha512" or base64.b64encode(hashlib.sha512(blob).digest()).decode() != digest:
        raise ValueError(f"archive integrity mismatch: {name}@{version}")
    archive = tarfile.open(fileobj=io.BytesIO(blob), mode="r:gz")
    return metadata, archive


def collect(item):
    name, version = item
    metadata, archive = package(name, version)
    files = {}
    with archive:
        for member in archive:
            path = PurePosixPath(member.name)
            if member.isfile() and NOTICE.match(path.name):
                if path.parts[0] != "package" or ".." in path.parts:
                    raise ValueError(f"unsafe archive path: {path}")
                files[str(PurePosixPath(*path.parts[1:]))] = archive.extractfile(member).read()
        if not files:
            # Some packages (e.g. fastdom) put the complete MIT notice in README.
            for member in archive.getmembers():
                if member.isfile() and member.name.lower() == "package/readme.md":
                    data = archive.extractfile(member).read()
                    if b"Permission is hereby granted" in data and b"Copyright" in data:
                        files["README.md"] = data
    if not files:
        repository = metadata.get("repository", {})
        repository = repository.get("url", "") if isinstance(repository, dict) else repository
        repository = repository.removeprefix("git+").replace("git://github.com/", "https://github.com/").removesuffix(".git")
        revision = metadata.get("gitHead", "")
        if repository.startswith("https://github.com/") and re.fullmatch(r"[0-9a-f]{40}", revision):
            for filename in ["LICENSE", "LICENSE.md", "LICENSE.txt", "LICENSE-MIT"]:
                url = repository.replace("github.com", "raw.githubusercontent.com") + f"/{revision}/{filename}"
                try:
                    files[filename] = download(url)
                    metadata["_notice_source"] = url
                    break
                except urllib.error.HTTPError as error:
                    if error.code != 404:
                        raise
        if not files:
            raise ValueError(f"no license notices in {name}@{version}; add a verified upstream source")
    return name, version, metadata, files


def refresh():
    root_versions = versions()
    packages = {(ALIASES.get(name, name), version) for name, version in root_versions.items()}
    provenance = {}
    for name, map_path in [
        ("mermaid", "package/dist/mermaid.min.js.map"),
        ("markdown-it", "package/dist/browser/markdown-it.umd.min.js.map"),
    ]:
        metadata, archive = package(name, root_versions[name])
        with archive:
            sources = json.load(archive.extractfile(map_path))["sources"]
        provenance[name] = {"source_map": map_path, "git_revision": metadata.get("gitHead")}
        if name == "mermaid":
            for source in sources:
                match = re.search(r"/\.pnpm/([^/]+)/node_modules/", source)
                if match:
                    package_name, version = match[1].split("_", 1)[0].rsplit("@", 1)
                    packages.add((package_name.replace("+", "/"), version))
                elif "node_modules/" in source:
                    raise ValueError(f"unrecognized bundled dependency: {source}")
        else:
            lock_url = f"https://raw.githubusercontent.com/markdown-it/markdown-it/{metadata['gitHead']}/package-lock.json"
            lock_bytes = download(lock_url)
            lock = json.loads(lock_bytes)["packages"]
            provenance[name].update(lock_url=lock_url, lock_sha256=sha(lock_bytes))
            for source in sources:
                if "node_modules/" in source:
                    module = source.split("node_modules/", 1)[1].split("/")[0]
                    packages.add((module, lock[f"node_modules/{module}"]["version"]))

    DEST.parent.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(dir=DEST.parent, prefix=".frontend-") as temporary:
        staged = Path(temporary)
        records = []
        with ThreadPoolExecutor(max_workers=6) as pool:
            for name, version, metadata, files in pool.map(collect, sorted(packages)):
                directory = name.replace("/", "__") + "@" + version
                record = {"name": name, "version": version, "license": metadata.get("license"),
                          "archive": metadata["dist"]["tarball"], "integrity": metadata["dist"]["integrity"], "files": {}}
                if metadata.get("_notice_source"):
                    record["upstream_notice"] = metadata["_notice_source"]
                for relative, data in sorted(files.items()):
                    target = staged / directory / relative
                    target.parent.mkdir(parents=True, exist_ok=True)
                    target.write_bytes(data)
                    record["files"][str(target.relative_to(staged))] = sha(data)
                records.append(record)
        # Keep the notices emitted by the bundlers as well as individual licenses.
        manifest = {"roots": root_versions, "provenance": provenance, "packages": records,
                    "assets": {str(path.relative_to(ROOT)): sha(path.read_bytes())
                               for path in sorted((ROOT / "assets/vendor").rglob("*")) if path.is_file()}}
        (staged / "manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
        if DEST.exists():
            shutil.rmtree(DEST)
        shutil.copytree(staged, DEST)
    print(f"Collected frontend notices for {len(records)} packages.")


def check():
    manifest = json.loads((DEST / "manifest.json").read_text())
    if manifest["roots"] != versions():
        raise ValueError("frontend versions changed; refresh their notices")
    assets = {str(path.relative_to(ROOT)): sha(path.read_bytes())
              for path in (ROOT / "assets/vendor").rglob("*") if path.is_file()}
    if manifest["assets"] != assets:
        raise ValueError("vendored assets changed; refresh and review their notices")
    for record in manifest["packages"]:
        for name, expected in record["files"].items():
            if sha((DEST / name).read_bytes()) != expected:
                raise ValueError(f"missing or changed notice: {name}")
    print(f"Verified frontend assets and {len(manifest['packages'])} package notices.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="verify the committed inventory without network access")
    args = parser.parse_args()
    check() if args.check else refresh()
