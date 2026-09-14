# Encrypted transfer recovery records

Status: DesktopVault uploads and downloads persist recovery context, support mid-file pause/cancel/retry, and dispatch eligible work after restart and unlock. Native cancellation retains a durable cleanup obligation. Legacy summaries without recovery context remain viewable but cannot be resumed. The evidence and limits below describe synthetic transport tests and native macOS UI review; no live Telegram or other-platform qualification is implied.

## Context version 1

Stored in schema-17 `vault_transfer_jobs.context`, with `context_version=1`. Integers are little-endian. Exact length is required; trailing bytes are rejected. The final 32 bytes are BLAKE3 of all preceding bytes, for accidental corruption detection, not a substitute for AEAD authentication. Raw master/file keys and Telegram sessions are never serialized.

| Field | Bytes / encoding |
| --- | --- |
| Magic | 8 ASCII bytes `TARKVR01` |
| Context version | u32, exactly 1 |
| Direction | u8: upload=1, download=2 |
| Account ID, task ID, chat ID | i64, u64, i64; positive SQLite-compatible IDs |
| Package ID, Vault ID | 16 bytes each, nonzero |
| Master-key generation | u32, positive |
| File-key wrap | algorithm u16, wrap generation u32, ciphertext32, tag16; existing crypto suite and positive generation |
| Creation timestamp, logical size | u64 each, SQLite-compatible; size positive |
| Logical filename | u32 UTF-8 byte length + bytes; 1–4096 bytes, no NUL |
| Native path | platform u8 + u32 byte length + bytes; 1–65536 bytes |
| Direction-specific fields | below |
| Checksum | BLAKE3-256 of everything above |

The fixed header before the filename length is 143 bytes. Unix paths use tag1 and native bytes, preserving non-UTF-8 names. Windows paths use tag2 and UTF-16LE units, preserving native names without lossy Unicode conversion. Paths must be absolute and cannot contain NUL. Foreign platform tags are rejected intact; no inferred path substitution occurs.

Upload fields after the source path: filesystem identity u128, source size u64, modification units u64, revision u64, full-source BLAKE3-256. Source size must equal the job size. Before reusing an encryption identity, the runtime must verify the actual source bytes against the protected digest; unchanged metadata alone is insufficient.

Vault source screening compares the canonical path, filesystem identity, size and
content modification time. The stored revision remains Unix ctime (metadata-change
time), which can change after permission or extended-attribute updates without a
content write. A revision change alone therefore does not reject admission,
preparation, finalization or resume. Full-source hashing, immutable per-part digests,
and remote ciphertext/AEAD verification remain required. Resume checks the saved
whole digest before restoring encryption work; even same-size content changes with
a restored modification time are rejected before nonce reuse. Both context and
pending-source codecs remain read/write v1, including the original revision bytes;
existing saved contexts and reservations are retained without conversion. See
[ADR 0035](adr/0035-vault-source-metadata-validation.md).

Download fields after the destination path: positive manifest message ID i64, manifest-envelope BLAKE3-256, complete plaintext BLAKE3-256. The runtime must authenticate the exact manifest, validate its package/part layout and key scope, and revalidate any retained local extents before skipping work. The persisted absolute destination must be honored with non-overwriting publication.

`VaultRecoveryContext::from_record` compares account, task, chat, package, direction and creation timestamp against the separate immutable database columns before exposing the decoded context. File-key unwrapping authenticates the existing package/vault/generation binding. A checksum is not a key or an authorization check. UI and execution owners still need their own expected account/generation guards.

The frozen Unix upload fixture is `crates/teleark-runtime/src/vault_recovery/fixtures/upload-context-v1.bin`; all values, paths and wrapped-key bytes are synthetic. Compatibility tests require byte-for-byte encoding and successful decoding. Unknown versions, corrupted checksums, invalid lengths and foreign scopes fail without modifying the stored record.

## Part reservation version 1

Stored as immutable `vault_transfer_parts.identity`, exactly 180 bytes:

| Field | Bytes / encoding |
| --- | --- |
| Magic | 8 ASCII bytes `TARKVP01` |
| Version | u32 little-endian, exactly 1 |
| Part header | existing 96-byte version-1 crypto header, retaining its big-endian encoding |
| Plaintext digest | BLAKE3-256, 32 bytes |
| Publication random ID | nonzero i64 little-endian |
| Checksum | BLAKE3-256 of the first148 bytes |

Header decoding applies the crypto crate's canonical limits, algorithms, layout and reserved-field checks. The account/task scope comes from the parent ledger record. Before execution, the runtime must additionally compare the header's package/layout against its admitted context. Part instance/nonce identity and publication ID are reserved before encryption or remote publication; a retry cannot replace the stored reservation. Ciphertext/cache durability, exact-byte validation and remote publication reconciliation must be integrated before these records support production resume. A cancelled send can have an unknown remote outcome and must not be blindly resent.

## Reserved publication transport integration

