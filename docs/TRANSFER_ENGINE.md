# Transfer Engine

Status: deterministic frontend-neutral encrypted engine, adaptive goodput
controller, bounded encryption pipeline, and concrete native-file, SQLite,
crypto, and Telegram byte-object adapters are implemented and tested.
Runtime integration covers encrypted upload, checkpoint restart, Manifest
publication, fresh-database File Key/layout recovery, authenticated download,
whole-file equality, and atomic finalization against a deterministic fake
remote. The desktop owns both a bounded durable Telegram-native download queue
and a serialized alpha Vault owner connected to Saved Messages encrypted
upload, authenticated scan, and restore download. Credentialed Telegram system
tests, physical multi-connection media-DC ownership, durable encrypted-task
controls/checkpoints, bandwidth control, and large-file encrypted streaming
remain.

## Scope

One frontend-neutral `TransferEngine` owns both upload and download work:

```text
TransferEngine
  scheduler
  upload workers
  download workers
  checkpoint/reconciliation service
  progress aggregator
  command/event boundary
```

A `TransferTask` represents one `LogicalFile`; a `TransferPart` represents one application part. Neither is an MTProto upload part. The Telegram adapter/`grammers` owns protocol-level mechanics.

The engine provides queueing, priority, pause, resume, retry, cancel, bounded concurrency, bandwidth policy, durable checkpoints, structured progress/errors, verification, and restart recovery. It never emits localized prose or GPUI types.

The cooperative `TransferEngine` owns this policy against project-owned ports
and is deterministic under an injected clock/jitter source. `teleark-runtime`
now supplies a production composition using native files, BLAKE3, SQLite,
`teleark-crypto`, and the Telegram adapter. Ordinary CI substitutes only the
remote object store, so it validates real local persistence/crypto/filesystem
behavior without claiming a credentialed Telegram network run.

## Ports and ownership

The engine coordinates narrow project-owned ports for:

- Telegram upload/download and remote discovery/verification;
- transfer/checkpoint repositories;
- bounded filesystem reads and safe destination writes;
- crypto frame encoding/decoding and manifest codec/key resolution;
- monotonic/wall clock inputs where needed;
- connectivity and cancellation signals.

Production long-running workers will require an explicit async owner and retained handles. The current engine is synchronous and cooperative: each bounded `step` admits only scheduler-approved parts, and cancellation/pause reaches a safe checkpoint boundary without spawning unbounded tasks or sleeping. Its port calls do not hold engine locks because the owner is single-threaded.

The SQLite and native-file adapters are deliberately thread-confined for
ownership by a bounded runtime worker. Their operations are synchronous and do
not contain network awaits. The Telegram byte-object adapter uses the retained
Telegram worker. Native channel downloads use a separate retained worker with a
bounded queue and locked snapshots; it never holds the snapshot lock during a
network operation. Encrypted Saved Messages work is serialized by the retained
Vault owner, which alone holds unwrapped keys and exposes frontend-neutral
snapshots. That alpha path does not yet reuse the generic engine's durable
checkpoint/control lifecycle.

## Adaptive goodput controller

`AdaptiveTransferController` owns a measured parameter envelope per DC:

```text
C = transfer connections
W = inflight RPCs per connection
F = active files
P = inflight logical parts per file
E = encryption workers
Qe = encrypted-part queue depth
```

It starts conservatively, changes one eligible parameter per probe, waits for a
configured settle interval, and records the complete before/after decision. A
gain of at least 3% is kept, 1–3% is
confirmed with another sample, less than 1% marks a platform and restores the
previous value, and a regression rolls back into recovery. RTT is evidence for
BDP and diagnostics but never causes a rollback while goodput still materially
improves. Target inflight bytes are 1.75 times the estimated BDP.

Memory use is one global byte budget subdivided into plaintext, encrypted,
network-inflight, and writer queues. Budget pressure reduces concurrency.
Network starvation by encryption prefers more encryption workers; a nearly full
encrypted queue reduces them. Soft active-file guidance is an explicit
`Respect`, `AdaptiveOverride`, or `Ignore` policy, and crossing it produces a
first-class decision event. Structured FloodWait observations pause only the
affected known lane until the exact deadline and emit a separate resume event.

