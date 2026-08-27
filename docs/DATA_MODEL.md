# TeleArk Data Model

Status: conceptual model remains provisional. A pre-release SQLite storage foundation now implements ordered schema migrations, file/search persistence, Telegram remote-object identity and scan cursors, settings, collections, transfer checkpoints, and index job/range records. Package/manifest/Vault persistence is still missing, and no schema has been released as a compatibility guarantee.

## Modeling rules

The model is file-centric and locale-neutral. Stable newtypes prevent accidental ID mixing. Persisted enums use stable symbolic/integer representations, never translated labels. Telegram/`grammers`, SQLite row, and GUI view-model types are adapters around project-owned domain types rather than the domain itself.

## Identity types

Expected IDs include `LogicalFileId`, `PackageId`, `ManifestId`, `RemoteObjectId`, `FilePartId`, `TransferId`, `TransferPartId`, `IndexJobId`, `IndexRangeId`, `AccountId`, `ChatId`, `CollectionId`, and `VaultId`. IDs need stable serialized representations; remote Telegram identity additionally includes account, chat, and message identity.

## Core entities

### LogicalFile

The item shown in the library and referenced by search, collections, and transfer history.

| Field | Meaning |
| --- | --- |
| `id` | Stable TeleArk identity |
| `name` | Original Unicode filename |
| `relative_path` | Optional original logical path; encrypted in Vault manifests |
| `size_bytes` | Plaintext logical size |
| `media_kind` | Locale-neutral category such as video/document/archive |
| `mime_type` / `extension` | Original normalized metadata where available |
| `caption` | Original Telegram/user content, not localized |
| `source_chat_id` | Optional source channel identity |
| `created_at` / `modified_at` | Domain timestamps with explicit UTC/offset policy |
| `storage_kind` | Native remote object or Vault package |
| `verification_state` | Stable state, rendered by the frontend |

A native Telegram media file resolves to one `RemoteObject`. A Vault file resolves to one `Package` and its parts.

The SQLite adapter currently persists the Core projection plus relative path, MIME type, extension, caption, local availability, and an optional absolute local source path. Logical-file IDs allocated by storage use a transactional monotonic allocator so deleting the highest row does not cause its identity to be reused. Local paths use an explicitly tagged platform representation rather than assuming UTF-8; they remain local-machine metadata and are not a portable manifest field.

### RemoteObject

Represents one Telegram-hosted object without leaking `grammers` types.

```text
RemoteObject
  id
  account_id
  chat_id
  message_id
  remote_kind            # native, vault_part, manifest
  opaque_remote_name
  expected_size_bytes
  transport_locator_data # adapter-owned/versioned, never a GUI concern
  observed_at
  availability_state
```

The `(account_id, chat_id, message_id)` tuple is required for unambiguous multi-account operation. File references/access hashes are volatile adapter data, not durable domain identity by themselves.

### Package, Manifest, and FilePart

`Package` groups the authoritative versioned manifest and ordered application parts for one Vault `LogicalFile`.

```text
Package 1 --- 1 LogicalFile
Package 1 --- 1 authoritative Manifest generation
Package 1 --- N FileParts
FilePart 1 --- 1 RemoteObject
Manifest 1 --- 1 RemoteObject
```

`FilePart` records zero-based index, plaintext offset/length, encoded/ciphertext length, frame count, plaintext/ciphertext BLAKE3 digests, and verification state. Part ranges are contiguous, non-overlapping, and cover exactly `LogicalFile.size_bytes`.

### Account and Chat

An account represents a Telegram authorization identity and references adapter-managed session/credential locations, never raw secrets in ordinary database/debug output. A chat belongs to an account. Index jobs, remote objects, and storage-channel settings always identify both.

### TransferTask and TransferPart

A `TransferTask` is one logical-file-level upload or download. It records direction, priority, state, requested source/destination, totals, progress, retry policy state, account, timestamps, and structured failure category. A `TransferPart` records per-application-part state, attempts, verified byte counts, remote association, and durable checkpoint data.

Allowed task states are centrally defined. The current Core foundation includes `Queued`, `Running`, `Paused`, `WaitingRetry`, `Verifying`, `Completed`, `Failed`, and `Cancelled`; network/FloodWait detail is structured scheduler/error data unless a later synchronized state-model change promotes it. Completed is terminal. The exact transition table is in `TRANSFER_ENGINE.md`.

SQLite now stores task state, totals, retry metadata, locale-neutral failure codes, and ordered per-part checkpoints transactionally. Repository validation rejects non-contiguous indices, offset gaps, mismatched totals/progress, malformed checkpoint version/data pairs, and a completed task whose parts are not fully verified. This is durable state storage, not a transfer worker or proof of remote reconciliation.

### IndexJob and IndexRange