The transport exposes `reserve_part_identity` and `resume_reserved_part`. The execution owner must commit the reservation before calling resume. Resume validates the complete part layout and plaintext digest before encrypting with the original identity, then reconciles exact ciphertext among bounded caption candidates. A saturated candidate result fails closed. Missing candidates use `ReservedPublicationStore`, which requires an explicit nonzero publication ID and has no ordinary-upload fallback. Verified receipts require matching encoded size, ciphertext digest, authenticated plaintext and layout. A restart can reconstruct the same bytes without retaining a live encryption object.

Telegram's [sendMedia contract](https://core.telegram.org/method/messages.sendMedia) defines `random_id` for deduplication and documents `RANDOM_ID_DUPLICATE` as a server error without a receipt. Runtime retains that response as ambiguous and must reconcile it; it is not successful completion. Do not assume an undocumented deduplication retention window. The exact-byte reconciliation remains required.

Deterministic transport tests cover lost receipts followed by reconstruction, delayed search with stable-ID replay, source/layout changes rejected before remote work and corrupt remote bytes rejected before manifest recording. These are transport tests, not proof of DesktopVault startup or persistent owner integration. Ciphertext caching and full cancellation coverage remain to be qualified; the ledger-backed part executor is described below.

## Upload part receipt version 1

`DurableUploadParts` opens the authenticated context for a running lease, validates each part's length, commits or rechecks its immutable reservation with the current generation, performs remote reconciliation/verification and commits the receipt. Existing receipts address the remote message directly, including while search is lagging. Database writes occur outside network operations so a separate control connection can persist pause/cancel intent. Successful in-flight work is receipted while pausing/cancelling; acknowledged or superseded owners cannot commit.

Upload receipts stored in `vault_transfer_parts.receipt` are exactly84 bytes: ASCII `TARKUR01` (8), little-endian u32 version1 (4), positive i64-compatible message ID encoded as little-endian u64 (8), BLAKE3 of the immutable reservation bytes (32), BLAKE3 of the preceding52 bytes (32). The digest binds the receipt to its reservation; it does not replace remote cryptographic verification. Unknown versions, trailing bytes, invalid message IDs and corrupt/foreign identity digests fail intact. The synthetic frozen fixture is `crates/teleark-runtime/src/vault_recovery/fixtures/upload-receipt-v1.bin`.

SQLite close/reopen integration tests cover durable reservation visibility before remote send, lost acknowledgments recovered under a new generation, unchanged reservation bytes, direct verification of saved receipts, stale owners stopped before remote work, pause-success receipt races and rejection after cancel acknowledgment. New DesktopVault uploads now use this executor through the existing bounded parallel encryption pipeline. Manifest outbox recovery is described below; download owners and UI acceptance remain required; queued restart dispatch is described below.

## Desktop upload path

The connected upload path now scans the source with a1MiB buffer to establish full-file and per-part digests, checks its filesystem identity again, and durably admits the wrapped key/package/source context before reserving encryption jobs. Source verification publishes byte counts in a distinct phase and never increases uploaded-byte progress. Saving recovery information has its own localized phase. Each encryption job is an opaque Send value with no database/transport owner; its result retains the task generation and context digest. The sender rechecks the lease after queueing and only commits verified receipts. Existing bounded crypto workers and queues remain in use.

Source bytes are checked against the admitted per-part digest before encryption and the full digest/identity again before manifest publication. The final ledger state is written after manifest verification and local inventory/health persistence. A ledger write failure still runs visible task failure/session-log finalization. Queued restart dispatch is now connected after unlock. Pending-batch admission, download recovery and independent controls are described in their sections below. Real Telegram transport is outside the synthetic acceptance evidence.

## Queued restart dispatch and explicit resume entry

Before DesktopVault creates any execution owners, Storage recovers interrupted ledger jobs across all retained accounts in one bounded-memory SQL operation: running becomes queued, pausing becomes paused and cancelling becomes cancelled, with a new execution generation. Unknown context versions remain untouched. Legacy history still has no fabricated recovery keys or paths.

After a successful active-key unlock, or history restoration when already unlocked, the desktop retains a background recovery job. It scans queued upload IDs in pages of32, checks the active Telegram account, skips paused/cancelled/failed jobs and continues after individual failures. Preview mode never schedules this work. Login/session generations guard UI callbacks. No user-control interface is delivered merely by this dispatch path.

`submit_resume_upload` accepts an explicit paused/retryable/queued upload and rejects other states. It verifies the wrapped key and source's canonical path, filesystem identity, size and whole digest, then reuses the original package, task, key and context generation. Manifest creation time, key generations and vault identity also come from the original context. An early validation failure is persisted and made visible instead of leaving the task queued indefinitely. Resumed activity is live, rather than replayed historical telemetry. Missing source/access/key/capacity conditions remain retryable; changed content and invalid contexts are blocked.

Synthetic composition tests exercise the real cold-start constructor, multiple queued failures without starvation, preservation of paused jobs, account rejection and historical-key admission gates. The later service-composition tests additionally cover successful uploads/downloads, blocked transport and restart; the original tests alone establish only the listed cold-start and key gates.

## Manifest outbox version 1