Runtime adapters advertise truthful bounds. The native grammers download path
currently varies P in real time from the controller, while C, W, and F are
clamped to the single owner/connection envelope actually exposed by the
adapter. Balanced retains P4–24 with two-second probe settling. A separate native-download
Max Throughput starts at P4 and searches P1–64. Initial upward increments are
limited to the current P and 16, so healthy growth is 4 → 8 → 16 → 32 → 48 → 64.
Useful gains keep or restore fast growth. Steps of up to four parts require at
least 1% measured gain; larger steps require 3%. Weak gains or regressions get a
fresh five-second confirmation window before rollback, avoiding decisions from
overlapping one-second samples of the five-second goodput window. Strong upward
gains can still settle after one second.

A failed upward probe restores the preceding value and records an exclusive
upper bound. Further increments bisect the remaining interval down to one part,
so 48 → 64 (failure) can recover via 56 and 60 instead of oscillating between
48 and 64. Near an adjacent upper bound, or at P64, the controller also probes
the lower interval: a successful coarse jump can have skipped a better point
between its endpoints. Downward probes settle for five seconds; reductions that
lose more than 0.5% goodput are undone and narrow the lower interval. Equal
throughput permits fewer requests. Flat throughput therefore stops growth.
Both directions record actual before/probed/after values for replay.

Three isolated retry events per ten-second window remain tolerated. A burst
restores the preceding upward probe; a burst without such a probe halves P,
instead of retreating only one part per cooldown after a large capacity drop.
The owner suppresses repeated burst reductions for ten seconds. Sustained
settled goodput losses above 20% trigger downward testing; if a mid-probe rate
collapse is far larger than the proportional reduction in work, old lower
bounds are invalidated. This avoids being stranded at an obsolete high P.
A lower-P probe that hurts throughput is rolled back rather than assuming that
every link slowdown can be repaired by reducing concurrency.

Learned upper boundaries are rechecked at +1 at least 60 seconds after failure,
and completed lower-side searches are throttled for 60 seconds. Successful
rechecks can grow their steps again. Bounds are observations local to an owner,
not fixed Telegram limits. Native adapters still clamp physical C/W/F to their
real capabilities and create no additional authorization sessions. See ADR 0011.
Physical media-DC lane identities remain unavailable from the current
grammers abstraction and are never fabricated in telemetry.

## Encrypted Saved Messages desktop path

The connected upload path validates one local source, generates a fresh package
and File Key, then runs a bounded reader → encryption-worker pool → encrypted
part queue → uploader pipeline. The Reader updates the whole-file BLAKE3 while
reading each range, avoiding a separate full-file pre-scan. Encryption of following parts overlaps network
upload of the current part; queue capacity and worker count are explicit and
included in telemetry. Each bounded 60 MiB-or-smaller part is re-downloaded for
verification, and the authenticated Manifest is published only after all parts
verify. The managed Saved Messages view scans at
most 1,000 exact-caption candidates, rejects unauthenticated or malformed
manifests, and presents only reconstructed logical-file metadata. Restore
downloads use manifest-bound locators, decrypt into one controlled private
partial, verify the whole-file BLAKE3 digest, flush, and atomically publish to a
runtime-allocated non-overwriting destination.

Progress snapshots include direction, logical bytes, part counts, elapsed time,
average throughput, destination, package identity, crypto suite, structured
failure state, adaptive parameters, rates, queues, memory, and controller
decisions. A schema-versioned session log permanently records these safe events.
Vault task snapshots themselves remain memory-only and offer no pause, cancel,
retry, priority, or restart resume. A process/network failure before Manifest
publication can leave orphan encrypted parts for a future reconciliation and
cleanup flow.

## Native channel-download path

The named Channels sidebar and raw Saved Messages view query the selected dialog in exclusive-cursor
pages and keeps source/message identity on every row. A runtime-allocated path
under the configured managed `Downloads` directory becomes a bounded runtime
request. The adapter refetches the document by chat
and message identity, streams it into a private `.teleark-partial` sibling,
checks the declared byte count, flushes, and atomically publishes the final
path. Existing destinations are never silently overwritten.

