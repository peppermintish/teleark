# Transfer Engine

Status: deterministic frontend-neutral engine, Core state machines, and SQLite checkpoint primitives are implemented and tested. `teleark-transfer` provides bounded scheduling, priority/FIFO ordering, pause/resume/cancel, structured retry/FloodWait handling, checkpoint and I/O ports, progress coalescing, source-change detection, ambiguous-success reconciliation, and safe `.partial` download finalization against fakes. Concrete Telegram/SQLite/crypto/filesystem adapters, async production workers, bandwidth control, and large-file streaming remain; GUI transfer values are still synthetic.

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

The cooperative `TransferEngine` now owns this policy against project-owned ports and is fully deterministic under an injected clock/jitter source. Its shipped I/O implementation is test support using bounded in-memory fake parts and a non-cryptographic digest, so persisted queue/state data and passing fake tests are not evidence that production bytes were uploaded, downloaded, decrypted, or verified through Telegram.

## Ports and ownership

The engine coordinates narrow project-owned ports for:

- Telegram upload/download and remote discovery/verification;
- transfer/checkpoint repositories;
- bounded filesystem reads and safe destination writes;
- crypto frame encoding/decoding and manifest codec/key resolution;
- monotonic/wall clock inputs where needed;
- connectivity and cancellation signals.

Production long-running workers will require an explicit async owner and retained handles. The current engine is synchronous and cooperative: each bounded `step` admits only scheduler-approved parts, and cancellation/pause reaches a safe checkpoint boundary without spawning unbounded tasks or sleeping. Its port calls do not hold engine locks because the owner is single-threaded.

The SQLite adapter is deliberately thread-confined for ownership by a bounded runtime worker. Its operations are synchronous and transactional and do not contain network awaits. Adapters connecting the transfer ports to that repository, the current Telegram adapter, production crypto streaming, and native filesystem operations remain unimplemented.

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

The target application part size is 1900 MiB and independent of Telegram Premium. Frame size is smaller and bounded as specified in `CRYPTO_FORMAT.md`. The engine does not create `/tmp/file.part001` or read a whole part into memory. It avoids a separate full-file hash pass by updating hashes while streaming the source in a deterministic plan; if concurrent part reads make whole-file order difficult, the implementation must preserve ordered whole-file hashing without unbounded buffering or justify a measured alternative.

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

The SQLite checkpoint row stores part offset/size/progress/state, attempts, optional remote-object ID, and opaque versioned non-secret checkpoint bytes. Save operations replace the task and its parts atomically, and a failed validation/write leaves the previous checkpoint set intact. Separately, the generic engine checkpoint port carries source identity, digest, remote-object, attempt, and verification evidence; fake crash-injection tests reconcile remote success before a missing local commit without duplicating a part. These two foundations are not yet adapted to each other or to real Telegram discovery, so production cross-system reconciliation remains incomplete.

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

Resume skips already verified part output only when the checkpoint, destination identity/length, and persisted verification evidence still agree. The fake-backed engine tests this rule, exact whole-file verification, failure isolation, flush-before-finalize ordering, and atomic publication; a native large-file positional-write adapter is still required.

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

Implemented deterministic storage coverage verifies transactional checkpoint round trips, invalid-offset rollback without data loss, layout validation, and interrupted-state requeueing. Core separately tests valid and invalid task/part state transitions and completion invariants. `teleark-transfer` adds 22 deterministic tests covering all scheduler limits, FIFO/priority, cooperative controls, bounded retry and FloodWait, progress coalescing, restart skip/revalidation, source mutation, ambiguous remote success without duplicates, hash failure isolation, and atomic successful download publication.

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
