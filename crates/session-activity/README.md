# session-activity

A Rust library for file activity observed during Claude Code and Codex
sessions. It normalizes supported tool activity, preserves the evidence behind
each path, and retains history independently of the calling application.
It is developed alongside Peekback and published separately on crates.io;
it has no GUI, Markdown, terminal, or Peekback runtime dependency.

```toml
[dependencies]
session-activity = "0.1"
```

Requires Rust 1.87 or newer. The supported storage/maintenance platforms are
macOS and Linux. The library is not currently validated on Windows, where
maintenance is unavailable. API reference: <https://docs.rs/session-activity>.

The library owns:

- `SessionKey`: agent plus native session ID.
- `FileEvent`: versioned file metadata, operation, outcome, timestamp, source,
  working directory, optional tool call ID, and rename origin.
- Adapters for supported `PostToolUse` payloads, Claude `PostToolUseFailure`,
  and Claude transcripts, including tool-result correlation.
- `Store`: retained event batches under a caller-supplied directory.
- `Store::sessions` and `Store::read_all`: enumerate and read retained history
  across both agent namespaces, independently of a live-session registry.
- `scan_report`: bounded filesystem candidates and completeness diagnostics
  from caller-supplied roots (`scan` returns only the events).
- `Store::maintain`: preview or apply compaction and timestamp retention.
- `files`: grouping observations by path without losing their evidence.

Peekback owns the hook command, live session registry, terminal metadata,
choice of scan roots, Markdown filtering, and labels. The library does not
read a global configuration or choose its own storage location.

```rust
use session_activity::{Agent, SessionKey, Store, files};

// The caller chooses the directory; the library never discovers global state.
let store = Store::new(std::env::temp_dir().join("my-app-session-activity"));
let session = SessionKey {
    agent: Agent::Codex,
    session_id: "native-session-id".into(),
};
let report = store.read(&session)?;
for warning in &report.warnings {
    eprintln!("{}: {}", warning.path.display(), warning.message);
}
let touched_files = files(report.events);
# Ok::<(), anyhow::Error>(())
```

To collect a hook, call `hook_events(agent, &payload, unix_seconds)` and pass
its result to `store.append(&session, &events)`. Only normalized metadata is
stored; the raw hook payload and document contents are discarded.

`cargo run --example inspect -- /path/to/activity codex SESSION_ID` prints
retained file summaries as JSON and reports incomplete history on stderr.
The example reads a store; it does not install hooks or start a viewer.

## API and data compatibility

Versions `0.1.x` retain compatible public API and storage behavior. Breaking
API changes require a new minor version while the crate is pre-1.0. Storage
schema versions are independent of crate versions: events currently use schema
1, and checkpoints use version 1. Unsupported schemas produce errors or batch
warnings rather than being silently interpreted as current data. Preserve old
history until a documented migration exists.

Events describe observations, not a complete audit of a session's filesystem
effects. Errors use `anyhow::Result`; error message wording is diagnostic and is
not a stable API. `Store::events` is strict, while `Store::read` returns healthy
events with warnings. A malformed authoritative checkpoint remains a hard error.
`transcript_events` is best-effort: an unavailable transcript or malformed
records can yield an empty or partial result without a diagnostic report.

Operations are `read`, `write` (create or replace), `create`, `modify`, `delete`,
`rename`, and `observed`. Claude `Read`, `Write`, `Edit`, and `MultiEdit` are
recognized. Codex `apply_patch` headers supply create/modify/delete/rename
operations. Arbitrary shell commands and Codex transcripts are not parsed.
Rename events carry the destination in `path` and origin in `previous_path`.
File queries include both, even when either path no longer exists.

Sources distinguish hooks, transcript records, legacy registry paths, and
project/scratchpad/memory scans. Structured `success` or `is_error`/`isError`
booleans can establish an outcome; explicit failure wins over contradictory
success flags. Claude failure hooks establish failure directly. Transcript
requests are matched to `tool_result` blocks by call ID; unmatched requests
retain unknown outcomes. Relative transcript paths use a record's cwd when
available. A scan only observes file modification times. Consumers
decide which operations, outcomes, and sources count for their use case.

