# Releasing Peekback

Cargo is the primary distribution method. Both [peekback](https://crates.io/crates/peekback)
and [session-activity](https://crates.io/crates/session-activity) were first
published as `0.1.0` on September 23, 2026. Users install the app with:

```sh
cargo install peekback --locked
peekback setup
```

The app and library have independent versions and tags. Publish a changed
library before an app that depends on its new version. An app-only release can
reuse the existing library release. The app remains Apple Silicon macOS-only;
the library also supports Linux.

No prebuilt GitHub release is published yet. Optional archive packaging is
described below. Preparation scripts and workflows do not publish crates,
create tags, or create GitHub releases.

## Validation evidence

The [CI run for commit d9e4ecd](https://github.com/tskupinski/peekback/actions/runs/35974796395)
passed the app job and both standalone library jobs. This is automated evidence,
not a substitute for the manual acceptance checks on each release.

| Component | Scope / evidence |
| --- | --- |
| App CI | macOS 15 ARM runner: checks, source Cargo install, setup, archive installation |
| Library CI | Standalone package tests, Clippy, rustdoc, and consumer example on Ubuntu 24.04 and macOS 15 |
| Release archive | `aarch64-apple-darwin`, deployment target macOS 14.0 |
| Local build/test host at 0.1.0 | Apple Silicon, macOS 26.3, Rust 1.87.0 |
| Agent payload coverage | Synthetic hook/transcript fixtures, both installed hook lifecycles, retained history |
| Terminal UI | Automated pseudo-terminal test; preview IPC uses a fake viewer |
| Manual integration coverage | Record actual agent versions and terminal backends in release notes |
| Other platforms | No Intel/Linux/Windows app builds or desktop/IDE composer integration |

Local agent versions observed during 0.1.0 preparation were Claude Code 2.1.280
and Codex CLI 0.156.1. These are an inventory, not claimed minimum versions or
proof of end-to-end coverage. The deployment target is a compatibility baseline,
not proof of testing on every macOS version. WezTerm and Kitty send backends
remain unverified on real installations.

## Prepare a release

1. Update the version and changelog of each crate being released. For a library
   release, also update Peekback's `session-activity` dependency requirement to
   match the workspace library. Do not republish an unchanged version.
2. Run `cargo check --workspace` on macOS to refresh `Cargo.lock` after version
   changes, then review the lockfile diff. Commit it with the release changes.
3. Run the checks below, review the package contents and manual acceptance
   checklist, push the reviewed commit, and wait for CI to pass.
4. Publish the library if changed, then the app if changed, using the commands
   below. Authenticate with `cargo login` if needed; keep credentials out of
   command arguments and repository files.
5. Verify registry installation and API docs. Tag the exact release commit with
   `v<APP_VERSION>` and/or `session-activity-v<LIBRARY_VERSION>` for the crates
   released. Tags trigger artifact verification, not publication.
6. Optionally create a GitHub release with the changelog, Cargo instructions,
   and tested configurations. Attach a binary archive only if it has been
   separately verified; include its checksum and signing disclosure.

Published versions are immutable. README, documentation, or code changes in a
published package need a new version to appear on crates.io or docs.rs; pushing
to GitHub only updates the repository. See [Cargo's publishing guidance](https://doc.rust-lang.org/cargo/reference/publishing.html).

## Build and inspect

Prerequisites on Apple Silicon macOS: Xcode command-line tools, rustup,
Python 3.9+, and Node.js. The repository pins Rust 1.87.0 in rust-toolchain.toml.
Node and Python are not runtime dependencies of the installed app.

```sh
cargo fetch --locked
bash scripts/check.sh
python3 tests/cargo_install.py
```

`scripts/check.sh` runs formatting, Clippy, Rust tests, frontend syntax and
picker behavior checks, license manifest verification, Python release tests,
and the terminal smoke test.

`tests/cargo_install.py` checks Cargo's package file list, installs from the
checkout into a temporary Cargo root, and exercises setup and both agents
with isolated settings and no helper scripts. Use `--offline` with cached
dependencies. This verifies a source install; the publish dry run below
separately verifies registry packaging.

## Publish the library, if changed

Verify the standalone package from a clean checkout:

```sh
python3 scripts/package-library.py
```

This creates `dist/library/session-activity-<VERSION>.crate` and SHA256SUMS.
It verifies Cargo packaging, extracts the archive outside the workspace, and
runs its shipped tests, Clippy, rustdoc, and a read-only consumer example.
For local review of uncommitted changes with cached dependencies, use
`--allow-dirty --offline`; rebuild from a clean commit before publishing.

The library package contains its sources, tests, example, README, changelog,
and license/notice files, plus Cargo-generated metadata. It contains no
Peekback binary, frontend assets, hooks, or GUI dependencies.

```sh
cargo publish -p session-activity --registry crates-io --locked --dry-run
# Review target/package/session-activity-<VERSION>.crate, then publish:
cargo publish -p session-activity --registry crates-io --locked
```

Confirm the version on [crates.io](https://crates.io/crates/session-activity)
and its [docs.rs build](https://docs.rs/session-activity). In a fresh Cargo
project, run `cargo add session-activity@<VERSION>` followed by `cargo check`
with the released version substituted.

## Publish the app, if changed

The version of `session-activity` required by the app must already be available
on crates.io. Cargo resolves the versioned dependency from the registry when
packaging the app, instead of using the workspace path.

```sh
cargo publish -p peekback --registry crates-io --locked --dry-run
# Review target/package/peekback-<VERSION>.crate, then publish:
cargo publish -p peekback --registry crates-io --locked
```

The root manifest includes sources, embedded assets, hook templates, Rust
integration tests, documentation, and license notices. Verify the published
package on an Apple Silicon Mac, substituting the released version:

```sh
cargo install peekback --version <VERSION> --locked
peekback --version
peekback setup --dry-run
peekback setup
```

Use `peekback setup --agent codex` or `--agent claude` to select one integration.
In Codex, review/trust hooks through `/hooks`. Setup preserves unrelated
settings and invokes the installed executable directly. If upgrading, run
`peekback quit`, then open a preview to start the new daemon. Check
`type -a peekback` if an older binary elsewhere on PATH shadows the Cargo install.

## Optional unsigned archive

From a clean Apple Silicon macOS checkout:

```sh
python3 scripts/package.py
# Replace <VERSION> with the version from Cargo.toml:
python3 tests/release_archive.py dist/peekback-<VERSION>-aarch64-apple-darwin.tar.gz
```

For a local review candidate, `python3 scripts/package.py --allow-dirty` records
`dirty: true` in BUILD.json. Rebuild from the committed release revision before
publishing. BUILD.json also records the commit, Rust version, target, deployment
target, signing status, and lockfile hash. The binary's `--version` must match
Cargo.toml.

The archive contains `bin/peekback`, compatibility hook scripts/snippets,
install.sh, README, changelog, release instructions, LICENSE, NOTICE, and
dependency notices. SHA256SUMS accompanies it. The install test uses temporary
directories, including paths with spaces, without editing real agent settings
or launching a viewer. Artifacts are written under ignored `dist/`.

If attaching an archive to a GitHub release, include these user instructions
with `<VERSION>` replaced:

```sh
shasum -a 256 -c SHA256SUMS
tar -xzf peekback-<VERSION>-aarch64-apple-darwin.tar.gz
cd peekback-<VERSION>-aarch64-apple-darwin
./install.sh
peekback setup
```

The installer uses `~/.local/share/peekback` with a binary symlink in
`~/.local/bin`, which must be on PATH. Hooks are configured by `peekback setup`.

Archives contain a locally/ad-hoc signed executable, **not Developer ID signed
or notarized**. Downloaded builds may be blocked by Gatekeeper. State this in
release notes. Cargo installations build locally.

Developer ID distribution requires signing the final executable, notarizing
the distribution, and verifying it on another Mac. Regenerate the archive and
checksums, then update BUILD.json and release notes to match. Keep credentials
out of the repository. See [Apple's Developer ID guidance](https://developer.apple.com/developer-id/).

## Workflows

CI checks pull requests and master. The app release workflow accepts `v*` tags
or manual dispatch, validates the tag against Cargo.toml, runs checks and
installation tests, then uploads workflow artifacts. The library workflow
accepts `session-activity-v*` tags or manual dispatch and verifies a standalone
crate. Both have read-only repository permissions and use actions pinned to
commit IDs. Workflow artifacts are not automatically public GitHub releases.

## Manual acceptance before each release

- [ ] Install through Cargo on a clean user account or another Mac; run
      `peekback setup`. Verify the optional archive separately if distributing it.
- [ ] Record macOS version, architecture, `peekback --version`, `claude --version`,
      and `codex --version` in the release notes.
- [ ] Register a real Claude session and a real Codex CLI session using the
      generated hook commands. Confirm both appear in `peekback status`.
- [ ] Create/edit a Markdown file in each session. Confirm it appears in
      `peekback browse`; use `p` to render it in the native viewer.
- [ ] Have each agent write a Markdown file through a shell command. Confirm it
      appears as `scan` after the turn ends, not before; stays listed after you
      edit it by hand; and is still listed after ending and resuming the session.
- [ ] Interrupt a turn with Esc, write a Markdown file yourself, then send the
      next prompt. Confirm the session does not list that file. With two
      sessions working in one directory, confirm a shell-written file is marked
      `shared` in the browser and viewer.
- [ ] Open a fresh session that has written no Markdown. Confirm `show` and the
      hotkey switch the viewer to it with an empty state, and that its first
      document opens as soon as it is written. With several tmux panes, confirm
      the hotkey picks the session in the pane you last used.
- [ ] Browse `--all-sessions`. Check provenance and filtering. Previews should
      preserve the originating live session, or open standalone when there is
      none.
- [ ] Check the viewer's Scratchpad picker (`Ctrl-P`, then `Tab`) in a Claude
      Code session: it lists the scratchpad's Markdown newest first, including
      files written mid-turn, and opening one keeps the session. In a Codex
      session it says there is no scratchpad.
- [ ] Configure bookmarked files, folders, and globs. Check the Bookmarks picker
      and refresh after editing the config; opening a bookmark keeps the session.
- [ ] Check Mermaid, math, syntax highlighting, live reload, normal window
      stacking, and the global hotkey after the first `peekback show`.
- [ ] Confirm `show` and terminal previews take focus, and `show --no-focus`
      leaves keyboard focus in the terminal. Add/remove/send comments and check
      their block markers, including across live reloads.
- [ ] Copy a selection and send one through a tested terminal backend. Confirm
      it lands in the intended prompt without submitting it. Record the backend;
      keep WezTerm/Kitty marked unverified unless actually exercised.
- [ ] Kill an agent process without ending its session. Confirm the session
      drops out of `peekback status` and the pickers, sends from a viewer still
      showing it fall back to the clipboard, and `peekback status --prune`
      captures its open turn and removes it. Fresh hook activity records process
      identity; older entries without it retain the previous behavior until
      refreshed. Check this for Codex too: its hook process ancestry is not yet
      verified.
- [ ] End the sessions; verify their retained files remain queryable and live
      registry entries disappear.
- [ ] Upgrade over an earlier installation, restart the daemon, and verify the
      installed hook still resolves the new binary.
- [ ] Review dependency notices and source-availability links after dependency
      updates. See [licenses/README.md](licenses/README.md).

Homebrew distribution, Intel builds, and notarization can follow independently.
