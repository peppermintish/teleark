# Transfers and diagnostics

## Current payload and presentation contract (2026-09-15)

[ADR 0041](adr/0041-memory-streaming-and-portable-upload-recovery.md) supersedes
older ciphertext spool, 60 MiB target and container-completion speed descriptions.
The new ceiling is **1.9 GiB of encrypted bytes**, including headers and tags.
Upload streams plaintext → in-memory encryption → bounded 512 KiB blocks → RPCs;
download streams RPC blocks → in-memory frame authentication/decryption →
plaintext partial output. Container and final-file checks precede publication.
Legacy ciphertext spools are recovery inputs only; tiny recovery metadata remains
persistent. Whole-file and container BLAKE3 share one sequential inspection pass.

[Pending uploads](PENDING_UPLOAD_FORMAT.md) are published before source inspection
and updated with verified published-container prefixes. Same-source/device handoff
requires the original recovery key and account/channel scope. Unpublished lost
memory is retired before fresh encryption. RPC confirmations update hot counters
immediately, including native download chunk replies. Ordinary display samples
publish once per second over three seconds (50 ms edge quantization), and silence
expires the rate. Restored/duplicate/stale confirmations cannot create speed.
Checkpoint persistence runs alongside transport and never resets progress.
Global totals are incremental; list filters/identities and action scopes are
cached, visible rows resolve changed records, and charts/maps have fixed bounds.

Three implementations must remain distinct: the deterministic Core transfer engine, the connected durable native-download owner, and the connected Vault owner. The first provides tested ports/policy; the latter two define currently exposed desktop capabilities. [Status](IMPLEMENTATION_STATUS.md) records remaining integration work.

## Ownership and ports

A task is one logical file; a transfer part is one application part, not an MTProto unit. Core uses project-owned transport, filesystem, checkpoint, clock/jitter and progress ports. Storage/native-file adapters are thread confined; Telegram requests use a retained bounded owner. No GUI types or localized prose cross these boundaries, and snapshot locks are not held during network operations.

The cooperative generic engine advances through bounded steps. Its scheduler requires global, direction, account and file permits together, uses priority/FIFO fairness, obeys account-scoped FloodWait and persists verified evidence at safe pause/cancel/restart boundaries. It never spawns one task per queued item.

## Connected native downloads

Managed destination allocation checks both the filesystem and the indexed native-history destination column, across all task states and accounts. A deleted output does not free its historical path; a fresh download receives a numbered path and a new task while preserving prior history. Allocation is bounded to 10,000 candidates and fails closed on storage errors. Managed allocation atomically creates an empty `<destination>.partial` reservation shared by native and encrypted owners; insertion constraints and final no-overwrite publication also reject concurrent collisions.

Requests carry positive account/chat/message identity and a runtime-allocated non-overwriting destination under managed Downloads. The adapter validates the actual account and refetches source identity before transport. Schema 9 records account scope; [Data model](DATA_MODEL.md) specifies one-time legacy attribution. Unknown/other-account work cannot enqueue, resume or retry. Switching pauses eligible work, waits up to 30 seconds for retained workers to release requests/partials, then signs out; failure leaves a visible error rather than switching under active work.

Native states are Queued, Running, Paused, Completed, Failed and Cancelled. Resume requests missing ranges; retry accepts Failed and Cancelled while retaining task/source/destination identity. Cancelled retries clear cancelled progress and wait for the previous worker to retire before starting. This lifecycle is separate from the generic Core transition table below.

The adapter fills 1 MiB logical parts using at most 128 KiB Telegram requests and writes out of order at explicit offsets. A private sibling partial and `TARKDPM1` bitmap record completed ranges. The exact format remains [ADR 0009](adr/0009-transfer-part-map-and-session-log.md): big-endian magic/header, fixed logical part size, total length, part count, bitmap length and valid trailing bits. Reject malformed/inconsistent/trailing data; never infer completion from a sparse file's length. Each request has a 60-second reply deadline. A timed-out transfer connection slot is skipped for the rest of that download; retries start with another slot and retain the same part/bitmap identity. The per-part retry cap and shared Telegram rate-limit deadline remain in force. [ADR 0048](adr/0048-native-download-connection-failover-and-diagnostics.md) records the failure and decision.

Pause/interruption and retryable failures retain only a valid partial/bitmap pair with completed data. Zero-progress, corrupt, incomplete, nonretryable and cancelled artifacts are removed. Completion requires every bit, exact expected byte length, flush/sync and atomic no-replace publication. Native verification is byte-count/finalization safety, not cryptographic content authentication.

