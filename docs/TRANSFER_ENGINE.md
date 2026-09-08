# Transfers and diagnostics

Three implementations must remain distinct: the deterministic Core transfer engine, the connected durable native-download owner, and the connected Vault owner. The first provides tested ports/policy; the latter two define currently exposed desktop capabilities. [Status](IMPLEMENTATION_STATUS.md) records remaining integration work.

## Ownership and ports

A task is one logical file; a transfer part is one application part, not an MTProto unit. Core uses project-owned transport, filesystem, checkpoint, clock/jitter and progress ports. Storage/native-file adapters are thread confined; Telegram requests use a retained bounded owner. No GUI types or localized prose cross these boundaries, and snapshot locks are not held during network operations.

The cooperative generic engine advances through bounded steps. Its scheduler requires global, direction, account and file permits together, uses priority/FIFO fairness, obeys account-scoped FloodWait and persists verified evidence at safe pause/cancel/restart boundaries. It never spawns one task per queued item.

## Connected native downloads

Managed destination allocation checks both the filesystem and the indexed native-history destination column, across all task states and accounts. A deleted output does not free its historical path; a fresh download receives a numbered path and a new task while preserving prior history. Allocation is bounded to 10,000 candidates and fails closed on storage errors. This is candidate selection, not an atomic reservation; insertion constraints and final no-overwrite publication still reject concurrent collisions.

Requests carry positive account/chat/message identity and a runtime-allocated non-overwriting destination under managed Downloads. The adapter validates the actual account and refetches source identity before transport. Schema 9 records account scope; [Data model](DATA_MODEL.md) specifies one-time legacy attribution. Unknown/other-account work cannot enqueue, resume or retry. Switching pauses eligible work, waits up to 30 seconds for retained workers to release requests/partials, then signs out; failure leaves a visible error rather than switching under active work.

Native states are Queued, Running, Paused, Completed, Failed and Cancelled. Resume requests missing ranges; retry accepts Failed and Cancelled while retaining task/source/destination identity. Cancelled retries clear cancelled progress and wait for the previous worker to retire before starting. This lifecycle is separate from the generic Core transition table below.

The adapter fills 1 MiB logical parts using hard-limit-compatible 512 KiB Telegram requests and writes out of order at explicit offsets. A private sibling partial and `TARKDPM1` bitmap record completed ranges. The exact format remains [ADR 0009](adr/0009-transfer-part-map-and-session-log.md): big-endian magic/header, fixed logical part size, total length, part count, bitmap length and valid trailing bits. Reject malformed/inconsistent/trailing data; never infer completion from a sparse file's length.

Pause/interruption and retryable failures retain only a valid partial/bitmap pair with completed data. Zero-progress, corrupt, incomplete, nonretryable and cancelled artifacts are removed. Completion requires every bit, exact expected byte length, flush/sync and atomic no-replace publication. Native verification is byte-count/finalization safety, not cryptographic content authentication.

History persists task/batch identity, original message metadata, progress/timing, attempts, verification and structured failure. A batch inserts its header and all children atomically. Startup normalizes interrupted Running rows to Queued, then schedules eligible account work only after account entry/source refresh. Terminal task deletion requires confirmation and removes owned history/log/partial/bitmap artifacts, never a completed user file. Bulk commands have a retained cancellable owner and surface partial failures.

Shutdown persists the latest observed progress and signals cancellation without waiting indefinitely for another network chunk. A blocked worker may detach for prompt process exit; the private part map remains restart authority.

## Adaptive policy and retries

`AdaptiveTransferController` measures goodput over real capabilities:

| Symbol | Parameter |
| --- | --- |
| C / W | Connections / inflight RPCs per connection |
| F / P | Active files / inflight logical parts per file |
| E / Qe | Encryption workers / encrypted-part queue depth |

Memory is globally budgeted across plaintext, encrypted, inflight and writer queues. Pressure reduces concurrency. Estimated BDP and target inflight bytes (1.75× BDP) are diagnostics/input, not reasons to roll back useful goodput. Probe one eligible parameter at a time and retain before/probed/after/reason for replay. Never invent media-DC lanes or concurrency the adapter cannot expose.

Balanced native downloads use P4–24 and two-second settling. Max Throughput starts at P4, searches P1–64 and grows by at most current P and 16. Fine probes need 1% gain, coarse probes 3%; strong gains settle after one second, weak gains receive a fresh five-second confirmation. Failed probes narrow an upper interval to one-part precision. Downward probes search skipped peaks and settle for five seconds; losses over 0.5% roll back. Boundaries/lower searches are revisited on a 60-second cadence. [ADR 0011](adr/0011-adaptive-native-probe-refinement.md) governs exact policy and supersedes ADR 0010 numerical tuning.

