# Synchronization recovery and manifest reuse — 2026-10-23

## Scope

Prepared release 0.5.20 fixes three independently observed problems: historical
30-second admission that recovered only after many successful pages, repeated
manifest downloads after a process restart, and ambiguous shell status during
subscription renewal. This is not a claim that all protocol work is eliminated
or that initial discovery, genuine gap repair and server subscription traffic
are unnecessary. No external client implementation was inspected.

## Behavior and deterministic evidence

The account budget preserves active server deadlines and doubles its interval
once per new cooldown window, but halves local spacing after each elapsed
minute outside the server wait. At most five windows recover 30 seconds to the
one-second floor. Restart includes elapsed wall time; complete windows are
applied without assuming the newly booted monotonic clock can represent the
whole offline age. Admission recalculates from the last actual start and cannot
release queued work in a burst. Ordinary network errors do not restart recovery.
Controlled clocks cover the exact minute boundaries, overlapping later waits,
queued deadline changes, cancellation, network errors and expired/active saved
startup cooldowns.

SQLite schema 27 adds an account/channel/message/source-version-bound encrypted
manifest cache. Production point queries reject edits, deletion evidence and
unresolved repair/gap work; atomic writes reject stale versions. Every hit is
authenticated and checked for manifest identity and locator scope. Cold-restart
operation-count tests forbid a download when the saved current envelope is
unchanged, require exactly one refetch for damaged cached bytes, forbid futile
refetch for an unavailable key and reject wrong-key plaintext. Original recovery
inventory remains independent and preserved when current cache bytes change.
The actual production lookup's EXPLAIN QUERY PLAN regression requires scoped
primary-key searches without table scans or temporary sorting.

Existing synchronization acceptance tests retain zero difference requests for
32 unchanged channels over repeated launches/reconnection, exactly targeted
requests after one head changes, and direct/duplicate push behavior without
redundant RPC. Existing in-memory manifest tests load only changed candidates
and remove deleted projections. New storage tests cover same-metadata edits,
account/channel isolation, gaps/repairs, deletion and stale writes. Automatic
schema-27 conversion is exercised across prior-version/skipped upgrade fixtures,
rollback/restart and preservation of original encrypted inventory/settings.

The shell separates caught-up content from periodic subscription execution and
names local pacing, Telegram cooldown and retry waits. Directory preparation,
real recovery, catalog work and failures cannot be reported as current. GUI
projection application must finish before shell readiness. English interface
tests cover 900×600 and native full-screen state flags. The informational
inspector retains live detailed phases and its bounded history.

## Compatibility

Read/write SQLite schema 27; ordered automatic upgrades 0–26 → 27, including
skipped releases, with verification before transactional commit. Existing
inventory has no reliable current-source-version binding, so the first catalog
load warms the new cache once; later unchanged cold starts reuse it. Original
recovery envelopes, downloaded-file inventory, destinations, settings, keys and
recoverable transfers remain intact. Payload, container, manifest, key/recovery
and transfer codecs are unchanged. Cooldown codec v1 and its existing setting
update timestamp retain their meanings. Dependency changes are limited to the
nine internal package versions; there are no new external dependencies/licenses.

## Validation state

The final locked workspace/all-target suite passes: **1,024 passed, zero failed,
10 existing ignored**, across ten suites. Focused synchronization/pacing,
authenticated cache cold-restart/corruption and storage regressions are included.
Locked workspace/all-target checking, strict Clippy, formatting, explicit Core
and i18n tests, warning-denied rustdoc and cargo-deny pass. LF checks and synthetic
Store configuration, workflow graph, MSIX, macOS library and Linux payload script
fixtures pass; 29 Python Store/release tests pass.

The local release-asset assembly fixture cannot run under the host's Bash 3:
its existing associative-array/GNU-find requirements are Linux CI scope. This
is not counted as a local pass. The tagged Linux CI quality job subsequently passed this gate. GitHub API
credentials cannot dispatch a workflow, create a PR or rerun a job, although
authenticated Git branch pushes work. Publication remains gated as described below.

After the Mac was unlocked, native English/light synthetic previews were
inspected at 900×600 and in actual native full-screen mode (1440×900 logical
points, with the native Exit full screen control). Live subscriptions show
“Up to date · Live updates”, local admission shows “Waiting for next request”,
and server cooldown shows “Waiting for Telegram”. Detailed reasons, phase age,
last activity and bounded timeline remain visible. Inspector scrolling leaves
the file list in place, Escape closes the inspector, and primary file actions
remain reachable. All three states were reviewed at both sizes. The preview
process was closed afterward; only the original real instance remained running.
The user's real instance has not been restarted, reconfigured or migrated by
this task. No real-account end-to-end speed measurement is claimed for the new
release; deterministic operation counts establish the specified reuse behavior.

## Tagged release attempt

Annotated `v0.5.20` and `main` were pushed atomically to `beb5988`.
[Tagged CI attempt 1](https://github.com/peppermintish/teleark/actions/runs/39845882949)
completed with repository quality and every Windows/macOS/Linux package job
successful. Native Linux tests pass (1,015 passed, 10 existing ignored), as do
native Windows tests (997 passed, 10 existing ignored); platform totals differ
because of existing conditional tests. Both native jobs also pass locked
checking, strict Clippy, explicit Core/i18n and warning-denied documentation.

The macOS native test job never received a runner and was canceled after repeated
allocation attempts. Its annotations explicitly report hosted macOS ARM capacity
constraints. This is a missing gate, not a test pass. GitHub Release publication
and Store delivery were therefore skipped; no v0.5.20 GitHub Release exists yet.
The seven package artifacts are CI outputs, not a published release asset set.
The macOS universal package job succeeded, including its native signing checks,
but does not replace the full native test job.

The current API account's rerun request returned HTTP 403 requiring repository
administration rights. The available browser session is signed out. A repository
administrator must select **Re-run failed jobs** on the completed run to retry
the canceled test and its downstream publication. Successful package jobs should
be retained. No tag was moved, failed gate bypassed, credentials changed or paid
runner enabled. Final release/asset verification remains outstanding until that
retry succeeds.
