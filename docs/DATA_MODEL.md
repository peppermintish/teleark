# Data model and SQLite contracts

The current pre-release schema is version **10**. Ordered migrations and tests preserve existing data; Rust/Serde layout never defines durable representation. Crypto/manifest bytes have separate provisional contracts. `LogicalFile` is the domain object; all persisted enums and identifiers are locale-neutral.

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
transfer_tasks, transfer_parts
native_download_batches, native_download_tasks
collections, collection_items
settings, id_allocators, vault_metadata, vault_downloaded_files
```

Migrations 1–6 establish the catalog, FTS triggers, checkpoints/ranges, tagged paths, monotonic IDs, remote-object identities, cursors and native history. Version 7 adds native batch identity and source sent-time/caption/MIME metadata; version 8 adds wrapped Vault metadata; version 9 adds native account scope; version 10 adds the Vault output inventory. Tests cover empty-to-latest and every prior-version upgrade, preserving indexed data.

Connections enable foreign keys, an untrusted schema, busy timeout and WAL for file databases. Strict tables, checks, prepared statements and transactions enforce repository invariants. SQL remains exclusively in Storage. Future schema versions/application IDs are rejected rather than guessed.

## Native download history: version 9

`native_download_tasks.account_id` is nullable `INTEGER`, with a check requiring a positive value when present. The index is `(account_id, state, created_at_unix_ms, id)`. New inserts require a positive account, chat and message ID. All members of a batch share account and chat; batch header and children commit atomically. Updates cannot rebind a known task to another account or erase its account.

Existing version-8 rows migrate with NULL ownership. Before the first configured Telegram connection returns, the runtime calls the storage resolver with the actual restored account, or no account when unauthorized. In one transaction it assigns all unknown rows only if that old session was restored, then writes the setting `native-download-account-migration.v1 = resolved`. Presence of this marker prevents subsequent assignment, including after a new login or restart. Unknown rows remain in history and cannot execute; the user can enqueue a fresh download from a known account/source.

Runtime snapshots preserve `Option<i64>` for legacy ownership. Native scheduling, resume/retry and the actual serialized Telegram operation enforce expected account identity. Switching pauses queued/running work and waits for worker release before sign-out. Local completed files are retained. Tests exercise upgrade/reopen, unknown provenance, attempted reassignment and cross-account operation rejection.

Native history also retains durable task/batch IDs, destination, original message metadata, size, progress, timestamps, attempts, verification and structured failure. Its restart/partial-file behavior is specified in [Transfer](TRANSFER_ENGINE.md); the `TARKDPM1` bitmap and schema-1 session log are explicit independent formats, unchanged by schema 9.

## Local output inventory: version 10

`vault_downloaded_files` stores a positive account/chat identity, canonical 32-character lowercase hexadecimal package ID, destination using the existing explicit platform path codec, nonnegative size and completion time. `(account_id, path_encoding, destination_path)` is unique; a new successful output at that path updates its identity. This local metadata is plaintext SQLite and is not a crypto/manifest format or a transfer checkpoint.

The account-scoped read API projects completed native history together with Vault outputs, at most 128 rows per page. Each source uses an account/ID index and applies its cursor/128-row bound before the union; no deep OFFSET or whole-history sort is required. Its typed cursor is `(kind, id)`, with native kind 0 before Vault kind 1 and IDs descending within each kind. Unknown-account history is excluded. No speculative backfill or legacy-account reassignment occurs. The literal `v10-vault-download.sql` fixture covers the Unix path encoding and Unicode name; upgrade/reopen, pagination and account isolation have deterministic tests.

Vault restore records its output after verified atomic publication and before reporting success. If the database write fails, the file remains intact and the operation reports a persistence failure. A crash in this narrow interval can leave an unregistered output; automatic filesystem discovery and reconciliation are not implemented. Pre-v10 Vault outputs have no durable local inventory. Native history deletion removes its observation source but never the user file; external deletion does not delete history or change historical completion.

## Core transfer and index records

Core task/part transitions live in `teleark-core`; storage validation does not replace that state machine. A checkpoint transaction replaces the task and ordered parts together and rejects noncontiguous indices, gaps, mismatched totals/progress, invalid codec version/data pairs, or a completed task lacking verified parts. Exact state rules are in [Transfer](TRANSFER_ENGINE.md).

Index rows persist account/chat, inclusive message-ID bounds, partial/complete state, checkpoint/counters, policy identity, generation and update time. Overlap within the same scope/policy is rejected. File upserts, job progress and range evidence can commit atomically. Interactive cache rows never imply surrounding history coverage. Date/order-rich ranges, compaction and complete incremental synchronization remain unfinished; see [Index](INDEX_ENGINE.md).

## Settings and wrapped keys

The `settings` table stores explicitly versioned preferences, locale override, API application credentials, storage-channel bindings and the one-time native-account marker. [Preferences format](PREFERENCES_FORMAT.md) defines active encodings. A personal API ID/Hash pair is saved/removed transactionally; the GUI receives only its ID/source status. The SQLite database is not encrypted. Telegram user sessions are in a separate adapter-owned protected cache.

`vault_metadata` is a singleton with the 16-byte Vault ID, explicit Password/Recovery Wrap bytes, nonzero generations and timestamps. It contains no raw password, Recovery Key, unwrapped Master Key, File Key or KEK. OS credential bindings are absent. The remotely authoritative crypto/manifest codecs do not change when this row's in-memory model changes.

## Search and deletion

FTS5 covers normalized filename/path/caption projections while preserving original text. Structured facets include account/channel, type, date, size, extension and local/remote/encryption/verification state. Current ordering is modified-time descending with logical-file ID as a unique tie-breaker. Opaque versioned keyset cursors bind the query/facet fingerprint; changed queries reject old cursors. Never concatenate user text into SQL or use deep OFFSET for scale.

Deleting catalog/history rows is distinct from deleting remote content. Collection removal does not remove files. Terminal native-history deletion removes owned logs/partials/bitmaps but never a completed user file. Remote package cleanup requires explicit reachability and recovery policy; it is not a database cascade. Smart-collection rule evaluation, other sort orders and million-record performance claims remain future work.
