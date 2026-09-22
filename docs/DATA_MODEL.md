# Data model and SQLite contracts

The current schema is version **22**. Ordered migrations and tests preserve existing data; Rust/Serde layout never defines durable representation. Crypto/manifest bytes have separate versioned contracts. `LogicalFile` is the domain object; all persisted enums and identifiers are locale-neutral.

## Identity and projections

| Entity | Identity / invariant |
| --- | --- |
| Logical file | Stable `LogicalFileId`, original Unicode name, plaintext size, kind, source, remote/encryption/verification states |
| Remote object | `(account_id, chat_id, message_id)`; volatile references/access hashes are refreshable adapter hints |
| Vault package | One logical file, authenticated manifest generation and ordered parts covering the file without gaps/overlap |
| Account/chat | Chats and remote projections are account scoped; no session secrets in ordinary rows |
| Collection | Membership references logical files, never encrypted pieces |
| Index range | Account/chat plus policy fingerprint/version and actual committed coverage |

Storage allocates local IDs transactionally and monotonically; deleting the highest row cannot reuse its ID. Original names/captions/paths are preserved, while searchable projections may normalize separately. Local source paths use an explicitly tagged platform encoding, including non-UTF-8 Unix paths; they are not portable manifest paths.

A native file maps to one remote object. A Vault manifest binds package/vault identity, File Key wrap, exact plaintext/encoded sizes, whole/part hashes and part locators. Application part indices are contiguous from zero and their ranges cover exactly the logical size. The current managed-file list is reconstructed from authenticated remote manifests; local package/manifest/file-part tables are not implemented.

## Implemented tables and evolution

```text
accounts, chats, remote_objects
logical_files, logical_files_fts
index_jobs, index_ranges, telegram_index_state
channel_sync_state, channel_sync_tombstones, channel_file_versions
managed_channel_watches, managed_channel_changes
transfer_tasks, transfer_parts
native_download_batches, native_download_tasks
collections, collection_items
settings, id_allocators, vault_metadata, vault_downloaded_files
vault_key_epochs, vault_inventory, vault_message_health, vault_upload_history
```

Migrations 1–6 establish the catalog, FTS triggers, checkpoints/ranges, tagged paths, monotonic IDs, remote-object identities, cursors and native history. Version 7 adds native batch identity and source sent-time/caption/MIME metadata; version 8 adds wrapped Vault metadata; version 9 adds native account scope; version 10 adds the Vault output inventory; version 11 adds Channel synchronization cursors and deletion evidence; version 12 adds per-message projection versions and durable private-channel observations without rewriting existing records. Tests cover empty-to-latest and every prior-version upgrade, preserving indexed data.

Connections enable foreign keys, an untrusted schema, busy timeout and WAL for file databases. Strict tables, checks, prepared statements and transactions enforce repository invariants. SQL remains exclusively in Storage. Future schema versions/application IDs are rejected rather than guessed.

The current migrator reads `user_version` and applies each missing migration in order, committing its SQL and version marker in the same transaction. This supports skipped schema versions and retry from the last committed step. [ADR 0017](adr/0017-versioned-automatic-migrations.md) extends the product contract to an automatic Runtime migration coordinator: expose detection/preparation/conversion/verification/index phases before expensive work, preserve a recovery path, and resume or roll back interrupted work without user scripts. The desktop now paints a startup acknowledgment before opening local state, opens/migrates it on retained background workers, and presents detection, transaction preparation, conversion and verification events in a bounded timeline. The final schema step checks SQLite structure and foreign keys before commit. Failures retain the database and offer retry/access/disk/version guidance. This covers ordered SQLite upgrades; cross-format conversion, external backup orchestration and complete encrypted-format migration qualification remain unfinished. A release records its supported schema paths; migration never guesses unknown account ownership or deletes user files.

## Native download history: version 9

`native_download_tasks.account_id` is nullable `INTEGER`, with a check requiring a positive value when present. The index is `(account_id, state, created_at_unix_ms, id)`. New inserts require a positive account, chat and message ID. All members of a batch share account and chat; batch header and children commit atomically. Updates cannot rebind a known task to another account or erase its account.