Schema 18 adds a dedicated outbox per upload job. Its pre-encryption commitment is BLAKE3 of the byte string `TARK manifest commitment v1` followed by a zero byte, the existing canonical manifest envelope prefix, canonical public-header bytes and canonical plaintext metadata bytes, in that order. Prefix lengths delimit both payloads. This commitment does not change the manifest format or provide remote authentication. Plaintext serialization is zeroized after hashing. The exact field values and lengths use the existing manifest-v1 codec.

Actual DesktopVault uploads now commit this commitment and a stable nonzero signed publication ID before sealing. They reuse the previously wrapped file key, save the complete encrypted envelope before sending, and authenticate/compare it before remote work. A changed name, timestamp, locator, digest or other metadata cannot reuse the generation's reserved encryption input. Restart uses the same encrypted envelope and publication ID. Known message receipts are read directly; otherwise bounded caption search must find an exact match, or the same sending ID is retried. Truncated/saturated discovery and conflicting content fail rather than authorize a new identity.

Only exact readback commits the remote message ID. Local inventory persistence precedes terminal job completion; ordinary uploads also persist their just-verified part health. Lost-ack tests close and reopen SQLite, hide search results, rebuild verified part state and prove one remote manifest, stable reservation/envelope, unchanged-metadata enforcement and detection of corrupted remote bytes. This is synthetic protocol evidence, not a guarantee about Telegram's undocumented deduplication retention period.

Actual upload resume now checks for a persisted complete manifest envelope before entering source metadata/hash preflight. If present, a dedicated retained-owner path validates the storage target, claims the running generation, registers cancellation, authenticates the envelope and reconstructs its original publication request without reading the source. The shared publisher verifies context, commitment, exact bytes and receipt, then local inventory is persisted before terminal acknowledgment. Publishing/persisting phases and a bounded session log remain visible. No full-file upload rate is invented for this recovery pass. Part health remains unchecked because this path verifies the manifest receipt, not every remote part again. Without a complete saved envelope, the normal source-validation path remains required; malformed saved data fails visibly rather than silently creating a new upload.

The lost-ack restart test now reopens an empty part worker and completes the saved publication without supplying any plaintext; the same test retains changed-input rejection and corrupted-remote detection. A missing outbox returns no recovery candidate without uploading. Service-composition tests also exercise source-free publication through DesktopVault. Account replacement interrupts retained operations. Abrupt-process tests independently verify persisted download extents and already-published output before the final job checkpoint; this is not a hardware power-loss test.

## Download extents

Actual encrypted downloads now admit a version-1 Download context before preparing the output. It binds the authenticated manifest envelope hash/message, wrapped key, whole plaintext hash and chosen absolute destination. Each authenticated part uses an immutable ledger extent reservation before fetching, then plaintext length/hash verification, partial-file write and `sync_all` before a receipt. Failures preserve the partial output. Destination allocation skips retained `.partial` files to avoid replacing failed-download data with a new task.

The extent identity is exactly 152 bytes: `TARKDE01` (8), codec version LE u32 (4), BLAKE3 of the encoded recovery context (32), part index LE u32 (4), plaintext offset and length LE u64 (8 each), plaintext BLAKE3 (32), encoded ciphertext BLAKE3 (32), remote message ID LE i64 (8), and part instance ID (16). The receipt is exactly 40 bytes: `TARKDR01` (8) followed by BLAKE3 of this identity (32). Its meaning is that the expected plaintext extent was verified, written and fsynced; it never authorizes skipping local verification after restart. The context digest also binds this receipt to the original authenticated manifest and destination.

The download executor can reopen the same ledger/partial file under a new generation. It hashes every receipted extent before reuse and re-fetches corrupt or missing contents. It rejects unverified remote bytes, and a stop arriving during fetch prevents a new write. Whole-file verification and flush precede publication. If a previous attempt already published the final file but did not finish inventory persistence, reopening verifies its size and full hash, rechecks reused ranges, and permits idempotent finalization without another remote download. Conflicting existing output is preserved and rejected. Restart recovery rejects symbolic links and other non-regular final entries, even when the link target contains the expected bytes. A retained read handle supplies both full-file verification and reused ranges; the final path is checked before and after reads and again at completion. Unix checks bind that path to the same device/inode, so replacing it with an identical-byte file cannot earn completion. Other platforms currently compare available file metadata and still require stable-identity parity qualification. A pathname substitution during the initial open remains a separate hardening requirement; these checks do not make blocking filesystem reads cancellable.

This executor is connected to new and explicitly resumed DesktopVault downloads. `submit_resume_download` accepts only paused, retryable or queued download records; it reuses the original task, package, manifest message/hash, wrapped key and destination. It validates package/context scope before queue mutation, fetches the saved manifest directly, verifies the envelope hash and authenticated content, and fences the new execution generation. A preflight failure becomes a durable retryable/blocked result without discarding existing output. Startup download projection and mixed queued dispatch are now connected as described below. Part-level storage tests do not prove successful end-to-end interruption. Crypto frame-boundary cancellation is described below. Final publication uses a sibling hard link to atomically reject existing destination entries, including dangling symlinks. The parent directory is synchronized before removing the partial name; cleanup failure preserves a recoverable extra name rather than invalidating the verified output. Filesystems that reject hard links fail without a rename fallback. Deterministic tests cover a destination created after preparation and a dangling destination link. Exclusive destination admission across native/encrypted workers, partial-file access/identity hardening, stop/finalize races, and live restart qualification remain required.