`reconcile` connects known outcomes to unknown observations for the same
session, call ID, operation, and paths. Conflicting known outcomes remain
visible, and scan observations are never promoted to attributed writes.
It collapses retries within each evidence source, retaining observations from
both hooks and transcripts. Calls without IDs remain separate. `files` applies
this automatically; consumers filtering failed operations should call
`reconcile` before filtering. The retained raw event history is unchanged.

Storage uses one immutable JSON batch per hook invocation under
`ROOT/AGENT/SESSION_ID/`. Private temporary files are renamed into place after
writing, so concurrent writers do not replace each other's observations. Files
are synced before publication; on Unix, newly created directories and the
directory containing the published batch are also synced. This strengthens
crash durability within the filesystem's sync guarantees.
Temporary files left by an interrupted writer are ignored. `Store::read`
returns healthy events plus warnings naming invalid, oversized, unreadable,
or unsupported batches, leaving damaged data untouched. `Store::events`
remains strict and returns an error if any batch is incomplete. Both reads
and writes validate session identity, schema, absolute paths, and rename
origins; batches are limited to 16 MiB. History is retained unless the owner
explicitly applies retention; session exit does not delete event files. Retried hook
calls can produce repeated observations, identifiable by tool call IDs when
the agent supplies them. Reconciled file queries collapse those retries.

`store.maintain(&session, None, false)` previews compaction; pass `true` as the
last argument to apply it. Supplying `Some(unix_seconds)` also drops events
strictly older than that cutoff. The report includes batch/event counts and
cleanup warnings. A persistent cutoff, exposed by `Store::retained_from`,
filters subsequent appends and reads; callers importing transient observations
should apply it too. Later maintenance can advance but never lower the cutoff.
Maintenance of an absent session is a no-op.

Maintenance preserves raw events without reconciliation and refuses any
damaged batch. It writes synced `.pack` chunks (each at most 16 MiB), then
atomically publishes a synced `.checkpoint` before deleting original batches.
The checkpoint identifies covered originals, making interrupted cleanup safe
to retry; uncommitted packs are ignored. A malformed checkpoint is a hard error
because the authoritative history cannot safely be inferred.

On Unix, readers and appenders hold shared advisory locks; maintenance holds
an exclusive lock. Lock files must not be deleted while the store is in use.
Maintenance is unsupported on other platforms. All consumers must understand
the checkpoint format before maintenance is applied: older binaries see only
uncompacted batches. Reads and maintenance still load the full retained history
into memory; compaction reduces filesystem overhead, not event parsing cost.

`store.sessions()` returns sorted session keys plus enumeration warnings. It
skips namespace/session symlinks and reports invalid directory names or access
failures. `store.read_all()` returns all healthy retained events and warnings;
a damaged checkpoint is reported for its session while other sessions remain
available. Pass its events to `files` to group by path across sessions; each
event retains its agent/session identity. These queries include compacted and
ended sessions, honor retention cutoffs, and do not perform filesystem candidate
scans. `read_all` loads all retained history into memory and locks each session
separately, so it is not a globally atomic snapshot of concurrent activity.

Paths are resolved lexically against the session cwd, including `.` and `..`;
symlink aliases are not unified. The library stores metadata, not file content
snapshots, and `exists` reflects the filesystem at query time. Scans skip hidden
and build directories, do not follow symlink directories, and share a caller-
supplied entry budget across roots. Callers should avoid scanning `/` or home.
`scan_report` reports visited entries, budget exhaustion, depth limits, and
path-specific I/O warnings. Missing optional roots are ignored; other failures
are reported. Completeness is relative to the caller's roots and exclusions,
not proof that every session-touched file was found.

```sh
cargo test -p session-activity
```