Existing version-8 rows migrate with NULL ownership. Before the first configured Telegram connection returns, the runtime calls the storage resolver with the actual restored account, or no account when unauthorized. In one transaction it assigns all unknown rows only if that old session was restored, then writes the setting `native-download-account-migration.v1 = resolved`. Presence of this marker prevents subsequent assignment, including after a new login or restart. Unknown rows remain in history and cannot execute; the user can enqueue a fresh download from a known account/source.

Runtime snapshots preserve `Option<i64>` for legacy ownership. Native scheduling, resume/retry and the actual serialized Telegram operation enforce expected account identity. Switching pauses queued/running work and waits for worker release before sign-out. Local completed files are retained. Tests exercise upgrade/reopen, unknown provenance, attempted reassignment and cross-account operation rejection.

Native destinations are globally unique across retained history, including terminal and legacy account-unknown rows. Runtime destination selection checks this indexed column even when the corresponding file is absent; deleting a local output does not release its history path.

Native history also retains durable task/batch IDs, destination, original message metadata, size, progress, timestamps, attempts, verification and structured failure. Its restart/partial-file behavior is specified in [Transfer](TRANSFER_ENGINE.md); the `TARKDPM1` bitmap and schema-1 session log are explicit independent formats, unchanged by schema 9.

## Local output inventory: version 10

`vault_downloaded_files` stores a positive account/chat identity, canonical 32-character lowercase hexadecimal package ID, destination using the existing explicit platform path codec, nonnegative size and completion time. `(account_id, path_encoding, destination_path)` is unique; a new successful output at that path updates its identity. This local metadata is plaintext SQLite and is not a crypto/manifest format or a transfer checkpoint.

The account-scoped read API projects completed native history together with Vault outputs, at most 128 rows per page. Each source uses an account/ID index and applies its cursor/128-row bound before the union; no deep OFFSET or whole-history sort is required. Its typed cursor is `(kind, id)`, with native kind 0 before Vault kind 1 and IDs descending within each kind. Unknown-account history is excluded. No speculative backfill or legacy-account reassignment occurs. The literal `v10-vault-download.sql` fixture covers the Unix path encoding and Unicode name; upgrade/reopen, pagination and account isolation have deterministic tests.

Vault restore records its output after verified atomic publication and before reporting success. If the database write fails, the file remains intact and the operation reports a persistence failure. A crash in this narrow interval can leave an unregistered output; automatic filesystem discovery and reconciliation are not implemented. Pre-v10 Vault outputs have no durable local inventory. Native history deletion removes its observation source but never the user file; external deletion does not delete history or change historical completion.

## Channel synchronization: version 11

Migration 11 introduced the fields below. The current supported source/read/write matrix is specified under version 16; the version-11 bytes retain their original meaning. Crypto/manifest/recovery codecs remain version 1; preferences, native bitmaps and wrapped keys retain their existing formats. Migration 11 is additive and transactional, and requires no export, reset or reconfiguration.

`channel_sync_state` is keyed by `(account_id, chat_id)` with a scoped chat foreign key. It stores the nonnegative Telegram `pts`, independent exclusive `history_before` plus `history_exhausted`, `repair_pending` plus `repair_before` for known-ID verification, `gap_pending` plus `gap_before`/`gap_until` for missing recent history, and a positive compare-and-swap `revision`. Absence represents an uninitialized state. Cursors are locale-neutral integers; PTS is an update sequence and message IDs are exclusive scan bounds, never proof that every integer should exist.

Each bounded commit validates account/chat scope, the expected revision and nonregressing PTS. File metadata, deletion tombstones and all cursor changes share one SQLite transaction. A failed/stale page leaves the previous state recoverable for replay. Authoritative difference/verification can apply same-second edits; ordinary history cannot revive tombstoned messages. `channel_sync_tombstones` uses `(account_id, chat_id, message_id)` and suppresses removed documents in the Channel projection. Deletions mark remote presence missing while retaining logical identity, local paths, encryption/package metadata and transfer history. Changed remote metadata does not inherit verification of old contents.