## Locked history and mixed startup recovery

History restoration reads a bounded account-scoped download page without opening local output or requiring keys. Schema 19 indexes this page independently of unrelated uploads. It merges up to 256 historical rows across upload/download, prioritizes active states and discloses omitted history. Existing live observations for the same account win against a delayed history read; equal task IDs in another account cannot reuse a cached row.

Readable saved downloads expose their original name, destination and durable state. Pending saved bytes/part geometry are unknown until local verification, so the UI shows unavailable progress values and a verification explanation rather than reporting a new 0% download. Completed ledger entries retain completed file-size progress. A damaged or unsupported context remains visible as an unavailable saved download with an explanation; the original record/files remain intact and no resume capability is fabricated. Upload history also validates its recovery context before advertising controls.

After unlock/history readiness the retained GUI recovery job submits a mixed upload/download scan. It scans queued IDs in bounded pages, fences account revision, and continues after individual failures. Active keys permit uploads; historical-only sessions skip uploads while allowing downloads. Paused, cancelled, retryable and blocked jobs are never implicitly restarted. Failures from explicit stop requests do not inflate the failed-recovery count. The upload-only API remains available with its active-key admission policy.

Recovery dispatch walks queued records in pages of 32 on its retained owner, rather than enqueueing every restored task into a fixed-capacity channel. History reads and rendering remain bounded; schema 22 prioritizes active/recoverable work before the 256-row visible cap and exposes omitted counts. Deeper direction/id pages are available in Storage; the GUI does not currently provide an unlimited-history browser. Guidance and English/light compact/full-screen review are described below.

## Bounded crypto cancellation and execution visibility

`encrypt_part_cancellable` and `decrypt_part_cancellable` accept a cancellation probe and preserve the existing byte formats. They check before initial work, before each bounded frame, after reads and before output writes. Cancellation returns a structured `CryptoError::Cancelled`, never a successful integrity summary. The original non-cancellable entry points remain wrappers. A frame already inside a synchronous read/write or AEAD operation finishes before the next check; this is not preemptive cancellation of a kernel call.

The actual encrypted upload pipeline copies the task token into each encryption context, and encrypted downloads attach it to decoding. Runtime maps crypto cancellation to the transfer cancellation state. Durable nonce/part reservations remain unchanged when an encryption worker stops; resume validates the original plaintext commitment and can use the same reserved identity. Tests stop encryption/decryption after the first frame, verify that the second frame is not read, and resume a cancelled reserved upload without an earlier remote publication.

Restored history placeholders, including queued metadata, can be evicted from the bounded projection to make room for an owned executing task; active owned tasks cannot. Single-task and upload-window admission check projection acceptance and fail with Capacity rather than starting invisible work. A rejected download projection also settles its claimed ledger state. Omitted history remains disclosed. Complete recency/pagination and retention-count qualification remain outstanding.

## Independent encrypted-transfer controls

Per-task upload and download pause/cancel requests use `submit_transfer_control` and a retained, bounded control owner independent of the transfer and key owners. The upload-named entry remains available as an alias. They persist intent before signalling the matching execution generation. Locking the vault does not prevent these controls. Active transport requests receive cancellation and the bounded pipeline checks cancellation before reading/encrypting work. The execution owner acknowledges the durable stop after its workers return. If no matching publication owner remains, controls acknowledge pending pause/cancel without treating an orphan running record as successful. Old generations cannot signal newer owners.

The transfer list exposes upload and download pause/resume/retry/cancel from the projected durable state. Download resume permits either active or historical keys; upload resume requires the active key. A queued download resume owns a cancellation token during manifest preflight, then claims its running generation before output work. Legacy summaries without a recovery record receive no fabricated resume capability. Control requests and long-running resume requests have separate retained UI jobs so a pending resume does not disable pause. Paused files have a separate upload-selection count and are not reported as cancelled or successfully uploaded. An explicitly stopped download does not produce a global failure banner. Download progress includes verified local reuse, while speed statistics count newly received data only, including at finalization. A preflight failure during resume refreshes the projected recovery state from Storage.

Deterministic tests use the actual independent control queue with blocked upload/download jobs and a locked vault. Public download-resume tests also verify repeated preflight failure retains context/partial data, account isolation and locked admission; these tests deliberately use an unauthorized transport and do not prove a successful Telegram resume. They verify persisted intent and cancellation before releasing the worker, then durable acknowledgment. Additional tests cover absent owners, repeated acknowledgment and stale generations. These checks do not cover interruption inside an individual AEAD primitive or blocking filesystem call, source pre-admission, account replacement, native previews or full upload-finalization crash windows; those remain required.

## Completion evidence still required

