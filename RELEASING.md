# Releasing Peekback

Cargo is the primary distribution method: publish `session-activity` first,
then the `peekback` app. Users run `cargo install peekback --locked` followed by
`peekback setup`. Both crates allow crates.io publication and share a repository,
with independent versions and tags. An unsigned Apple Silicon archive remains
an optional download for users without Rust.

Preparation scripts do not change repository visibility, create tags, or upload
releases. The app remains macOS-only even though it is distributed through Cargo.

## Supported and validated configurations

| Component | Scope / evidence |
| --- | --- |
| Release archive | `aarch64-apple-darwin`, deployment target macOS 14.0 |
| Local build/test host | Apple Silicon, macOS 26.3, Rust 1.87.0 |
| CI configuration | macOS 15 ARM runner; CI results become available after pushing |
| Library CI configuration | Standalone package tests/docs on Linux (Ubuntu 24.04) and macOS 15; results available after pushing |
| Claude Code | Terminal integration; installed version observed: 2.1.280; real-session acceptance test still required |
| Codex CLI | Lifecycle-hook terminal integration; installed version observed: 0.156.1; real-session acceptance test still required |
| Agent payload coverage | Synthetic hook/transcript fixtures, both installed hook lifecycles, retained history |
| Terminal UI | Automated pseudo-terminal test; preview IPC uses a fake viewer |
| Other platforms | No Intel/Linux/Windows binaries or desktop/IDE composer support in this release |

The agent versions above are a local inventory, not claimed minimum supported
versions. Record actual end-to-end test versions below before publishing.
The deployment target is a compatibility baseline, not proof of testing on every
macOS version. Intel support needs a separate build and runtime test.

## Build and inspect

Prerequisites: Xcode command-line tools, rustup, Python 3.9+, Node.js for the
JavaScript syntax check. The repository pins Rust 1.87.0 in rust-toolchain.toml.
Node and Python are not runtime dependencies of the installed app.

```sh
cargo fetch --locked
bash scripts/check.sh
python3 tests/cargo_install.py
# Optional binary archive:
python3 scripts/package.py
python3 tests/release_archive.py dist/peekback-0.1.0-aarch64-apple-darwin.tar.gz
```

Packaging requires a clean checkout. During development, use
`python3 scripts/package.py --allow-dirty` to make a review candidate. Its
BUILD.json records `dirty: true`; rebuild from the committed release revision
before publishing. BUILD.json also records the commit, Rust version, target,
deployment target, signing status, and lockfile hash. The binary's `--version`
must agree with Cargo.toml.

The archive contains `bin/peekback`, hook scripts and snippets, install.sh,
README, changelog, release instructions, LICENSE, NOTICE, and dependency notices.
SHA256SUMS accompanies the archive. The install test uses temporary directories,
including paths with spaces; it does not edit real agent settings or launch a
viewer. All artifacts are written under ignored `dist/`.

CI checks pull requests and master. The release workflow runs on `v*` tags or
manual dispatch, verifies the tag against Cargo.toml, runs the checks, and
uploads workflow artifacts. It has read-only repository permissions and does
not publish a release. Actions are pinned to commit IDs.

## Cargo application release

`tests/cargo_install.py` checks Cargo's package file list, installs from the
checkout into a temporary Cargo root, and exercises setup and both hook agents
with isolated settings and no helper scripts. Use `--offline` with cached
dependencies. CI runs this on Apple Silicon. This source-install check works
before the first library publication; it is not a registry package verification.
The root manifest explicitly includes sources, embedded assets, hook templates,
Rust integration tests, documentation, and license notices.

From a clean checkout, after the library release below is available on crates.io:

```sh
cargo publish -p peekback --registry crates-io --locked --dry-run
# Review the generated target/package/peekback-0.1.0.crate, then publish:
cargo publish -p peekback --registry crates-io --locked
cargo install peekback --version 0.1.0 --locked
peekback setup --dry-run
peekback setup
```

The app's first package/publish dry run must wait for `session-activity`: Cargo
resolves the versioned dependency from the registry when preparing the published
app, rather than using the workspace path. Recheck ownership/availability of
both names before publishing; both were unregistered during this preparation,
which does not reserve them. No token or upload is needed for local source
installation. Authenticate outside this conversation using `cargo login` when
ready to publish; never commit credentials.

For users testing the checkout before publication:

```sh
cargo install --path . --locked
peekback setup
```

Use `peekback setup --agent codex` or `--agent claude` to select one integration.
Setup preserves unrelated settings, backs up changes, and invokes the installed
executable directly. In Codex, users must review/trust hooks through `/hooks`.
The release does not automatically modify trust or hook-disable settings.

## Standalone library release