Successful native publication also releases its empty `<destination>.partial`
reservation before reporting Completed. The native transport's hidden partial and
bitmap remain separate from this admission marker. Cleanup checks for a regular,
zero-length marker and a regular final output matching the task's expected size;
nonempty encrypted partials, symlinks, missing/changed outputs and recoverable
native work stay intact. Deletion synchronizes the parent directory. Cleanup errors
are logged without invalidating the completed user file. At startup, a retained
reservation-cleanup worker retries markers from up to 10,000 restored completed
records, independently of startup, SQL, new downloads and explicit cancellation
cleanup; it does not scan user directories, redownload files or run a recurring
sweep. Omitted historical records are outside this maintenance pass. A cancel
arriving during completion cleanup keeps its cancelled outcome instead of being
overwritten by a late Completed callback. Persistent schemas and partial codecs
are unchanged.

History persists task/batch identity, original message metadata, progress/timing, attempts, verification and structured failure. A batch inserts its header and all children atomically. Startup normalizes interrupted Running rows to Queued, then schedules eligible account work only after account entry/source refresh. Terminal task deletion requires confirmation and removes owned history/log/partial/bitmap artifacts, never a completed user file. Bulk commands have a retained cancellable owner and surface partial failures.

Shutdown persists the latest observed progress and signals cancellation without waiting indefinitely for another network chunk. A blocked worker may detach for prompt process exit; the private part map remains restart authority.

## Manual concurrency and retries

Production owners use `TransferTuning` and a fixed-parameter telemetry controller.
No Balanced/Max Throughput profile, throughput probe, BDP rule or memory-pressure
heuristic changes user-selected values. Legacy adaptive algorithms remain only
for compatibility/test coverage; ADR0036 supersedes their production policy.
Defaults and bounds are in [Preferences](PREFERENCES_FORMAT.md).

Uploads run three files concurrently, each with ten asynchronous 512 KiB
`upload.saveBigFilePart` requests. Independent retained MTProto sender pools share
the authenticated session and mandatory network gateway, with two connections
per direction by default; cloning a Client alone would not create connections.
RPCs are assigned round-robin across the selected direction's pools. Connections
are shared across tasks rather than multiplied by each file. No encryption worker,
checkpoint write or thread join runs on the UI or network reactor.

One producer per file reads/encrypts a frame, writes the immutable ciphertext to
its bounded spool and hands the reusable buffer to a queue of two ready blocks.
The consumer owns at most the configured concurrent part requests and returns
buffers for reuse. Backpressure bounds production; no whole plaintext container
is retained. Internal result windows also retain at most 128 recent completed
manifests / 16 MiB. All hard maxima bound the session even with the largest manual
settings; setting high limits can deliberately consume more memory and is not a
promise of higher throughput. MTProto owns additional bounded in-flight wire copies.

Native and Vault downloads share the user-selected file gate. Native workers and
Vault download owners have retained bounded pools separate from upload execution.
Both native and bounded encrypted-object reads use the selected part, connection
and attempt limits. Native logical parts remain 1 MiB, split into protocol reads;
this preserves their existing download bitmap codec. Vault downloads authenticate
complete application containers as before.

Part attempts use structured errors, 60-second request timeouts and bounded
exponential retry backoff. Direction-wide FloodWait gates honor the entire server
deadline and never shorten it; healthy in-flight requests may finish. There is no
concurrency reduction or increased limit in response to a retry. A failed upload
part cancels that stream's retained RPC tasks; independent files remain runnable.
Byte rate caps continue to apply across tasks through the shared budgets.

The upload inspector prioritizes acknowledged-byte rate intervals, a 512 KiB
part map, queued/active work, last activity, retry deadlines and a bounded timeline.
Up to 128 events and 96 rate samples remain in memory, with truncation counts;
samples coalesce to at most four per second plus container boundaries. Restored
acknowledgements are excluded from new throughput. Logs use the existing bounded
background writer and disclose dropped records; logging cannot block transport.
100% still requires message/manifest publication and local persistence.

## Connected encrypted storage

New uploads validate the account's bound, currently owned private TeleArk channel before touching plaintext/keys. Saved Messages is available only as an explicit legacy recovery source. Retained operation leases carry the required key to independent background owners; key operations remain serialized.

Upload first acknowledges preparation and hashes source bytes with bounded
buffers. A streaming encryption pass checks each container digest against that
admission, overlaps encryption with part RPCs and checks source identity again
before manifest publication. Normal content uploads are accepted from local sealed
hashes plus Telegram's publication receipt, with **no content read-back**. Only
ambiguous publication recovery or legacy receipt reconstruction reads a bounded
remote candidate. The small authoritative manifest keeps its publication
verification. See [recovery records](VAULT_TRANSFER_RECOVERY.md) for the 24-hour
window, immutable ciphertext replay and safe retirement.