Storage transitions and codec tests are necessary but do not qualify the full feature. Production validation must cover durable batch admission, locked startup hydration, mid-file transport/crypto cancellation, pause/cancel races, ciphertext-cache and partial-file validation, unchanged key/package/part identities, remote success before local receipt, manifest success before final local commit, whole-file verification, retry/backoff, source/key/remote/output failures, stale account owners and bounded memory. UI must show true capabilities, actionable unrecoverable reasons and first-use guidance in English/light native900×600 and full-screen reviews before the goal is complete.

### Partial-file admission checks

The native encrypted-download adapter rejects existing final directory entries (including dangling symlinks) before preparing output. Existing partial paths must be regular files; absent paths use exclusive creation. On Unix the opened file identity must match the pre-open metadata before resizing. Tests preserve empty unrelated symlink targets, dangling links and directories. After preparation the adapter retains the opened file handle for extent reads, writes, whole-file hashing and synchronization; registering a replacement destination drops its previous handle. Finalization rejects non-regular partial names and, on Unix, a named inode different from the retained handle. A deterministic replacement test proves that subsequent writes do not touch the replacement file and finalization refuses it. The remaining race between the final pathname identity check and hard-link publication, platform-specific identity parity, and blocking special-file substitution during open still require hardening; these checks do not claim an adversarial-filesystem guarantee.

### Recovery guidance in the interface

Task detail failure guidance precedes metadata and distinguishes retryable work, blocked source/key problems, other blocked work, and restored history without usable recovery actions. Guidance reflects the existing action availability; it does not offer Resume for a blocked or unsupported record. The storage guide includes pause/resume/cancel behavior, retained source/partial-file requirements, restart eligibility and recovery limits. Its expanded overview is independently scrollable within 220 logical pixels so transfer/file actions remain reachable. English/light native reviews cover 900×600 and actual fullscreen; layout regressions cover early failure guidance and the guide's final paragraph. This is contextual guidance, not proof that every end-to-end recovery case is implemented.

### Native download resume checkpoint ordering

A deterministic regression reproduced an old native worker replacing a newly queued resume with Paused while the resume caller awaited its durable checkpoint. Paused retirement now preserves a newer Queued or terminal projection. Queue admission rejects still-paused/cancelled control tokens and stale non-queued projections; resume retries deduplicated admission after acknowledgment even if the previous owner existed before the write began. No scheduling lock spans the storage acknowledgment. The gated test verifies preserved Queued state, no new attempt during the checkpoint wait (including a capacity notification), and exactly one replacement attempt completing without an external UI wake. This closes that observed resume race; it does not constitute a complete audit of all native cancel, startup and terminal checkpoint races.

### Stale native start and resume retirement

Native worker startup atomically consumes only pending resume/retry tokens and requires the current projection to remain Queued before recording an attempt or entering the backend. A queued command invalidated by pause/cancel performs no backend work. Resume retirement uses compare-and-exchange under the short scheduling guard so it cannot reset a newer pause/cancel/retry token. Pause/resume retirement no longer re-publishes or re-persists command state: the command caller already performed that durable write, and replaying it could overwrite a newer command. Deterministic tests cover stale resume retirement after three newer controls and stale worker entry under five control/projection combinations, preserving attempt/event counts and proving no backend calls. Cancel cleanup ownership and terminal checkpoint ordering beyond these cases remain under audit.

### Native cancellation cleanup ownership

Native cancellation installs a bounded per-task cleanup barrier before signalling stop. After persisting cancellation it waits for retained worker ownership to be released before discarding partial/bitmap files. A 30-second quiescence timeout or cleanup failure returns an error and preserves data rather than deleting under an active writer. Scheduling and history deletion respect the barrier; no scheduling mutex spans storage, the wait, or filesystem cleanup. An immediate retry is retained as a pending request while the backend still sees Cancel. Only successful cleanup allows the queued retry checkpoint and subsequent deduplicated admission. The final checkpoint retains slot ownership to handle a retry arriving during that write. On cleanup failure, the transient retry projection returns to the durable Cancelled state. Old cancellation retirement never persists a projected pending retry before cleanup acknowledgment.

The existing immediate-cancel/retry regression now gates both the old backend and cleanup, verifies no overlap or premature restart despite refill, and exercises successful deletion plus denied cleanup with retained bytes and a still-Cancelled SQLite record. The durable cleanup-owner sections below add restart recovery for saved cancellation/retry obligations, visible waiting/failure phases and stale terminal-write fencing.

### Resume-source preflight cancellation

A resumed upload registers its queued execution token before target/source preparation, then rechecks the persisted generation and Queued state. Register-before-read prevents a stop arriving between that check and hashing from being lost. The source hasher checks cancellation before opening, before/after every bounded 1MiB read, and before returning digest evidence; cancellation is independent of the 100ms presentation throttle. A deterministic reader cancels after its first block and proves that no second read or throttled progress callback is needed to stop. A blocking filesystem read itself is not preempted.

Resume already owns a visible row, so preparation updates its log/telemetry without replacing a newer Pause/Cancel state. The queued registration is released before claiming Running; the existing generation-fenced transport registration takes over. Unclaimed preflight failures are settled by the resume owner from the ledger rather than transiently publishing a generic failure over an accepted stop. Fresh uploads still lack a durable pre-admission job during full source hashing; pending batch admission remains required. The batch command explicitly means stop after the current file, and its semantics are unchanged.