The crate contains only its sources, tests, example, README, changelog, and
license/notice files (plus Cargo's generated manifest, lockfile, and VCS
metadata). It has no Peekback binary, frontend assets, hooks, or GUI dependencies.

```sh
python3 scripts/package-library.py
# Local review of uncommitted changes, using cached dependencies:
python3 scripts/package-library.py --allow-dirty --offline
```

This creates `dist/library/session-activity-0.1.0.crate` and its own SHA256SUMS.
It verifies Cargo packaging, then extracts the archive outside the workspace
and runs the shipped tests, Clippy, rustdoc, and a read-only consumer example.
CI performs the same check on macOS and Linux. The separate library-release
workflow accepts `session-activity-v*` tags and uploads a reviewable crate artifact.

Before first publication, ensure a crates.io account with a verified email is
ready and recheck availability/ownership of `session-activity`. It was
unregistered when checked during this preparation; that is not a reservation.
Authenticate using `cargo login` outside the conversation; do not put the token
in a workflow, command argument, or repository file.

From the reviewed, clean checkout, after making the linked source repository
public and passing CI:

```sh
cargo publish -p session-activity --registry crates-io --locked --dry-run
# Final, explicit publication step:
cargo publish -p session-activity --registry crates-io --locked
```

Confirm the registry entry and docs.rs build. In a fresh project, check
`cargo add session-activity@0.1.0` and build it to verify registry consumption.
The real publish operation is not performed by preparation workflows. Published
crate versions are immutable; follow [Cargo's publishing guidance](https://doc.rust-lang.org/cargo/reference/publishing.html).

Library releases use `session-activity-v0.1.0`; app releases use `v0.1.0`. For
later releases, update the relevant changelog and version independently. Keep
Peekback's `session-activity` version requirement compatible with its local path
dependency. The initial two tags should point to the same reviewed commit.

## Signing policy for the preview

The archive contains a locally/ad-hoc signed executable. It is **not Developer
ID signed or notarized**. Downloaded builds may be blocked by Gatekeeper. State
this in release notes; do not describe the archive as signed for distribution.
Users who prefer a local build can follow the README source installation.

For a signed distribution, a maintainer must supply Developer ID credentials,
sign the final executable with a secure timestamp and appropriate hardened
runtime settings, notarize the distribution, and verify it on another Mac.
Then regenerate the archive/checksums and accurately update BUILD.json and
release notes. Do not put certificates or credentials in the repository.
See [Apple's Developer ID guidance](https://developer.apple.com/developer-id/).

## Manual acceptance before publishing

- [ ] Install through Cargo on a clean user account or another Mac; run
      `peekback setup`. Verify the optional archive separately if distributing it.
- [ ] Record macOS version, architecture, `peekback --version`, `claude --version`,
      and `codex --version` in the release notes.
- [ ] Register a real Claude session and a real Codex CLI session using the
      generated hook commands. Confirm both appear in `peekback status`.
- [ ] Create/edit a Markdown file in each session. Confirm it appears in
      `peekback browse`; use `p` to render it in the native viewer.
- [ ] Check a Mermaid diagram, math, syntax highlighting, live reload, and the
      global hotkey after the first `peekback show`.
- [ ] Copy a selection and send one through a tested terminal backend. Confirm
      it lands in the intended prompt without submitting it. Record the backend;
      keep WezTerm/Kitty marked unverified unless actually exercised.
- [ ] End the sessions; verify their retained files remain queryable and live
      registry entries disappear.
- [ ] Upgrade over an earlier installation, restart the daemon with
      `peekback quit`, and verify the installed hook still resolves the new binary.
- [ ] Review dependency notices and source-availability links after any dependency
      update. See licenses/README.md.

## Publish after review

1. Finish the acceptance checklist and CHANGELOG.md release date/status.
2. Commit the intended source, tests, workflows, assets, and notices. Review the
   changes before making the currently private repository public.
3. Update the GitHub description to: “Browse Claude Code and Codex session files
   in the terminal, with a native Markdown preview.”
4. Push the reviewed commit and wait for app and library CI. Tag it `v0.1.0`
   and `session-activity-v0.1.0`; inspect both workflows' artifacts/checksums.
5. Make the repository public when ready, publish the library, verify its
   registry/docs.rs pages, then dry-run and publish the app. Verify a fresh
   `cargo install peekback --locked` and `peekback setup`.
6. Create a GitHub release with Cargo installation instructions, changelog, and
   tested matrix. If attaching the optional unsigned archive, include its
   SHA256SUMS and signing disclosure.

Homebrew distribution, Intel builds, and notarization can follow independently.
The first app archive is intentionally not Developer ID signed or notarized.
