# TeleArk Architecture

Status: accepted target architecture with a persistent local catalog, persisted Telegram API ID onboarding, desktop-connected Telegram QR/code login, source discovery, bounded document browsing/indexing, and a retained native-download worker. The encrypted transfer/recovery composition exists below the GUI, while its desktop Vault/upload workflow remains incomplete. See `IMPLEMENTATION_STATUS.md` for exact current capability.

## Purpose

TeleArk turns Telegram-hosted media into a file-centric library. The enduring user and domain abstraction is `LogicalFile`, not a Telegram message, remote document, MTProto upload unit, or application multipart object.

The architecture prioritizes recoverability, integrity, cryptographic safety, legal cleanliness, testability, and frontend independence. A local SQLite database accelerates the product but is not the recovery authority for encrypted Vault packages.

## System shape

```text
                         Frontends
                 +----------+----------+
                 |                     |
             GPUI GUI              Future CLI
                 |                     |
                 +----------+----------+
                            |
              frontend-neutral application API
                            |
   +------------------------+-------------------------+
   |                        |                         |
Index services       Transfer services        Vault services
   |                        |                         |
   +------------- domain models/events/ports --------+
                            |
             +--------------+--------------+
             |                             |
       SQLite adapter                Telegram adapter
                                             |
                                          grammers
```

Dependency arrows point inward toward project-owned contracts. Infrastructure adapters implement ports; domain and application code do not depend on infrastructure or presentation types.

## Layer responsibilities

### Domain and application Core

Core owns domain IDs, models, invariants, commands, state transitions, structured errors, frontend-neutral events, and application-service orchestration. It may define narrow ports for storage, Telegram transport, clocks, randomness, credential stores, and filesystems when substitutability or deterministic tests justify them.

Core does not know GPUI, SQL, `grammers` models, OS keychain APIs, localized prose, or widget callbacks. Public actions describe business intent, for example `search_files`, `start_upload`, `pause_transfer`, `start_index_job`, `unlock_vault`, and `recover_storage`.

### GUI

The GPUI frontend owns windows, layout, navigation, input, focus, dialogs, virtualized rows, presentation state, accessibility, localized rendering, and conversion from domain DTOs into view models. It sends commands to application services and consumes snapshots/events. It must not directly query SQLite, call Telegram, hash/encrypt files, or persist checkpoints.

### Storage adapter

The SQLite adapter owns migrations, repositories, FTS5, transaction boundaries, query plans, keyset pagination, index-range persistence, and transfer checkpoints. SQL remains centralized here. WAL, foreign keys, busy timeout, batch sizes, and indexes are selected deliberately and verified with tests/profile data.

### Telegram adapter

The Telegram adapter contains all `grammers` and MTProto types. It owns authentication/session integration, account and chat discovery, history/update access, media upload/download, FloodWait mapping, and remote locator resolution. It maps results into TeleArk-owned DTOs and structured errors. Its session cache uses a bounded, explicitly versioned file replaced atomically with owner-only permissions; session secrets never enter the Library database or GUI diagnostics.

### Crypto and Vault adapter

The Crypto subsystem owns versioned codecs, envelope encryption, AES-256-GCM frames, Argon2id password derivation, key wrapping, secure randomness, BLAKE3 hashing, redacted secret types, and validation. It has no GUI, Telegram, or database dependency. OS credential storage is a replaceable platform adapter and is not part of the durable format.

### Index Engine

The Index Engine coordinates partial and incremental scans through a history-source port, normalizes Telegram metadata, performs bounded batched writes, and maintains non-contiguous `IndexRange` coverage. Search normally reads local SQLite/FTS5 rather than remote history.

### Transfer Engine

One Transfer Engine schedules upload and download tasks. It owns bounded concurrency, priority, retry/backoff, FloodWait handling, pause/resume/cancel, progress aggregation, verification, checkpoints, and restart reconciliation. A task represents one logical file; a transfer part represents one application part.

## Workspace boundaries

The current workspace is:

```text
teleark-core          domain and application services
teleark-storage       SQLite adapter and migrations
teleark-telegram      grammers adapter
teleark-crypto        durable crypto/manifest codecs and crypto operations
teleark-index         bounded historical indexing coordinator
teleark-transfer      deterministic transfer scheduler and engine
teleark-runtime       desktop adapter-worker composition
teleark-i18n          locale negotiation, resources, formatters
teleark-gui           GPUI application
```

`teleark-cli` and a shared `teleark-test-support` crate remain future additions;
the Index and Transfer crates currently keep their deterministic fake ports in
their own test modules.

Allowed dependencies are deliberately one-way:

| Crate area | May depend on | Must not depend on |
| --- | --- | --- |
| Core | domain-focused permissive libraries | GUI, GPUI, SQLite implementation, grammers |
| Storage | Core contracts, SQLite crate | GUI, grammers |
| Telegram | Core contracts, grammers | GUI, SQLite implementation |
| Crypto | Core/domain contracts as needed, crypto crates | GUI, grammers, SQLite |
| Index | Core contracts and project-owned history/repository ports | GUI, grammers, SQLite implementation |
| Transfer | Core contracts and project-owned I/O/checkpoint ports | GUI, grammers, SQLite implementation |
| Runtime | Core contracts and concrete adapters | GPUI types |
| i18n | locale/resource libraries, structured error mapping | GPUI-specific types |
| GUI | Core API, i18n, GPUI/gpui-component | raw SQL, grammers types |
| CLI | Core API, i18n | GPUI |

Circular dependencies are a boundary problem, not a reason to introduce global mutable state or copied models.

## Commands, snapshots, and events

Frontends issue typed commands to application services and receive typed results/snapshots. Long-running work emits frontend-neutral events such as:

```text
IndexUpdated
TransferUpdated
FileAdded
FileChanged
AccountUpdated
VaultStateChanged
```

Events contain stable IDs and locale-neutral data, never widget types or translated status strings. Delivery uses bounded channels or another mechanism with explicit backpressure. Low-level progress is aggregated before emission (initial target: roughly 100–250 ms, subject to profiling).

GPUI state changes occur on the GUI's supported update path. Tokio tasks never mutate GPUI entities from arbitrary worker threads.

## Logical-file and Vault model

```text
Native Telegram media
LogicalFile -> RemoteObject

Encrypted multipart package
LogicalFile -> Package -> Manifest
                       -> FilePart 0 -> RemoteObject
                       -> FilePart 1 -> RemoteObject
                       -> ...
```

Collections, search results, and transfer history reference `LogicalFile`. Application parts are compatibility-oriented and account-tier-independent; the initial target is 1900 MiB. Each is streamed as smaller authenticated crypto frames. MTProto upload parts remain the responsibility of `grammers`/transport and are never confused with application parts.

## Persistence and recovery authority

SQLite is a local index, cache, checkpoint store, settings store, and search engine. It is not the sole authority for encrypted packages. A versioned manifest stored with opaque remote parts must be sufficient, together with valid account access and the required key, to rediscover and reconstruct the package after local database loss.

Desktop preferences cross the GUI/runtime boundary as the typed
`DesktopPreferences` value and are stored atomically under the explicit
`preferences.v1.*` SQLite namespace. The GUI never reads or writes those rows
directly. One optional managed-files root controls a runtime-owned filesystem
layout with `Downloads`, `Cache`, and `Logs` children. The runtime creates the
layout and
allocates non-overwriting download destinations; the GUI never prompts for or
constructs individual download paths. SQLite and the Telegram session remain
in the platform application-data directory so an active database or session is
never moved by a preference change. Encoding, defaults, validation, and
compatibility behavior are specified in
[PREFERENCES_FORMAT.md](PREFERENCES_FORMAT.md).

Process diagnostics use structured `tracing` events with an explicitly safe
field allowlist. A bounded non-blocking writer emits daily JSONL files under
the managed `Logs` directory; storage, Telegram, and transfer owners never
block on log I/O. Frontend-neutral native-download snapshots separately expose
safe timing, verification, event, and failure-class data for localized UI.
Diagnostic output is not a durable compatibility format. See
[DIAGNOSTICS.md](DIAGNOSTICS.md).

Telegram-native desktop downloads persist separately in schema-v7
`native_download_batches` and `native_download_tasks` tables because they do
not yet represent encrypted multipart packages. The runtime owns bounded
per-channel time/kind scans, atomic batch creation, the bounded queue,
cooperative control, chunk-level progress aggregation, resumable private
partials, source message metadata, and restoration. The GUI consumes snapshots,
aggregates a batch into an expandable presentation row, and never reads those
tables directly.

The Telegram application API ID and API Hash are resolved by the
frontend-neutral runtime worker. An atomically managed personal pair persists
in SQLite and takes precedence over an optional distributor pair compiled into
an official TeleArk build. Frontends can inspect only the numeric API ID and a
`User`/`Distribution` source marker; the runtime loads the hash directly for
authentication and redacts it from debug output. Telegram authorization
sessions remain owned by the Telegram adapter's separately protected session
cache. A source build without either complete pair fails closed. TeleArk never
embeds or falls back to Telegram Desktop credentials: distributor builds must
use credentials registered for their own TeleArk application.

Cross-system operations cannot be atomic. If a remote upload succeeds and local checkpoint persistence fails, restart reconciliation uses package ID, part index, deterministic/recoverable opaque naming, and remote discovery to repair state without blind duplicate upload.

Persistent representations use explicit versioned codecs and bounds checks. They do not serialize arbitrary Rust memory layout. See `CRYPTO_FORMAT.md` and `MANIFEST_FORMAT.md`; both formats are currently provisional and unshipped.

## Major data flows

### Indexing

```text
Telegram history source
 -> normalized records
 -> bounded batches
 -> SQLite + FTS5 + IndexRanges
 -> keyset-paginated Core query
 -> virtualized frontend rows
```

### Upload

```text
source identity + LogicalFile + fresh package/file key
 -> bounded range reader
 -> plaintext hashes
 -> authenticated crypto frames
 -> Telegram upload
 -> remote verification
 -> durable checkpoint/reconciliation record
 -> encrypted versioned manifest
 -> manifest upload and verification
 -> Completed
```

### Download

```text
manifest resolution and key unwrap
 -> destination.partial
 -> bounded part download
 -> ciphertext verification
 -> authenticated frame decryption
 -> direct writes at final offsets
 -> plaintext/whole-file verification
 -> flush
 -> atomic rename
```

The final destination name never appears as a completed file before all mandatory authentication and integrity checks pass.

## Global invariants

- `LogicalFile.size_bytes == sum(FilePart.plaintext_size_bytes)` for multipart files.
- File-part indices are contiguous from zero and ranges have no gaps or overlap.
- A completed transfer has every required part and the manifest verified.
- A package has one authoritative manifest for a given format version/generation.
- AES-GCM nonces never repeat under one content key.
- User/source text remains original Unicode and is never localized in storage.
- Persisted status values and Core errors are locale-neutral.
- GPUI and `grammers` types do not cross their adapter boundaries.
- Secrets are never logged and do not reveal raw bytes through `Debug`.

## Architectural change process

Changes to dependency direction, the Logical File abstraction, storage authority, crypto/manifest formats, part-size policy, or licensing/clean-room policy require an ADR. Once a durable format ships, changes must preserve old read compatibility or introduce an explicit migration/new version with fixtures. Update implementation status whenever code lags behind this target design.