Balanced allows four attempts per part; Max Throughput allows eight. Network backoff starts at 250 ms and caps at eight seconds. Delayed parts stay in a bounded queue while healthy reads continue. Max Throughput tolerates three isolated retries per ten seconds; a burst rolls back an upward probe or halves P, with a ten-second cooldown. Server waits reduce P immediately and suppress probes through the deadline plus cooldown.

Every protocol chunk checks a shared connection-owned native FloodWait gate. A new deadline can extend but never shorten it; already-sent requests may finish. Server waits are never capped by network-backoff policy. The gate survives native task changes in that connection, but not process restart or independent connections. Authorization/integrity/source failures do not blindly retry.

`Respect`, `AdaptiveOverride` and `Ignore` govern advisory active-file guidance only. They never override protocol limits or server deadlines. Throughput strategy is captured at task start/resume/retry independently of this policy. Native limits remain 64 logical MiB parts and the controller's 512 MiB budget. Synthetic capacity models are regression tests, not real bandwidth benchmarks.

## Connected encrypted storage

New uploads validate the account's bound, currently owned private TeleArk channel before touching plaintext/keys. Saved Messages is available only as an explicit legacy recovery source. The Vault owner alone retains the unlocked Master Key and serializes create/unlock/lock/rewrap/recovery and transfer requests.

Upload uses one source-identity-checked pass: reader and whole-file BLAKE3 → bounded encryption pool → bounded encrypted-part queue → uploader. Following-part encryption overlaps upload. The desktop ceiling is 60 MiB plaintext per part with 8 MiB frames and a 64 MiB encoded-object bound; the 1900 MiB compatibility target still needs larger streaming transport. Each part is re-downloaded and verified before the final authenticated manifest is published. Source mutation fails with structured `SourceChanged` and cannot create a manifest mixing versions.

Managed scanning considers at most 1,000 exact-caption candidates. Manifest discovery merges indexed exact-caption matches with exact matches from the latest 512 messages read directly from Telegram history. Candidates are deduplicated by message ID, recent observations win, and the newest 1,000 candidates remain within the existing bound. This avoids waiting for search visibility after publication; ordinary part reconciliation does not add a history scan. Both sources still require the same manifest download, AEAD authentication, name and strict layout validation. Names/captions classify candidates only; authentication and strict layout validation establish a logical file. A scan owns cancellation through GUI/runtime/Telegram and a 30-second deadline per request. Cancellation terminates recovery instead of counting as a rejected manifest. Identity checks are bounded to ten seconds. GUI loading state is independent of key operations; stale account/source results are rejected.

Restore authenticates the manifest/File Key, validates locators/ranges, downloads and verifies encoded parts, authenticates each frame, writes one controlled private partial, verifies part/whole plaintext BLAKE3, flushes and atomically publishes without overwrite. Generic integration tests recover with a fresh SQLite database and no caller-supplied File Key/layout, then prove byte equality.

Multi-file upload accepts at most 128 nonempty files. Runtime preflight deduplicates canonical source paths, checks cumulative size and preserves original names; each queued item rechecks size/mtime before encryption. The same retained Vault owner executes one item at a time, revalidating account/channel and key prerequisites for each. Source-specific failures can continue; authorization, permission and network failures stop new work in that batch. A shared cancellation flag stops at file boundaries, so a current file can finish and remaining items become Cancelled. The GUI retains failed/cancelled selections for explicit resubmission. Snapshots expose actual batch membership, queued time and per-file outcomes; they do not invent network capabilities.

Vault task snapshots/controls are memory-only, retaining at most 256 recent members and evicting whole terminal batches so their displayed counts do not silently shrink: there is no mid-file encrypted pause/cancel/retry/priority/restart-resume UI yet. Lock requests do not revoke keys already held by a running transfer; switching accounts waits for Vault completion. Interruption before manifest publication may leave orphan ciphertext. Scan cancellation does not imply transfer cancellation. Empty-file desktop upload, durable encrypted lifecycle, orphan cleanup and complete restart-time AEAD-identity hydration remain open.

## Local availability and disk space