An `IndexJob` describes one requested scan/synchronization with account/chat, content policy, requested temporal scope, progress/checkpoint cursor, state, counters, and timing. An `IndexRange` records actual historical coverage rather than one last-message marker:

```text
account_id + chat_id
lower bound (time/message ordering key)
upper bound (time/message ordering key)
coverage state: partial or complete
content-policy fingerprint
checkpoint/evidence
```

Ranges may be non-contiguous. Only ranges with compatible content policy can merge. `INDEX_ENGINE.md` defines coverage semantics.

The current SQLite rows persist account/chat scope, inclusive message-ID bounds, partial/complete coverage, checkpoint, counters, policy version/fingerprint, scan generation, and update time. Storage rejects overlapping ranges for the same scope and policy and can commit file upserts, job progress, and range evidence atomically. Date/source-order bounds, range compaction, and scanner-produced evidence beyond these fields remain target work.

### Collections

A manual collection uses `CollectionItem` rows linking collections to logical files. A smart collection stores a versioned, locale-neutral rule AST over facets such as channel, type, size, date, and extension. It evaluates to logical files; it never contains multipart pieces.

### Vault and encryption metadata

`VaultMetadata` stores non-secret configuration and wrapped key material. It may reference password and recovery wrapping records, algorithm/format IDs, KDF parameters, and credential-store bindings. Raw passwords, unwrapped Vault Master Keys, File Keys, and derived KEKs must not be stored as ordinary database fields or logged.

## SQLite implementation and remaining areas

The implemented pre-release schema currently contains:

```text
accounts, chats
logical_files, logical_files_fts
index_jobs, index_ranges
transfer_tasks, transfer_parts
collections, collection_items
settings, id_allocators
```

Five ordered migrations create this schema, configure external-content FTS5 triggers, add checkpoint/index tables, add tagged local paths and ID allocators, and add Telegram remote-object identities plus per-source scan cursors. Empty-to-latest and every pre-latest-to-latest path are tested with data preservation. Foreign keys, strict tables, checks, uniqueness constraints, prepared statements, and explicit transactions enforce practical invariants. The connection enables foreign keys, WAL for file-backed databases, a busy timeout, and an untrusted schema.

Still absent are tables/repositories for Vault `file_parts`, `packages`, `manifests`, encryption profiles, and Vault metadata. Native Telegram documents now use `remote_objects` keyed by account/chat/message with monotonic revision checks and an opaque bounded transport key. Smart-collection rule payloads are currently versioned inline on the collection rather than represented by a separately interpreted rule repository. Because the product and recovery formats have not shipped, current table names and columns remain pre-release and are not yet a public compatibility promise.

## Relationships and deletion

- Deleting a local index record is not the same as deleting a remote Telegram object.
- Remote deletion or disappearance changes availability state and may leave an auditable/tombstone record according to sync policy.
- Removing a logical file from a collection never deletes the file or its remote parts.
- Package cleanup is explicit and must account for manifest/part reachability; cascading remote deletion is never an accidental database cascade.
- Account removal must define whether local indexed metadata, sessions, transfer checkpoints, and remote content are retained or removed.

Destructive behavior requires an explicit product flow and recoverability review.

## Search and pagination projections

Search projections return stable domain/view DTOs containing a keyset cursor. Ordering always includes a deterministic unique tie-breaker, commonly `(primary_sort_value, LogicalFileId)`. Deep `OFFSET` is not the million-record strategy. FTS indexes normalized searchable copies while preserving original Unicode metadata.

Search facets include account/channel, media type, date, size, extension, local/remote state, encryption, multipart, and verification. Filters are structured query inputs, not concatenated raw SQL.

The SQLite adapter implements these structured filters, phrase-based FTS5 over name/path/caption, and modified-time-descending keyset pages with `LogicalFileId` as the unique tie-breaker. Its versioned opaque cursor is bound to the query/facet fingerprint and is rejected after a query change. Tests cover tied traversal without duplicates, FTS insert/update/delete triggers, Chinese and Japanese content, case-insensitive extension filtering, and adversarial query text. Other sort orders and million-record performance remain unimplemented/unmeasured.

## Required invariants

- Logical-file size equals the sum of multipart plaintext lengths.
- Part indices are contiguous from zero; ranges have no overlap or gap.
- Each Vault package has exactly one authoritative manifest per generation.
- A `Completed` transfer has a verified manifest/object set and all required verified parts.
- A remote object has complete multi-account identity.
- Index coverage never claims more content/policy than was durably scanned.
- Smart collection rules and statuses are locale-neutral.
- Original user/source Unicode text is preserved exactly enough for display/recovery.
- Secret key bytes do not appear in ordinary persisted metadata, logs, or `Debug`.

These invariants require domain tests, database constraints/tests, and manifest validation; documentation alone is insufficient.