Managed scanning considers at most 1,000 exact-caption candidates. Manifest discovery merges indexed exact-caption matches with exact matches from the latest 512 messages read directly from Telegram history. Candidates are deduplicated by message ID, recent observations win, and the newest 1,000 candidates remain within the existing bound. This avoids waiting for search visibility after publication; ordinary part reconciliation does not add a history scan. Both sources still require the same manifest download, AEAD authentication, name and strict layout validation. Names/captions classify candidates only; authentication and strict layout validation establish a logical file. A scan owns cancellation through GUI/runtime/Telegram and a 30-second deadline per request. Cancellation terminates recovery instead of counting as a rejected manifest. Identity checks are bounded to ten seconds. GUI loading state is independent of key operations; stale account/source results are rejected.

Restore authenticates the manifest/File Key, validates locators/ranges, downloads and verifies encoded parts, authenticates each frame, writes one controlled private partial, verifies part/whole plaintext BLAKE3, flushes and atomically publishes without overwrite. Generic integration tests recover with a fresh SQLite database and no caller-supplied File Key/layout, then prove byte equality.

Multi-file upload accepts any number of nonempty files, without a fixed count cap. The retained owner schedules 128-file internal windows; the window size is not a user limit. Selection-wide incremental counters and a five-phase timeline cover pending files that are not yet materialized as transfer rows. Detailed history retention is disclosed. Runtime preflight deduplicates canonical source paths, checks cumulative size and preserves original names; each queued item rechecks size/mtime before encryption. A work-conserving pool executes the selected number of items concurrently, revalidating account/channel and key prerequisites for each. Source-specific failures can continue; authorization, permission and network failures stop new work in that batch. One shared cancellation flag spans all internal windows and stops at file boundaries, so a current file can finish and all remaining selected items become Cancelled. The GUI retains failed/cancelled selections for explicit resubmission. Snapshots expose actual internal batch membership, queued time and per-file outcomes; they do not invent network capabilities. `VaultUploadReport.completed_count` reports every success; `completed` contains only recent receipts (128 files / 16 MiB), backed by the durable authenticated catalog. Failure/cancellation paths remain available for retry. See ADR 0028.

Vault snapshots retain bounded recent history while durable jobs, controls,
part reservations/receipts and manifest outboxes support pause/cancel/retry and
restart recovery. See the recovery contract for exact supported states. Locking
does not revoke admitted operation keys. Empty desktop uploads, orphan cleanup,
and credentialed multi-DC/throughput qualification remain separate work.


## Local availability and disk space

Historical Completed means transfer finalization succeeded; current local presence is a separate observation. Every three seconds, a retained GUI owner asks Runtime for up to three 128-row account-scoped pages, revisiting older history while prioritizing recent native/Vault outputs. Filesystem calls run off the UI thread. The presentation cache holds at most 10,000 recent paths; account changes clear it and discard stale results. Missing means the parent remains accessible; inaccessible parents, permissions and I/O failures are Unavailable. Matching size is Present, not a new cryptographic verification; same-size replacement is not detected.