The task snapshots exposed to GPUI are `Queued`, `Running`, `Paused`,
`Completed`, structured `Failed`, or `Cancelled`. The adapter schedules missing
1 MiB logical parts concurrently and fills each with the required 512 KiB
Telegram requests. Out-of-order completions use positional writes and update a
strict `TARKDPM1` completion bitmap. Transferred bytes, current speed, ETA, part
states, and the controller's desired P update live. The adapter preserves the
private partial and bitmap across network/process interruption; explicit cancel
removes both. Explicit retry accepts both failed and cancelled native tasks,
retains their task/source/destination identity, and clears cancelled progress.
If the cancelled owner is still active, retry keeps its cancellation signal
until it releases its requests and files, then starts a fresh attempt on the
same retained worker. Restored cancelled rows also support retry. This native
lifecycle is separate from the encrypted Core state machine below.
Pause interrupts at a safe request boundary, while resume requests
only missing parts. Transient network failures and FloodWait responses retry the
affected logical part up to four (Balanced) or eight (Max Throughput) total
attempts with bounded backoff; FloodWait uses the server delay. Recovery
reduces P according to the selected strategy instead of terminating the whole
file immediately. Retry and recovery decisions
are written to the session log. A per-channel batch request scans at most 50,000 messages, retains at
most 2,000 matching files, and can filter inclusively by sent-time range and
file kind. SQLite schema v7 persists bounded task history, progress, timing,
attempt, verification, failure data, source message metadata, and an optional
batch identity. The batch and all child tasks are inserted atomically. GPUI
renders the batch as one aggregate transfer row and expands its child tasks on
request; individual task controls still target the durable child identity.
The desktop presents native lifecycle controls in each row and a scope-aware
bulk toolbar. One retained, cancellable background command owner serializes a
selected batch so persistence does not block the GPUI thread. Unsupported
actions are disabled and failures are surfaced above the list. Users can delete
terminal native tasks after an explicit task-count confirmation in that toolbar. Deletion removes its database history, session log, partial, and
bitmap but never removes a successfully downloaded user file. Retryable failures
retain only a valid bitmap/partial pair with completed data; zero-progress,
incomplete, corrupt, non-retryable, cancelled, and user-deleted artifacts are
removed.
Startup restores every retained row,
normalizes interrupted running work to queued, and resumes it after Telegram
authorization/dialog discovery. The path verifies Telegram's declared byte
size rather than a content hash. Matching structured tracing and session events
omit filenames and paths.

Application shutdown first persists the latest observed byte checkpoint and
signals the observer, but never waits indefinitely for Telegram to deliver one
more network chunk. A blocked download worker is safely detached so the
desktop process can exit promptly.

## Task state machine

The Core foundation already centralizes the minimal task transition function and tests. Its states are:

```text
Queued
Running
Paused
WaitingRetry
Verifying
Completed
Failed
Cancelled
```

Allowed transitions:

| From | Allowed destinations |
| --- | --- |
| `Queued` | `Running`, `Paused`, `Cancelled` |
| `Running` | `Paused`, `WaitingRetry`, `Verifying`, `Failed`, `Cancelled` |
| `Paused` | `Running`, `Cancelled` |
| `WaitingRetry` | `Running`, `Paused`, `Failed`, `Cancelled` |
| `Verifying` | `Completed`, `Failed`, `Cancelled` |
| `Failed` | `Queued` (explicit retry), `Cancelled` |
| `Completed` | none |
| `Cancelled` | none; a new task is required |

Part state is separately modeled so verified parts survive task pause/restart. Invalid transitions such as `Completed -> Running` fail as domain errors; repositories cannot mutate states around the transition function. Network and FloodWait reasons remain structured retry/scheduler data rather than translated strings; introduce new first-class states only by updating Core, tests, and this table together. A remote RPC success is `RemoteUploaded`, not task completion.

SQLite stores the task and part states as stable symbolic values. Before replacing a task and all checkpoints in one transaction, it validates contiguous zero-based indices, gap-free offsets, total/progress agreement, checkpoint version/data pairing, and the verified-part invariant for `Completed`. Core remains authoritative for transition legality; persistence does not replace the state machine.

## Scheduler and backpressure

The scheduler enforces all active limits together:

- global worker limit;
- upload and download limits;
- per-account limit;
- per-file application-part limit;
- optional bandwidth limits;
- account-scoped Telegram FloodWait deadlines;
- network availability;
- task priority with starvation-resistant ordering.

Acquiring only one limit is insufficient: a work item starts when it owns every applicable permit. Permits are RAII/reliably released on all exit paths. The queue is bounded or durably paged; the engine never spawns one Tokio task per queued item. Priority changes reorder waiting work without interrupting verified critical sections.