## Pending upload storage boundary

Schema 20 adds a metadata-only queue separate from executable recovery contexts.
Bounded durable admission, indexed restart pages and atomic generation-fenced
promotion are implemented and tested in Storage. Migration rollback preserves
old recovery bytes. The separate source codec and pending control storage are now implemented.
Actual batch admission, pending controls, explicit resume, startup dispatch and
locked history projection are connected. Successful synthetic DesktopVault restart
composition and atomic-selection process-interruption evidence are described below.

### Pending source codec v1

`TARKVQ01` is distinct from the executable `TARKVR01` context. Its ordered fields
are magic (8 bytes), version (LE u32, 1), account (LE i64), task (LE u64), chat
(LE i64), batch (LE u64), creation milliseconds (LE u64), vault ID (16 bytes),
master-key generation (LE u32), native filesystem identity (LE u128), source size,
modified units and revision (three LE u64), logical filename (LE u32 byte length
plus UTF-8), native absolute path (the same platform-tagged bounded path codec as
v1 executable recovery), then BLAKE3 of every preceding byte (32 bytes).
The record is bounded to 128 KiB, filename to 4096 bytes and path to 64 KiB.
Account/chat/task/batch must be positive and SQL-compatible, timestamps must fit
nonnegative SQL integers, source size is nonzero and vault/key generation cannot
be empty. Non-UTF-8 Unix paths survive unchanged. Trailing data, truncation,
checksum corruption, foreign path platforms and future versions are rejected.

Metadata inspection canonicalizes the selected source and reads its native
identity without reading file contents. Pending records contain no content hash,
package, file key or part nonce. Decoding binds account/task/chat/batch/time to
independent database columns. Before promotion, the executable upload context
must match source path/identity/size, display name, task scope, creation time and
vault/key generation. A vault change requires actionable key/reselection guidance,
not silent use of another vault. The actual owner must still hash the complete
source and recheck identity before this promotion boundary; the checksum alone
is neither authentication nor permission to execute.

Pending controls persist pause/cancel/resume/retry/failure with a generation
increment on every accepted change. This invalidates old preflight results even
when a pause is quickly followed by resume. Cancelled/promoted entries never
resume through the pending API; promotion hands ownership to the executable
ledger. Future codecs remain readable as opaque records but cannot transition.
The independent runtime control owner now signals pending preflight registrations
after persisting intent, and projects the resulting state. The codec alone still
does not prove successful end-to-end transfer recovery.

### Batch admission integration

Actual upload selection now allocates all task/batch identities and captures
metadata-only source contexts before the first file executes. It requires the
active vault record and commits the complete selection in one durable transaction
before remote channel validation. Source contexts are encoded one at a time; no
file reads or network operations occur inside the transaction. The background
saving phase stays visible, and its saved-file count advances only after commit.
A cancellation or encoding/write failure rolls back all newly admitted rows.
Only one 128-file window of detailed rows is materialized during execution.
Queued uploads register a preflight cancellation owner before checking the saved
pending state/generation, compare the native source identity before full hashing,
and atomically promote the admitted context before claiming the executable job.
The executable context keeps the original queue creation time.

Shared preflight failures and stop-after-current decisions settle pending entries
as blocked/retryable/cancelled; scheduling failure also settles later admitted
windows. Existing paused intent is retained. A 513-file actual-selection test
reopens an independent database inside the validation boundary, sees every queued
source with no executable job, and verifies durable terminal outcomes for the
entire selection after shared failure/cancellation.

Pending control-owner fallback, resume/startup dispatch and history hydration
are connected; validated queue rows expose their current recovery actions.
Atomic selection admission supersedes the previous independently committed groups.
An independent reader sees no uncommitted prefix; a failure after 256 inserts rolls
back the selection. A subprocess exits without running destructors at the same
boundary, and reopening preserves the old queue with no runnable new prefix.
Account lifecycle and post-admission scheduling boundaries still need qualification.
The saving counter reports committed files, never uploaded bytes or successful
completion. Native review of this incremental phase belongs to final UI
qualification, using English/light at compact size and actual full-screen.

### Pending control, startup and history dispatch

When an encrypted upload has no executable job yet, the independent control owner
loads its pending record, persists pause/cancel before signaling the registered
preflight lease, and projects the accepted state. If promotion won the first
lookup or the pending CAS, control re-reads and applies to the executable ledger.
Repeated stop commands preserve state. Locked-vault controls do not require raw
keys. Tests cover pending lease signaling and a promotion between lookup and
control handoff.

Explicit upload resume now also accepts valid pending Queued/Paused/Retryable
records, retaining the same task, batch, timestamp and source identity. It requires
the active vault/key scope and repeats complete channel validation before the
usual per-file target/source checks. Failed preflight retains the pending context
and supplies a durable retryable/blocked projection. Startup dispatch pages pending
Queued records only, rechecks account revision per task and does not convert a
concurrently paused task back to Queued. Synthetic unauthorized-transport tests
prove dispatch/failure preservation, not a successful remote resume.