Missing/changed outputs disable ordinary open/reveal actions. A completed native task can explicitly create a fresh account-validated download with a non-overwriting destination; its old history remains intact. No observation triggers network traffic or file deletion. Vault outputs persist through the schema-10 inventory; its finalization caveat is in [Data model](DATA_MODEL.md#local-output-inventory-version-10).

An event-driven background query resolves the actual Downloads volume and displays its available bytes in the main status bar, triggering on startup, transfer progress and completion, preference changes, and user interaction without idle polling loops. In-flight queries coalesce so repeated events never spawn redundant filesystem operations. Unavailable volumes stay unknown; they are not assigned the system disk's free space. The slower application-directory usage scan cannot suppress this indicator.

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

Workers produce raw byte/part events; the frontend consumes bounded/coalesced snapshots plus immediate important state/error changes. Details expose timing, queue wait, attempts, verification, failures, current/average rates, part state, memory/queues and Live/Replay decisions. A failed native part records its structured cause, zero-based local transfer connection slot, attempt and elapsed time; the inspector displays the one-based slot and cause above the trace. The slot is a local assignment, not a measured Telegram DC or network lane. Unknown values stay unavailable. Failures map from structured categories; behavior never parses strings.

Every real transfer attempts to open a private `Logs/Transfers/native-download-<id>.jsonl` or `vault-transfer-<id>.jsonl` before data movement. Schema 1 retains its existing start/part/controller/finish records. Native `part_state` records now write schema 2 with `connection_slot` (zero-based) and a nullable `failure` code (`timeout`, `network`, `server`, `rate_limited`, `authorization`, `unexpected_response`, `other`). Old schema-1 records keep their meaning and remain readable: their terminal part, attempt and elapsed time can be restored after restart, while cause and slot remain explicitly unknown. Schema-2 terminal part causes and slots can also be restored. The restoration reads at most the final 64 KiB and stops at the latest session boundary. The SQLite schema and `TARKDPM1` bitmap codec are unchanged. One background thread writes complete records from a queue bounded to 256 records and 1 MiB, with a 64 KiB per-record limit. Queue saturation, oversized records and write/open failures increment a cumulative omission count shown in Settings and transfer details. A subsequent successful write adds `log_records_omitted` with an explicit `all_transfers` scope and count. A gap is never a complete replay, and the queued tail is best-effort at process exit. Logging failures do not fail transfers; task persistence and resumable part maps remain independent. Readers branch on schema and tolerate additive fields/events; incompatible meanings need a new version. [ADR 0022](adr/0022-bounded-background-session-logs.md) supersedes the synchronous log behavior in [ADR 0009](adr/0009-transfer-part-map-and-session-log.md). Files retain task-based cleanup; broader disk retention remains future work.

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

## Batch presentation

Single-file transfers, including one-member batches, occupy one 42-point row. Batch headers occupy 63 points and show completed/total and failed counts. Collapsed headers use the ordinary row surface with a semibold title. Activating a header with 2–9 members toggles one continuous inline group: the darker header, lighter member surface, neutral outline and left marker connect the title to its files. Child checkboxes and names are indented; members have no individual boxes. Rounded group ends and outside spacing separate adjacent batches and ordinary tasks. Hover/selection tint each row's existing surface, preserving the hierarchy and marker. Row content retains its normal height; only group boundaries add spacing. Search/state filters can still reveal matching members without expanding unrelated work, and boundaries follow adjacent visible batch identities in constant time per rendered row.

Batches with more than nine members open a centered auxiliary window initially smaller than the parent. Its fixed title and scrolling members share the same group surfaces, outline and continuous left marker. It uses the same revisioned transfer projection and command owner, virtualizes member rows, and follows parent notifications without another service loop. The window supports member inspectors and action feedback/confirmation, can be reused after repeated activation, and closes on account change, parent-window close or parent-owner release. Member rendering holds a weak owner reference. Closing the auxiliary window does not cancel transfers. Opening is deferred past the parent event lease because the framework draws the child synchronously. No transfer lifecycle or persistent format changes.


## Durable Vault upload history

[ADR 0029](adr/0029-durable-upload-history.md) supersedes the memory-only history behavior. Schema 16 persists admitted upload windows, file starts and terminal summaries. Database acknowledgment precedes completion in the UI. Startup converts unfinished local operations to explicit interrupted history without resending them; the inspector explains how to check Storage and start a new upload from its source. Account-scoped restoration runs before remote catalog loading, with visible background phases and generation checks. The current view retains 256 recent rows plus the complete boundary batch and discloses older stored rows. Restored totals and available sanitized logs are real; live telemetry is explicitly unavailable. Older versions did not save task summaries, so already-lost rows cannot be reconstructed. Encrypted part resumption and browsing older upload history remain open.

## Aggregate upload and download speed limits

[ADR 0030](adr/0030-shared-transfer-speed-limits.md) defines independently adjustable,
application-wide payload budgets for upload and download. Transfers and both
transfer Settings sections open the same editor (integer KiB/s; zero is unlimited).
Successful saves immediately update existing connection clones and survive route
replacement and restart. Failed saves retain the active limits.

Native downloads reserve a chunk before issuing its request; encrypted downloads,
verification reads and manifest payloads use the same download budget. Uploads pace
bounded source reads before handing bytes to the transport. Authentication, proxy
checks, catalog control requests and avatars do not consume file-transfer budgets.
This is payload admission pacing, not an exact socket-byte ceiling: MTProto overhead,
internal transport retransmission and previously admitted/in-flight buffers are not
counted again. The token burst is at most 512 KiB per direction; each upload reader
admits at most 16 KiB per read and grammers assembles 512 KiB transport parts, so low
upload caps can have a long visible wait before each part is sent. Existing transport
concurrency bounds its in-flight buffers. Download requests use a 4–512 KiB aligned
chunk chosen at logical-part start; policy changes wake waits immediately without
changing the existing persistent logical-part map.

A global status strip remains visible across navigation and collapsed batches while
caps are configured. The editor shows waiting counts/durations, last payload
admission and an expandable timeline (latest 16 transitions per direction, with
omission counts). A retained 250 ms presentation sampler reads bounded in-memory
snapshots, never SQL or queues; unchanged idle frames are not invalidated. Existing
transfer cancel controls cancel pending admission through task cancellation. Limits
can be changed to zero to release bandwidth waits; waiting itself is not completion.

Synthetic English-only review: `--preview-ui --preview-state=speed-limits --locale=en-US` opens the editor with bounded fixture history and wait feedback. It does not run transfers or save settings.