An initial recent page captures dialog PTS before the read, followed by difference catch-up and legacy-cache validation. Server-declared `TooLong` records gap recovery back to the previous local boundary, then independently verifies cached IDs; each history/verification page commits its cursor so restart continues safely. Message history is server-limited for very old gaps, so previously unindexed old files can remain outside this projection; a persisted private gap warning retains that uncertainty. This is not exhaustive exact-ID range reconstruction. Neither path reinterprets old `telegram_index_state` or claims generic index-range coverage. Selection priority, retry deadlines and the bounded event timeline are transient; durable cursors and pending recovery survive restart. Tests cover every prior schema, skipped upgrades, mid-migration DDL failure, SQLite disk-full rollback/retry, retained setting/key fixture bytes, atomic page rollback, stale CAS, deletion replay, scope and gap-repair restart.

## Incremental views and managed-channel observations: version 12

Migration 12 introduced the additive tables and indexes below. Version-12 bytes retain their original meaning; the current supported source/read/write matrix is specified under version 16.

`channel_file_versions` is keyed by account/chat/message and stores a positive local commit revision for changed metadata or explicit invalidation. It is independent of Telegram PTS, timestamps and encrypted format versions. Unchanged ordinary pages leave this value alone. Explicit edits and post-gap revalidation invalidate derived manifests even when filenames, sizes and timestamps compare equal. History/search only inserts previously unknown IDs and cannot overwrite a newer push. A coherent `cached_channel_view` reads rows and their channel revision in one SQLite snapshot; commit outcomes return actual upserts/deletions for bounded per-channel GUI deltas.

`managed_channel_watches` is keyed by account/chat, with `catalog_ready`, cumulative `change_count`, monotonic `acknowledged_count` and optional last-change time. It establishes observation only and never authorizes remote writes or substitutes for private-channel identity checks. `catalog_ready` records completion of bounded historical manifest discovery, not complete channel history. `managed_channel_changes` stores sequence, message ID, locale-neutral `edited`/`deleted`/`gap`, observed PTS and wall-clock observation time. Message ID 0 is reserved for a gap of unknown affected messages. `(account, chat, message, kind, pts)` deduplicates replay. Only 128 recent records are retained; cumulative counts disclose omitted history. Acknowledgement advances at most through the caller's displayed sequence, preserving newer unread changes.

Observation records and file changes share the PTS transaction. Normal new messages do not create edit warnings; failed/CAS-conflicting commits do not advance warnings or cursors. Deletions preserve recoverable local evidence and suppress deleted manifest candidates. The candidate lookup uses scoped indexed captions and returns at most 1,001 records to expose the 1,000-file display bound. Derived authenticated metadata remains an account/chat/version-scoped memory cache, is bounded at 16 MiB and is cleared on lock; it stores neither File Keys nor full decrypted manifests. [ADR 0019](adr/0019-event-driven-channel-projections.md) defines the event and recovery contract.

## Effective search timestamp indexes: version 13

The original schema-13 implementation accepted schema 0 and upgraded schemas 1–12. The current release matrix is superseded by version 16 below. Newer schemas and different application IDs remain intact with actionable guidance. No export, reset or reconfiguration is required.

Migration 13 adds four search indexes on `COALESCE(modified_at_unix_ms, created_at_unix_ms, -9223372036854775808)` and descending logical-file ID: global, account, account/chat and kind scopes. Existing raw timestamp indexes continue to support explicit timestamp-range facets. No data rows, wrapped keys, settings or recoverable transfer state are rewritten. Search cursor version 1, the null sentinel and tie ordering retain their meaning.

The retained background migration owner reports detection, preparation, indeterminate index construction and verification. SQLite's transaction preserves the original schema/data if construction is interrupted or storage is full; integrity checks precede committing version 13. Restart repeats only uncommitted work. Fixtures cover every prior version, failed index creation, disk-full rollback, wrapped-key/generation and setting preservation, successful restart and idempotent reopen.

## Core transfer and index records

Core task/part transitions live in `teleark-core`; storage validation does not replace that state machine. A checkpoint transaction replaces the task and ordered parts together and rejects noncontiguous indices, gaps, mismatched totals/progress, invalid codec version/data pairs, or a completed task lacking verified parts. Exact state rules are in [Transfer](TRANSFER_ENGINE.md).

Index rows persist account/chat, inclusive message-ID bounds, partial/complete state, checkpoint/counters, policy identity, generation and update time. Overlap within the same scope/policy is rejected. File upserts, job progress and range evidence can commit atomically. Interactive cache rows never imply surrounding history coverage. Date/order-rich ranges, compaction and generic coordinator integration remain unfinished; see [Index](INDEX_ENGINE.md).