Locked history hydration includes pending entries that never acquired legacy
history rows. Account/state/id index queries bound candidates per state, prioritize
queued work and avoid scanning promoted receipts. Invalid/future source contexts
remain non-actionable placeholder rows. The final history count is an exact ID
union across legacy, pending and executable records, so compatibility rows do not
double-count tasks. The current 256-row view still uses ID order for candidate
selection; full history navigation remains under qualification. A 300-entry test verifies the low-ID queued task stays
visible, duplicate history appears once, and 44 omitted entries are reported.

### Native restart test activation boundary

The observed Running-versus-Queued restart assertion was caused by the test helper
calling `activate_account` before checking the restored checkpoint. That method
intentionally refills the queue. A gated restarted backend reproduces Running at
that point deterministically. The regression now constructs an inactive owner,
checks Queued and retained bytes without any backend entry, activates the account,
checks Running while the backend is held, then verifies completion and accumulated
duration/rate. It needs no extra presentation-driven scheduling call. This resolves
that test’s timing assumption; other stale-owner persistence and shutdown boundaries
still require their own qualification.

Pending resume keeps the admitted display name even though the source path is
canonical. Resolving a selected symbolic link must not silently rename the upload
or invalidate the immutable name check during pending promotion.

### Successful service restart composition

Instance-owned test wire replies exercise `DesktopVault`, its control queue, real
source files, encryption/authentication, `TelegramObjectStore` mapping and SQLite
together. Acceptance cases cover:

- Pending alias-name preservation across pause/restart and authenticated download.
- Upload transport pause with identical publication ID and ciphertext on restart.
- Download transport pause with the same restored task and destination.
- Saved-manifest completion after deleting the synthetic original source.
- Part and manifest publication whose successful reply is lost: remote bytes exist
  without a verified local receipt; after pause/restart or a durable network failure
  and explicit retry, reconciliation completes without another send. Manifest
  recovery also succeeds with the original synthetic source deleted.
- A 61 MiB file using the production 60 MiB part limit: pause during the second
  part, restart, recheck and reuse the first verified local extent, fetch only the
  interrupted part, then compare the entire output hash and both durable receipts.
- Upload and download network failures persisted as Retryable, followed by restart
  and successful retry with unchanged recovery context and a new generation.
- Upload and download cancellation acknowledged during transport and retained
  across restart. Cancelled is terminal: resume is rejected before remote work;
  starting again requires a new task. Original source files remain intact.
- Stopping a three-file upload batch while the current file's transport is
  blocked. The independent control owner acknowledges the durable stop before
  transport is released; the current file completes and two queued members stay
  cancelled after restart and automatic queue restoration. Only the current
  file's encrypted part and manifest are published.

These tests use synthetic wire replies, not real MTProto delivery. They do not
qualify all shutdown, lost-acknowledgment, account-replacement or cleanup crash
windows by themselves.

This composition exposed two actual defects. Resumed queue/history projections
retained a zero part count after part geometry became known, so completed receipts
violated the compatibility history's part-count constraint. Upload admission now
refreshes part geometry and execution start time while preserving control intent.
The acceptance test checks both the completed ledger and completed projection.

Encrypted download previously downloaded and decrypted each part for discovery,
then repeated both operations before writing. The connected download path now
reads directly from the authenticated manifest locator and retains the verified
plaintext for the durable writer. It still checks ciphertext, frame authentication,
manifest metadata, source scope and plaintext integrity. The operation-count test
requires one fetch per needed part and no fetch for a verified reused extent;
a transport regression rejects this entry point without an opened manifest and
rejects modified encrypted bytes.


### Durable native cancellation cleanup