Initial numeric defaults are configuration, not durable format. The deterministic scheduler tests enforce global, direction, account, and per-file limits together, plus priority/FIFO ordering and account-scoped FloodWait deadlines. Benchmark and tune them on realistic production networks before release.

## Upload pipeline

```text
inspect source and capture identity
 -> create LogicalFile/package and fresh File Key
 -> calculate application-part ranges
 -> for each scheduled part:
      bounded range reader
      -> plaintext whole-file/part BLAKE3 updates
      -> framed AES-256-GCM encoder
      -> encoded-part BLAKE3 update
      -> Telegram upload
      -> RemoteUploaded
      -> remote existence/size/identity verification
      -> durable verified checkpoint
 -> build authenticated manifest from verified locators
 -> upload and verify manifest
 -> package/task Verifying
 -> consistency verification
 -> Completed
```

The compatibility target application part size remains 1900 MiB and independent
of Telegram Premium. The current production-alpha adapter deliberately caps
plaintext parts at 60 MiB so each encoded object fits a 64 MiB in-memory safety
bound; it therefore still allocates one bounded application-part buffer. Frame
size is 8 MiB. Reaching the compatibility target requires a streaming
native-file ↔ crypto-frame ↔ Telegram pipeline before release.

### Source mutation

Capture source size, modification time, and platform filesystem identity where available before work. Revalidate at safe boundaries and after the final range. A mismatch produces structured `SourceChanged`, stops further publication, and prevents a manifest combining different source versions. The policy must account for coarse timestamp resolution and cannot rely on mtime alone where stronger identity is available.

### Remote verification

Transport success records `RemoteUploaded`. Verification then confirms the remote message/document exists, expected encoded size/identity, package/part naming and counts, and any retrievable fingerprint/hash invariant. Only verified parts become reusable checkpoints. The UI localizes and distinguishes “uploaded” from “uploaded · verified.”

## Cross-system crash consistency

Telegram and SQLite cannot commit atomically. Critical failure points include:

```text
remote upload succeeds
 -> process stops before checkpoint commit

checkpoint commits
 -> process stops before scheduler observes it
```

Remote package ID, part index, generation, and recoverable opaque name form an idempotency/reconciliation key. On restart, query the local checkpoint and remote storage before retrying an ambiguous part. If exactly one valid remote object is discovered, verify it and repair the checkpoint. Conflicts/duplicates are surfaced for deterministic resolution; never blindly upload another copy.

Durably persist state transitions/checkpoints in transactions. A checkpoint includes enough source identity, encoded length/digests, remote locator, attempts, and verification evidence to decide safely after restart. Crash injection tests exercise `AfterRemoteUpload`, `BeforeCheckpointCommit`, and `AfterCheckpointCommit`.

The SQLite checkpoint row stores part offset/size/progress/state, attempts,
optional remote-object ID, and opaque versioned non-secret checkpoint bytes.
Save operations replace the task and its parts atomically. The runtime now
adapts this repository to deterministic Telegram package/part discovery;
authenticated re-download repairs missing local evidence, and normal lifecycle
transitions are persisted. Final-output rename versus final database-commit
crash recovery still needs a dedicated failure-injection policy.

## Download pipeline

```text
resolve/authenticate manifest and File Key
 -> validate all layout/locator bounds
 -> reserve destination.partial safely
 -> schedule bounded part downloads
 -> verify exact encoded length/BLAKE3
 -> authenticate/decrypt frames
 -> verify part plaintext BLAKE3
 -> write at final plaintext offsets
 -> verify whole-file BLAKE3
 -> flush data and required metadata
 -> atomic rename destination.partial -> destination
 -> Completed
```

Use safe positional writes (`pwrite` or platform-equivalent adapter) so parts can land directly in final offsets. Do not create decrypted `.part001` files and concatenate them. Concurrency must not permit overlapping writes; validated manifest ranges prove disjointness before work begins.

The final filename must not exist as a supposedly complete result until every required authentication/integrity check passes and output is flushed. Existing destination, filesystem permissions, disk-full behavior, cancellation, and cleanup/quarantine of `.partial` files require explicit tested policies.

Resume skips already verified part output only when the checkpoint, destination
identity/length, and persisted verification evidence agree. Tests cover this
rule, exact whole-file verification, failure isolation, flush-before-finalize,
and atomic publication. The native adapter performs positional writes to one
`.partial` file, incrementally hashes source/final partial files with a fixed
1 MiB buffer, and uses same-directory atomic rename.