## Settings and wrapped keys

The `settings` table stores explicitly versioned preferences, locale override, legacy API application credentials, storage-channel bindings and the one-time native-account marker. [Preferences format](PREFERENCES_FORMAT.md) defines active encodings. A personal API ID/Hash pair is saved/removed transactionally; the GUI receives only its ID/source status. The SQLite database is not encrypted. Telegram user sessions are in a separate adapter-owned protected cache.

`vault_metadata` is a singleton with the 16-byte Vault ID, explicit Password/Recovery Wrap bytes, nonzero generations and timestamps. It contains no raw password, Recovery Key, unwrapped Master Key, File Key or KEK. OS credential bindings are absent. The remotely authoritative crypto/manifest codecs do not change when this row's in-memory model changes.

## Search and deletion

FTS5 covers normalized filename/path/caption projections while preserving original text. Structured facets include account/channel, type, date, size, extension and local/remote/encryption/verification state. Library query DTOs also project the exact source channel title and an optional native message ID, scoped by the file's account/chat. Only a single matching non-package remote object supplies that ID; ambiguous multi-object projections remain unavailable. These are read projections over existing tables, with no migration or persistent encoding change. The durable `uploaded` enum indicates recorded remote presence, not who uploaded it; the catalog UI labels unverified records as indexed.

Current ordering is modified-time descending with logical-file ID as a unique tie-breaker. Opaque versioned keyset cursors bind the query/facet fingerprint; changed queries reject old cursors. Never concatenate user text into SQL or use deep OFFSET for scale.

Deleting catalog/history rows is distinct from deleting remote content. Collection removal does not remove files. Terminal native-history deletion removes owned logs/partials/bitmaps but never a completed user file. Remote package cleanup requires explicit reachability and recovery policy; it is not a database cascade. Smart-collection rule evaluation, other sort orders and million-record performance claims remain future work.

## Library local-copy projection

The local Library reads native/Vault completed-output inventory for the current account and imported originals with local-only catalog state. Runtime checks file metadata off the UI thread and excludes missing/inaccessible/non-file paths. Current file size/date are observations, never new cryptographic verification. Native-download, Vault-download and imported catalog keys remain separate; local copies do not invent LogicalFile IDs. No schema changes or backfill are needed.

Local pages read at most 128 candidates per storage request, cooperatively skipping empty/unmatched pages until results or exhaustion. An ephemeral typed cursor binds account, filename search and type filter; it is not serialized or persisted. Cancellation is checked between candidates and storage pages. The UI resets/cancels on source/filter/navigation changes and rejects stale account/generation responses. Imported originals remain available without a signed-in account. Remote catalog queries require and filter by the current account. The local view does not scan arbitrary disk directories or recover outputs absent from retained inventory.

## Explicit network policy: version 14

This migration introduced schema 14. The earlier v0.4.4 checkpoint accepted schema 0 (new database) and upgraded schemas 1–14 to read/write 15; version 16 below supersedes that matrix. Unknown/newer schemas remain intact and are rejected. Migration 14 inserts the version-1 explicit direct policy only when absent, preserving existing settings, wrapped keys, files and recoverable transfers. Its transaction validates the database before committing the version. Failure rolls back; restart resumes missing steps without scripts or reconfiguration. The existing background migration phases cover this step.

