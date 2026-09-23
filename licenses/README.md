# Third-party notices

Peekback is Apache-2.0; see the root LICENSE and NOTICE. Dependencies retain
their own licenses. Release archives carry this directory alongside the binary.

- `frontend/`: license and copyright files from the exact npm releases.
  `manifest.json` records npm integrity hashes, upstream notice URLs where
  needed, source-map/lockfile provenance, and SHA-256 hashes for shipped assets
  and collected notices. Mermaid's bundled dependency versions come from its
  source map; markdown-it's come from its release-revision lockfile. KaTeX's
  package includes the font assets under its upstream license.
- `rust-overrides/`: notices omitted by some published Cargo packages, copied
  from their `.cargo_vcs_info.json` source revisions. `sources.json` records
  URLs and checksums.
- `rust/` (generated in release archives): notices from the locked target
  dependency graph, including build/proc-macro packages conservatively.
  `index.json` records package versions, license expressions, and repositories.
- `texts/`: canonical license/exception terms from a pinned SPDX revision,
  including terms that upstream notices reference only by link.
- `SOURCE-AVAILABILITY.md`: source locations for EPL/MPL components. Release
  archives also include the unmodified option-ext source under `sources/`.

Normal checks and packaging use committed notices without network access.
Package sources must already be present in Cargo's cache (`cargo fetch --locked`).
To refresh after a dependency update:

```sh
python3 scripts/vendor-licenses.py
python3 scripts/update-rust-license-overrides.py
```

These commands download metadata and notices but execute no downloaded code.
Review the changes, especially new license types and bundled dependencies.
`scripts/vendor.sh` refreshes frontend notices after updating assets.
`python3 scripts/vendor-licenses.py --check` detects stale assets or notices;
packaging fails if a locked Rust package has no collected license text.