Historical Completed means transfer finalization succeeded; current local presence is a separate observation. Every three seconds, a retained GUI owner asks Runtime for up to three 128-row account-scoped pages, revisiting older history while prioritizing recent native/Vault outputs. Filesystem calls run off the UI thread. The presentation cache holds at most 10,000 recent paths; account changes clear it and discard stale results. Missing means the parent remains accessible; inaccessible parents, permissions and I/O failures are Unavailable. Matching size is Present, not a new cryptographic verification; same-size replacement is not detected.

Missing/changed outputs disable ordinary open/reveal actions. A completed native task can explicitly create a fresh account-validated download with a non-overwriting destination; its old history remains intact. No observation triggers network traffic or file deletion. Vault outputs persist through the schema-10 inventory; its finalization caveat is in [Data model](DATA_MODEL.md#local-output-inventory-version-10).

An independent five-second query resolves the actual Downloads volume and displays its available bytes in the main status bar. Unavailable volumes stay unknown; they are not assigned the system disk's free space. The slower application-directory usage scan cannot suppress this indicator.

## Generic Core state and recovery

| From | Allowed next states |
| --- | --- |
| Queued | Running, Paused, Cancelled |
| Running | Paused, WaitingRetry, Verifying, Failed, Cancelled |
| Paused | Running, Cancelled |
| WaitingRetry | Running, Paused, Failed, Cancelled |
| Verifying | Completed, Failed, Cancelled |
| Failed | Queued (explicit retry), Cancelled |
| Completed / Cancelled | Terminal; a new generic task is required |

Part state is separate so verified parts survive restart. An RPC success is RemoteUploaded, not task completion. Checkpoint transactions reject noncontiguous indices, gaps/overlap, inconsistent totals/progress and a completed task without verified evidence.

Telegram and SQLite cannot commit atomically. After ambiguous remote success, discover deterministic package/part identity, verify exactly one candidate and repair the checkpoint before retrying. Never blindly duplicate an object. Crash injection covers remote success and checkpoint boundaries; final rename versus final database commit still needs dedicated production policy. Persistent source identity, exact lengths/digests, locators and versioned non-secret checkpoint bytes decide safe reuse.

The manifest is published/verified last. Output stays partial until every required authentication/integrity check succeeds. Existing destinations, traversal, permission/disk failures, overlap, wrong keys and corruption must never expose a purportedly complete target. Exact bytes and nonce rules live in [Crypto](CRYPTO_FORMAT.md) and [Manifest](MANIFEST_FORMAT.md).

## Progress, session logs and diagnostics

Upload snapshots expose ephemeral typed activity for storage discovery, per-file destination checks, reading/encryption, the Telegram request queue, streaming upload, message confirmation, readback verification and manifest publication. Phase duration uses a monotonic clock. Telegram's public `upload_stream` API reads through a bounded observer; uploaded presentation bytes include transport-buffered/in-flight ciphertext converted to logical-size equivalents. They are not acknowledgements or verified checkpoint bytes. Readback reports actual received encoded bytes. Existing `transferred_bytes`, confirmed parts and session logs remain verified evidence; file/batch progress uses the transient upload counter and is capped at 99% until finalization succeeds. Manifest upload/readback remains the Publishing phase and cannot inflate file-part progress. Inline observers retain only one numeric snapshot under a short lock, with no event backlog or buffers. The GUI coalesces activity updates at 250 ms and elapsed time at one-second granularity; terminal rows stop activity updates. No stored format or protocol sequencing changes.

Workers produce raw byte/part events; the frontend consumes bounded/coalesced snapshots plus immediate important state/error changes. Details expose timing, queue wait, attempts, verification, failures, current/average rates, part state, memory/queues and Live/Replay decisions. Unknown values stay unavailable. Failures map from structured categories; behavior never parses strings.

Every real transfer attempts to open a private `Logs/Transfers/native-download-<id>.jsonl` or `vault-transfer-<id>.jsonl` before data movement. Schema 1 retains its existing start/part/controller/finish records; one background thread writes complete records from a queue bounded to 256 records and 1 MiB, with a 64 KiB per-record limit. Queue saturation, oversized records and write/open failures increment a cumulative omission count shown in Settings and transfer details. A subsequent successful write adds `log_records_omitted` with an explicit `all_transfers` scope and count. A gap is never a complete replay, and the queued tail is best-effort at process exit. Logging failures do not fail transfers; task persistence and resumable part maps remain independent. Readers branch on schema and tolerate additive fields/events; incompatible meanings need a new version. [ADR 0022](adr/0022-bounded-background-session-logs.md) supersedes the synchronous log behavior in [ADR 0009](adr/0009-transfer-part-map-and-session-log.md). Files retain task-based cleanup; broader disk retention remains future work.

Process diagnostics are a separate lossy facility: daily UTC JSONL rotation, 15 retained files, dedicated nonblocking writer, 4,096-line queue and retained flush guard. Full queues drop events instead of stalling owners; Settings shows the cumulative dropped count and opens Logs. Changing managed root moves process diagnostics after restart. Only `teleark`/`teleark_*` targets are subscribed; dependency events are excluded. Process diagnostic fields are not a stable public format.

Both log families allow only fixed operation/event names, structured classes and numeric identities, offsets, bytes, timings, rates, queue/memory/controller/part counters. Never log paths, filenames, captions, channel titles, file content, sessions, phone/code/password/API Hash, QR tokens or key material. Session-log paths may be displayed locally but are not written into log events. See [Security](SECURITY.md).

## Verification

Deterministic tests exercise states, all scheduler permits, priority/FIFO, retries/FloodWait, cooperative controls, partial artifact validation, source mutation, ambiguity/checkpoint failure, bounded encryption, account isolation, manifest cancellation, fresh-database recovery and final byte equality. Use owned temp files/databases and fake remotes; ordinary CI has no Telegram credentials. Credentialed crash/recovery/throughput tests and million-file measurements remain separate validation work. Versioned checkpoint/log changes include automatic supported-version conversion or retained readers, preservation of resumable state and visible migration activity; see [ADR 0017](adr/0017-versioned-automatic-migrations.md).

Upload completion invalidates any older GUI managed-scan generation before installing the already verified publication receipts. Completion callbacks check login generation, account and storage channel, and never repopulate plaintext metadata while locked. Later explicit scans remain authoritative; fresh history supplementation prevents a lagging search result from immediately replacing newly published files with an empty list. No manifest bytes, checkpoints or persisted state change.

## Upload preflight scheduling

Submission immediately shows preparation feedback. The Vault owner publishes all queue rows and a cancellable batch owner before remote work. Complete uniqueness discovery runs once per batch; every member still refetches target ownership/private metadata and the pinned account/channel identity before accessing plaintext or allocating file keys. Independent candidate inspections run at a maximum fan-out of four within the existing request deadline. Single-file API calls retain complete discovery. SQLite bindings are not an authorization prerequisite. Shared preflight failures mark every queued member; cancellation during validation stops all members when that bounded call returns. Account switching is blocked while submission, queued validation or upload is active. [ADR 0016](adr/0016-bounded-upload-preflight.md) defines the batch-wide uniqueness observation window.

The native download bitmap's in-memory representation is packed and maintains incremental completed-byte/part counters. It still reads and writes `TARKDPM1` version 1 with identical bit positions and canonical bytes; unused high bits retain the original reader behavior. `crates/teleark-telegram/src/fixtures/native-part-map-v1.bin` fixes the existing three-part out-of-order example for compatibility checks. No migration is required for this representation-only change.

Native download sampled checkpoints (2026-09-08): the 750 ms progress sample is submitted without waiting to the bounded Storage actor. Queue saturation may defer that sample, with a sanitized diagnostic; live progress and the native part map remain authoritative for presentation and resumable bytes. Lifecycle/final writes remain acknowledged. Storage only applies samples to the same account, attempt and running state, with nondecreasing bytes and timestamp, so late samples cannot revive retired work. The persistent schema and part-map codec are unchanged.

Owner destruction is nonblocking: shutdown submits the latest native progress sample without replay copies or acknowledged SQLite waits. A full queue retains the previous checkpoint and part map; the transfer owner retains its adapters/state and performs acknowledged retirement when cancellation returns. Storage/Telegram last-sender closure retires their owned work; frontend destructors join only already-finished workers. This does not promise completion of diagnostic writes or an in-progress operating-system call at abrupt process exit.

Native history retention follows [ADR 0023](adr/0023-native-history-and-recovery-budgets.md): new admission shares a 10,000-resident-record budget, evicts only completed/cancelled resident groups, and protects recoverable/scheduled batches. Eviction preserves database rows, logs and user files; redownload can load cold history. Startup restores every recoverable task and its batch members even if a legacy backlog exceeds the budget, then fills remaining slots with recent terminal history. The list discloses omitted terminal records. Full replay remains for bounded executing/scheduled work and 16 recent idle tasks; older idle tasks keep 20 part events/32 decisions with explicit gaps. Lifecycle history keeps the latest 256 events and an omission count. Persistent schema/bitmap versions are unchanged.