The schema increment prevents older readers (maximum 13) from opening a database and silently ignoring an enabled proxy. Policy bytes have an independent codec documented in [Preferences](PREFERENCES_FORMAT.md#network-proxy-policy-version-1); unsupported policy versions block network startup even when the database schema is supported. Tests cover every prior schema, setting/key preservation, failed policy insertion/rollback, restart and idempotent reopen.

## Schema 15: retained key epochs and file health

`vault_metadata` continues to select the active upload key. `vault_key_epochs` stores independently identified 16-byte vault IDs and the original password/recovery wrappers and their generations. Automatic migration copies the previous singleton; atomic active-key updates retain older epochs. All previous supported database versions upgrade through the ordered transaction chain.

`vault_inventory` has the account/channel/manifest-message composite primary key, opaque remote name and vault ID, authenticated encrypted manifest envelope (maximum 16 MiB), observation timestamp, last full-scan token and an explicit invalid-remote-manifest flag. The first authenticated envelope remains recoverable even after remote corruption/deletion. `vault_message_health` keeps per-account/channel/message presence and observation time; sync tombstones override earlier presence. Unknown means unknown. No new plaintext names or keys are persisted. Inventory reads use descending message-ID keyset pages; a full run never materializes the full inventory. See [ADR 0025](adr/0025-fixed-channel-and-retained-key-epochs.md).


## Upload history: version 16

Current SQLite read/write version is **16**; automatic supported upgrades are **0–15 → 16**, including skipped releases. Migration 16 adds `vault_upload_history` and indexes without rewriting existing bytes. It is transactional, uses the existing visible migration owner and verification before the version commit, and preserves originals on failure. The historical version-15 matrix above is superseded by this section. Crypto, manifest, recovery, preferences and native transfer codecs do not change.

Upload summaries use explicit SQL columns and independent state/error codec v1. The globally unique random task ID is scoped with its account; batch/channel identity, original filename, package ID, bytes/parts, queue/start times, duration and average throughput are durable. `sequence` is local insertion order, so tied timestamps and random IDs do not reorder history. Updates preserve this sequence. States are `queued`, `running`, `completed`, `failed`, `cancelled`, `interrupted`; failure codes are the explicit snake-case ApplicationErrorKind mappings in Runtime's upload history codec, with unknown codes rejected. Completed rows require a package ID and full confirmed byte/part counts. No source paths, content or keys are added.

At service startup, queued/running rows become interrupted without changing their saved totals. Account-scoped reads retain the latest 256 rows plus the complete boundary batch (up to 128 members); omitted older rows remain stored. There is no automatic resend or old-page browser. Runtime writes acknowledged admission/start/terminal summaries outside UI and network reactors; samples remain in memory. See [ADR 0029](adr/0029-durable-upload-history.md) for publication ambiguity, local privacy and legacy-history limits.


## Durable encrypted transfer ledger (schema 17, integration in progress)

Migration 17 is additive. Databases at versions 0–16 automatically reach read/write 17 through the existing ordered, verified transactions. Existing schema-16 upload receipts remain unchanged: they do not contain source/key/part recovery context and must not be promoted into resumable jobs. A failed migration leaves the prior version and original rows intact; restart retries the missing step. Readers supporting only schema 16 reject schema 17, and newer unknown schemas remain intact.

`vault_transfer_jobs` owns account-scoped stable task and package identities, direction, immutable versioned recovery context, persisted control state and an execution generation. Context version 1 is bounded to 2 MiB; raw file/master keys and sessions are forbidden. The runtime context and part reservation codecs are specified in [recovery records](VAULT_TRANSFER_RECOVERY.md); production execution wiring is not yet complete. `vault_transfer_parts` stores immutable, bounded part identity reservations and optional immutable verification receipts. These bytes are interpreted by the runtime's independently versioned codecs, not Rust layout or SQL. Reservations precede encryption; receipts follow authenticated remote verification or fsynced local writes. This storage layer does not itself authenticate cryptographic receipts.

State transitions compare account, task, generation and expected state. Claiming queued work increments its execution generation. Pause/cancel intent is committed before signalling the worker. A worker can receipt an in-flight success while pausing/cancelling, but cannot overwrite control intent with completion. After stop acknowledgement or a new execution generation, late receipts are rejected. Retryable failures require an explicit retry; blocked failures cannot be resumed blindly. Cold-start recovery changes running→queued, pausing→paused and cancelling→cancelled, incrementing generations; this runs before any new execution owner, never on ordinary navigation. Queued recovery still requires runtime source/key/remote validation and ambiguous-publication reconciliation.

Critical ledger transactions use `synchronous=FULL` and `fullfsync=ON`, restoring the connection's prior policy afterward. In WAL mode FULL commits add the durability sync that NORMAL omits; fullfsync requests the stronger macOS sync where supported. See [SQLite synchronous](https://www.sqlite.org/pragma.html#pragma_synchronous) and [fullfsync](https://www.sqlite.org/pragma.html#pragma_fullfsync). Actual storage hardware must honor synchronization; tests verify policy, rollback and file reopen, not physical power-loss qualification. Account/state/id keyset pages use `vault_transfer_jobs_schedule`, verified with the actual filtered deep-cursor query plan, and return at most 256 identities per page. Part pages are also bounded to 256.

## Manifest publication outbox (schema 18, integration in progress)

Migration 18 adds `vault_manifest_outbox` without rewriting schema-17 jobs or part receipts. Supported databases 0–17 automatically reach read/write 18 through ordered transactions; a failed step retains its prior version and existing rows. Readers supporting only earlier schemas reject version 18; unknown newer versions are never reset. The recovery context and part codecs remain version 1 with unchanged meanings.

Each `(account_id, task_id)` has one immutable versioned content commitment and nonzero signed Telegram `random_id`. Version 1 commits the exact canonical manifest header and metadata before encryption. An optional immutable encrypted envelope (at most 67,239,936 bytes) is saved before remote send; an optional positive message ID is saved only after reading back and comparing the entire envelope. The foreign key preserves job scope. No plaintext manifest or unwrapped key is stored in this outbox.

All reservation/envelope/receipt writes use the same full-sync transactions and generation fencing as the job ledger. Reservations and envelopes require Running; late verified receipts may be saved during Pausing/Cancelling, never after acknowledgment or a newer owner. Retry cannot replace the commitment, sending ID, ciphertext or receipt. The runtime authenticates saved ciphertext and compares its decoded public header and metadata before using it. Future outbox codecs are retained but cannot publish through the version-1 writer.

## Recovery history index (schema 19)

Migration 19 adds `(account_id, direction, id)` on encrypted transfer jobs. It changes no context, extent or outbox bytes. Supported versions 0–18 upgrade automatically through the ordered transactions to read/write 19; older binaries reject this newer schema and preserve it. Download history uses this index for account-scoped descending ID pages, a strict cursor after the first page and a limit of at most 256. The first page includes the maximum supported task ID. Tests check the actual deep-cursor query plan and absence of a temporary sort.

## Pending upload admission (schema 20)

`vault_pending_uploads` stores account/task/chat/batch identifiers, creation time,
independently versioned opaque source metadata (1–131072 bytes), generation,
state and an optional semantic failure code. States are queued, paused, cancelled,
retryable, blocked and promoted; only retryable/blocked carry a failure code.
This additive migration preserves existing job contexts, part receipts and
manifest outboxes unchanged, including upgrades skipping earlier releases.
Migration failure rolls back the current step and a later open retries it.

Complete-selection admission commits all metadata in one fully durable transaction,
encoding one bounded record at a time. The older bounded-group entry remains for
individually admitted groups; the actual upload selection uses the atomic iterator.
Duplicate admission must match immutable metadata and cannot rewind state.
Iterator failure or process death cannot expose a runnable prefix. Saving is
presented before work; its committed-file counter advances only after commit. The indexed account/state/id
cursor returns at most 128 queued entries, including unknown codec versions so
unsupported work remains available for explanation rather than being discarded.

Promotion compares the queued generation and full pending identity, inserts the
separate immutable v1 executable job, and marks the queue entry promoted in one
transaction. Failure leaves the original entry queued. Paused/cancelled entries,
stale generations, future codecs and conflicting executable jobs cannot promote.
The pending record remains as an idempotency receipt; retention qualification is still in progress. Runtime queue/control/startup/history integration is connected. The source
codec is specified in [transfer recovery](VAULT_TRANSFER_RECOVERY.md#pending-source-codec-v1).
Pending pause/resume/retry/cancel/failure transitions compare the expected state
and generation, increment generation and retain immutable metadata. Future
codecs cannot transition. No upload parts, key wraps
or file contents are generated by this storage admission API.

Pending history candidates use bounded descending-ID pages through each state
prefix of the schedule index, with Queued candidates prioritized over terminal
history. Combined encrypted-history totals count the ID union of compatibility
upload rows, non-promoted pending entries and executable jobs; one task is counted
once even while its durable representations overlap. These queries run during
history hydration, never on an idle presentation timer.


## Native cancellation cleanup (schema 21)

Migration 21 adds `native_download_cleanup` without modifying native task rows,
Vault contexts, keys or existing receipts. Supported schemas 0–20 upgrade through
ordered transactional steps to read/write 21; applications supporting only older
schemas reject this database version. A conflicting schema object leaves the old
version and bytes intact. Existing cancelled tasks are not automatically assigned
cleanup obligations whose ownership was never recorded.

Each cleanup record references a native task with `ON DELETE RESTRICT` and stores
codec version 1, the owning attempt, request timestamp and a durable retry flag.
Beginning cleanup atomically records cancellation and the obligation under FULL
synchronous/fullfsync policy. Completed tasks, foreign accounts and stale attempts
cannot acquire the obligation. While it exists, sampled progress cannot revive
Cancelled and ordinary task saves cannot queue another attempt or overwrite an
unknown future codec. History includes outstanding cleanup even outside ordinary
terminal retention; deleting the row cannot orphan required cleanup.

Retry only records intent. After the execution owner drains and the backend
exclusively removes and synchronizes TeleArk-owned partial artifacts, acknowledgment
removes the cleanup obligation and queues an accepted retry in the same transaction.
A failed or unacknowledged cleanup remains discoverable in indexed pages of at most
128 records. Future codecs remain intact and cannot be retried or acknowledged.
These Storage APIs do not perform filesystem work or establish that cleanup has
happened. DesktopTransfers now calls them from an independent bounded control
owner; a separate retained worker waits for the old writer, removes partials and
synchronizes the parent before acknowledgment. Startup hydrates pending records
before downloads can be scheduled, preserves retry intent, and attempts supported
cleanup once. A failed cleanup stays visible until retry or another application
start. Future/unsupported cleanup is not executed. Runtime and UI retention protect
these tasks, including when the underlying download state is Cancelled.

Deterministic runtime tests cover blocked cleanup alongside a completing unrelated
download, restart with a previously accepted retry, no-retry cleanup preserving a
final file, and nonblocking frontend destruction with the durable record retained.
Native filesystem substitution and platform-specific directory-sync qualification
remain separate delivery checks.

## Pending upload batch lookup (schema 22)

Version 22 adds `vault_pending_uploads_batch(account_id,batch_id,state,id)`.
The normal transactional migration from every supported earlier version preserves
all task/context bytes and builds this index automatically. No codec changes.
Batch stop constrains account, batch and state through this index, including
promoted receipts; unrelated promoted history is not scanned. A regression checks
the actual filtered update query plan, alongside atomic cancellation and reopen tests.

The same migration adds the direction-scoped `vault_transfer_jobs_history`
expression index: live states sort first, paused/retryable/blocked states second,
terminal states last, with descending task ID within each group. The initial
bounded history projection uses this ordering before applying its 256-row cap.
Existing direction/cursor reads remain available for deeper history consumers.
The regression includes 300 terminal receipts following an older queued job and
checks that no temporary sorting tree appears in the filtered query plan.


## Credential storage (schema 23)

Current SQLite read/write version is **23**. Supported automatic upgrades are **0–22 → 23**, including skipped releases, through the existing visible transactional migration owner. Credential tables independently track the selected backend, a revision, bounded credential identities and optional local payloads, and a durable cleanup outbox. Runtime serializes Keychain work on a separate bounded retained owner. It verifies every target copy before the compare-and-swap backend transaction. Older schema/key/file bytes retain their meaning; no encrypted payload, recovery bundle, manifest or transfer checkpoint codec changes.

macOS defaults on; Windows/Linux default off and reject enabling. A copied macOS library on other platforms retains unavailable Keychain references while switching to local mode, with explicit reentry/recovery guidance. No missing secret is guessed or overwritten. See [ADR 0053](adr/0053-selectable-system-credentials.md) for bounds, cleanup, cancellation and recovery semantics.

The API credential codec is exactly 37 bytes: version byte 1, a positive four-byte little-endian API ID, and 32 ASCII hexadecimal API Hash bytes. Proxy policy readers retain version 1; version 2 has exactly `version`, `mode: credential`, and a 64-hex `credential_id`. Credential payloads retain the explicit version-1 proxy JSON codec. Each new proxy configuration gets a fresh opaque identity; the policy reference commits only if the original record still matches, so delayed migration cannot restore an old route after a newer change. Direct mode remains version 1 with no credentials. Future/damaged values fail closed and are preserved.

Filtered channel batches use existing download schema/codec versions. [ADR 0054](adr/0054-filter-driven-channel-downloads.md) describes bounded filtered discovery and unique per-batch directories.