## Progress and frontend boundary

Workers report raw byte deltas internally. A progress aggregator computes:

- logical and encoded bytes transferred;
- current application part and verified part counts;
- moving-window speed and ETA with an explicit clock;
- direction/task state and retry/FloodWait timing.

It emits coalesced frontend-neutral events at a controlled target interval (initially approximately 100–250 ms, subject to profiling) plus immediate significant state changes. Backpressure drops/coalesces redundant progress, never terminal/error transitions. The GUI reads stable snapshots and updates GPUI state on its supported thread.

## Error taxonomy

The target structured taxonomy includes network, FloodWait with duration, authorization, source missing/changed, disk full, permission denied, remote missing/conflict, hash mismatch, authentication failure, corrupt/unsupported manifest, unavailable key, wrong password, database, cancellation, and internal invariant failures. Each error exposes safe classification such as retryable, user-action-required, corruption, key problem, or fatal. Business logic never parses strings, and safe context never contains secrets.

Retry uses bounded attempts/backoff with jitter from an injectable source and respects server-provided FloodWait. Authentication, manifest corruption, source mutation, and integrity failures do not retry blindly.

## Testing requirements

Implemented deterministic storage coverage verifies transactional checkpoint
round trips, invalid-offset rollback without data loss, layout validation, and
interrupted-state requeueing. `teleark-transfer` adds 25 deterministic tests
covering scheduler limits, FIFO/priority, cooperative controls, bounded retry,
FloodWait, restart, source mutation, ambiguous remote success, native file
identity/positional writes, hash isolation, and atomic download publication.
Runtime integration adds real SQLite, BLAKE3, encrypted parts, authenticated
Manifest recovery, and exact download equality over a deterministic remote.

Use a fake Telegram transport, temporary checkpoint store, fake clock, controlled file adapter, deterministic test data, and failure injection. Required tests include:

- every valid/invalid task and part state transition;
- global/direction/account/file concurrency never exceeding configured limits;
- priority changes, fairness, pause/resume/cancel, network loss, and FloodWait;
- partial/transient/permanent upload/download failures and bounded retry;
- restart skips verified parts;
- ambiguous remote success reconciles without duplicate upload;
- source change aborts before the next part/manifest;
- wrong key/tamper/hash mismatch/missing part never exposes final path;
- non-overlapping positional writes and final BLAKE3;
- progress throttling and terminal events under channel pressure;
- complete fake-remote upload, database-loss recovery, and download equality.

Ordinary CI never needs a Telegram account. Protected real-network tests are added only after fake tests pass and must use dedicated credentials outside the repository.

## Native Max Throughput retry scheduling

The throughput strategy is independent of `SoftLimitPolicy`. Native downloads
capture the persisted strategy at each owner start (including resume/retry).
Balanced allows four attempts per part; Max Throughput allows eight. Network
failures back off from 250 ms exponentially to an eight-second ceiling. Delayed
parts remain in the bounded missing-part queue while successful inflight parts
continue to complete and write; one retry no longer blocks the entire writer.
The event loop polls cancellation at most every 50 ms while waiting for parts
(excluding filesystem work). JoinSet ownership cancels outstanding reads on exit.

Max Throughput tolerates three retry events in a ten-second window, then
reduces P once and suppresses further reductions/probes for ten seconds during
that burst. Server waits bypass this tolerance: P reduces immediately and
probing remains suppressed through the wait plus ten seconds. Every protocol
chunk checks a shared native-download gate; a received FloodWait immediately
extends it, never shortens it. The gate lives on the Telegram connection so
next native tasks/resumes cannot erase it. Already sent RPCs may still finish.
FloodWait retry durations are never capped by the network backoff ceiling;
authorization and nonrecoverable failures are never retried. An exhausted
retry budget terminates the task rather than bypassing a wait. The gate is
in-memory and does not coordinate independent connections or process restarts.

Bounds remain 64 inflight logical MiB parts and the existing 512 MiB controller
budget; protocol chunks remain 512 KiB. No persistent part-map, checkpoint, or
session-log encoding changes. Deterministic tests validate ramp, regression,
retry tolerance, memory protection, retry bounds and shared deadlines. Real
Telegram throughput and account-limit incidence still require credentialed
measurement; synthetic tests are not bandwidth benchmarks.