Schema 21 adds the cancellation cleanup contract described in
[the data model](DATA_MODEL.md#native-cancellation-cleanup-schema-21).
DesktopTransfers now routes cancellation and cleanup-aware retry through an
independent bounded control owner. It commits cancellation and the cleanup
obligation before signalling the old download, and saves retry intent before
acknowledging it. Cleanup workers hold no scheduling mutex during writer waits,
filesystem work or SQLite acknowledgment. A second retained worker handles file
removal, so one blocked cleanup does not stop ordinary downloads.

Startup restores pending cleanup before account activation may schedule downloads.
Queued retries stay fenced until successful cleanup and requeue commit together.
Failure retains partial data and retry intent, removes the active ownership claim,
and exposes a retryable cleanup error; future codecs or mismatched attempts remain
non-actionable. The retained threads do not join from frontend Drop. Shutdown
leaves any unacknowledged obligation available on the next launch.

Native list rows, collapsed batches and the inspector distinguish waiting for the
writer, removing partial files and cleanup failure. They show phase elapsed time,
last activity, preserved-file guidance and the durable retry state. Cleanup events
join the bounded lifecycle timeline. Delete/pause/resume/cancel actions are hidden
while cleanup owns the task; only supported retry is offered. The existing
presentation clock refreshes elapsed labels, without polling Storage or driving
cleanup scheduling. The cleanup control loop dispatches on startup and commands,
not timer ticks; its timeout only observes shutdown.
The global status bar also retains an account-scoped cleanup entry on other
pages. It prioritizes cleanup failure, opens the task inspector, clears a stale
search and expands the containing batch. The revision/owner-keyed rate cache
also caches this entry; idle renders do not rescan retained task history and
cleanup never contributes an invented throughput sample.

Real temporary-file/service tests verify restart after a denied cleanup with a
saved retry, no premature attempt while the restarted cleanup is blocked, an
unrelated completed download during that wait, and exactly one eventual retry.
A separate case removes the actual native partial/map names while preserving an
existing final file and never calling download. The same restart test covers a
removed output directory: cleanup synchronizes the nearest surviving ancestor,
acknowledges the absent partial, and does not recreate the removed directories.
Synchronization failures retain the obligation; permission errors remain distinct
from other persistence errors. Dropping the frontend while cleanup
is blocked returns without waiting and retains the obligation. These tests do not
qualify foreign path substitution or directory synchronization on every platform.

## Restored pending upload batch controls

`submit_stop_upload_batch` routes to the independent control owner, so the GUI
can retain a background wait while the transfer owner is busy. The pending
selection ledger cancels pending and promoted queued upload members in one durable
transaction, excluding the registered current preparation. Only after that commit does the owner signal
the live selection boundary token and update retained cancelled rows. A restored
selection needs no in-memory batch handle. The existing fast shutdown boundary
API stays separate and does not acquire a database connection on the UI thread.

A temporary-database Runtime test verifies account isolation, restored operation
without a live batch, current-preparation preservation, immediate cancellation
projection, and restart persistence. Storage tests verify atomic rollback and
large-selection coverage, rollback of both ledgers when the formal update fails,
and both start/stop orderings: a running owner is retained, while a stale start
cannot execute a cancelled queued job. Running owners and future codecs remain
unchanged. Formal cancellation preserves the generation contract and records its
time. For legacy jobs without a pending receipt, the same transaction uses the saved
upload-history batch only when account and source chat also match. Any existing
pending receipt remains authoritative, including future codecs or conflicting
batch metadata. Tests cover these exclusions and verify that the actual update's
legacy branch uses the batch-history index. Full service promotion/control races,
and native window interaction remain integration work. When a valid formal
recovery context accompanies older upload history, history hydration now uses
the ledger's queued, running, pausing and cancelling states rather than retaining
the legacy interrupted label. A locked-history regression uses a real encoded
recovery context and persisted transitions to verify those phases and the batch
association without unlocking keys. Records without a valid recovery context
retain the legacy interrupted handling.

### Terminal ledger and missing upload summaries

When a valid upload ledger and an older summary disagree, hydration uses the
ledger's completed, retryable or blocked outcome. Completed uploads recover
known file bytes, part geometry and package identity; stale summary speed and
duration are cleared rather than inferred. Known failure codes retain their
specific explanation. Unknown codes stay visible with recovery disabled. Scope
checks include the source chat, filename and size before overlaying a summary.

A bounded direction-index page also recovers uploads with no history summary,
including queued and completed jobs and a non-runnable placeholder for damaged
contexts. This path reads metadata only while the Vault is locked. A valid
promoted admission receipt restores its batch association. The combined list
retains its existing display bound and omitted-count disclosure. Tests exercise
terminal ledger transitions over stale summaries and queued/completed/damaged
jobs without summaries using temporary SQLite and synthetic recovery contexts.

## Connection replacement and cancellable final verification

Every real encrypted-transfer registration retains a bounded lifecycle
subscription. Connection/account replacement signals the operation token without
I/O or acknowledged owner calls inside the callback. Registration checks the
revision both before and after subscription to close the admission race. A token
interruption without a persisted user cancellation remains retryable; explicit
pause/cancel still follows its authoritative ledger state. The service test blocks
part transport, replaces the account, releases transport and verifies the original
account retains a retryable job and the new account has no copied job.

Final partial-file hashing and verification of an already-published file check
cancellation before each bounded 1 MiB read. Publication checks again after flush.
A blocking individual OS read or flush cannot itself be preempted. Tests verify
cancellation between reads and retention of the partial without final publication.

Inspector concurrency values are labeled as configured limits. The unmeasured
worker count is omitted. Restoring upload history propagates database failures
instead of silently treating them as an absent recovery record; malformed records
remain non-executable placeholders.

## Shared output admission and process interruption

The library reserves each new native or encrypted output by exclusively creating
its sibling `.partial` file before returning the destination. Two owners choosing
the same visible filename cannot both acquire that reservation; the losing owner
tries the next suffix. Existing final files, retained partials and saved native
reservations remain excluded. Eight concurrent owners exercise this admission
boundary. A crash before the task ledger is admitted may leave an empty reserved
partial; it is preserved and never represented as a resumable task without a
context. No unrelated output is overwritten to reclaim its name.

The abrupt-download regression exits a real child process with status 73 without
running Rust destructors, both after a durable first extent and after publication
but before terminal persistence. Reopening the database advances the execution
generation, re-verifies local bytes, fetches only a missing extent and accepts an
already-published verified output without another remote download.
