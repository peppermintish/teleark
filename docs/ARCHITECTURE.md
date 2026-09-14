# TeleArk architecture

TeleArk presents Telegram content as files. `LogicalFile` is the enduring domain abstraction; messages, encrypted pieces and MTProto units are adapter details. [Implementation status](IMPLEMENTATION_STATUS.md) distinguishes connected product behavior from tested engine foundations and unfinished work.

## Boundaries

```text
GPUI Kit frontend / future CLI
  -> frontend-neutral Core APIs and Runtime composition
     -> Storage / Telegram / Crypto / Index / Transfer ports and adapters
```

| Crate | Owns | Excluded dependencies |
| --- | --- | --- |
| `teleark-core` | Domain IDs, invariants, commands, errors, events, service ports | GPUI, SQL implementation, grammers |
| `teleark-runtime` | Desktop service composition and retained adapter workers | GPUI |
| `teleark-storage` | SQLite repositories, migrations, transactions, FTS5, cursors | GPUI, grammers |
| `teleark-telegram` | grammers/MTProto, sessions, remote metadata and byte transport | GPUI, SQLite implementation |
| `teleark-crypto` | Explicit crypto/manifest codecs, keys, AEAD and validation | GPUI, Telegram, SQLite |
| `teleark-index` | Cooperative scan coordinator and coverage evidence | GPUI, concrete SQL/grammers |
| `teleark-transfer` | Cooperative scheduler, verification, retries and checkpoints | GPUI, concrete SQL/grammers |
| `teleark-i18n` | Fluent resources, negotiation, formatting and error messages | GPUI |
| `teleark-gui` | Windows, navigation, inputs, focus, accessibility and DTO presentation | SQL, grammers, crypto/session/checkpoint implementation |

Dependencies point toward project-owned contracts. Traits belong at meaningful substitution/testing boundaries, not around every struct. The GUI receives structured Runtime/Core snapshots and errors; it never decides outcomes by parsing translated prose. A future CLI reuses these APIs; no CLI is currently implemented.

## Product projections

- **TeleArk** is a dedicated private broadcast channel owned by the active account. Its Files view shows authenticated manifests as complete logical files; Raw Files exposes original Telegram documents and candidate encrypted objects.
- **Channels** read their local projection first; selecting one promotes background work and maintains the protocol-required viewed-channel subscription. Their account/chat/message identities remain attached to all file and download requests.
- **Library** has separate Local files and Remote files projections. Local files pages account-scoped native/Vault completed outputs and imported originals, with cancellable Runtime filesystem observations that exclude missing/inaccessible files and use current size/date. Local copy keys stay distinct from catalog LogicalFile IDs. Remote files is the active account’s persistent catalog/FTS projection populated by browsing/indexing; those records do not prove a download. File type filtering is independent of source tabs.
- **Transfers** is a fixed primary destination. Refreshing or scrolling channels cannot replace the route, selected transfer, filter or transfer scroll owner.
- Existing Saved Messages packages remain available through Settings → Key Vault → Advanced → legacy recovery. New uploads never use Saved Messages, and migration never moves or deletes remote objects.

The GPUI Kit 0.6.0 facade uses the matching gpui-pre 0.3.3 family. `application()` chooses the native platform and `init()` initializes the enabled layers. Base provides behavior/focus/accessibility; Component provides styled controls, segmented tabs, Sidebar and DataTable. Native/raw and managed/transfer lists virtualize visible rows. Presentation owners live under GUI `app/`; screen composition lives under `screens/`. Palette and geometry are centralized. [ADR 0012](adr/0012-gpui-kit-and-private-storage-channel.md) records the dependency and design decision.

## Account and storage identity

Startup restores an existing Telegram session and displays its avatar/name with Log In and Switch Account. Entering the workspace refreshes that account's sources before native task scheduling becomes eligible. A new session automatically starts QR login, with a secondary phone method, code and optional 2FA. Account switching requires an explicit sign-out confirmation before pausing workers and signing out. API configuration is an explicit login/settings action.

Private-channel bindings are account-scoped SQLite caches. Desktop setup and Vault preflight derive identity and uniqueness from complete remote discovery, ownership/private metadata, description pointer and a pinned account/channel-bound identity record. One candidate is required; conflicting or damaged evidence stops setup and writes. Recognizable legacy channels can be upgraded from remote evidence alone. [ADR 0015](adr/0015-remote-authoritative-storage-identity.md) defines the cross-device policy and its non-atomic creation limits; [Preferences format](PREFERENCES_FORMAT.md) specifies remote codecs. No local creation-intent setting is used. Upload queues are visible before network work. Complete uniqueness discovery runs once per batch with at most four concurrent candidate inspections; each file refetches the target identity before plaintext access. [ADR 0016](adr/0016-bounded-upload-preflight.md) defines scheduling and its consistency window.

Schema 9 records the account on each new native download. On the first configured connection, the runtime atomically resolves legacy NULL ownership using the actual restored session, or leaves it unknown permanently if unauthorized. New logins cannot claim unknown old work. Scheduling/resume/retry and actual Telegram execution validate the expected account. Before switching, native tasks pause and their retained workers release requests/partials; running Vault work must finish. Local completed files and history are retained. See [data model](DATA_MODEL.md).

## Ownership and cancellation

Storage, Telegram and Vault operations run off the UI thread behind bounded request queues and retained lifecycle handles. The Vault owner alone holds the unlocked Master Key. GUI callbacks use account/source generations to reject stale results. Long network operations do not hold snapshot/state locks.

Channel and managed private-channel synchronization share one retained account owner in Runtime. Telegram contributes bounded metadata pushes and official difference/history APIs; contiguous PTS updates commit directly and gaps recover from durable PTS. Storage returns actual deltas plus coherent local baselines. GUI awaits events and patches only the relevant rows; neither a global revision nor a fixed UI poll drives database reads. Selected/private subscription deadlines, retries and passive metadata reconciliation retain their protocol roles. Private edit/delete pushes immediately publish a pending warning; confirmed observations/acknowledgements persist through schema 12. Private Raw Files uses the shared projection; Files authenticates only new/invalidated local manifest candidates and reuses bounded derived metadata. Legacy Saved Messages remains an explicit separate recovery reader. Manifest queues, reads, authentication and terminal outcomes report through the same event feed and retain cancellation/account/generation guards. [ADR 0019](adr/0019-event-driven-channel-projections.md) supersedes the earlier polling/private-reader behavior in [ADR 0018](adr/0018-local-first-channel-sync.md).

Automatic managed-manifest synchronization retains a background caller outside the Vault workers. Each attempt acquires a session-fenced key lease; transient failures retry after 2–60 seconds without requiring a new channel event. Server-requested waits are ephemeral typed error metadata and survive the Telegram/object/Vault adapters without shortening. The account event feed reports retries and terminal outcomes. A scan result cannot replace the key-operation result shown after unlock. Cancellation, account replacement and key/session changes stop retries and reject late projections; backoff never occupies the key, transfer or scan worker. Existing on-disk formats are unchanged.

File-key setup and recovery keep their requested intent and only resume after successful key access. Application access is separately gated by an optional PIN across the whole window. Locking leaves all service owners, key sessions and cached projections running. Account switching, application Quit/Close and proxy application use one event-driven transfer drain gate. See [ADR 0037](adr/0037-application-pin-and-transfer-drain.md), which supersedes the earlier application-facing lock behavior.

The primary sidebar collapses to a 64-point icon rail or expands to 184 points. Channels have their own contextual list; inspectors own bounded, occluding scroll viewports. A retained local-output observer reads account-scoped pages and performs filesystem metadata work through Runtime off the UI thread. The status bar spans the bottom of the entire window and displays synchronization activity without click actions. Network settings opens the bounded, independently scrolling activity inspector without changing the current workspace. It also queries download-volume space independently of directory usage scans.

## Files, durability and recovery

A native file resolves to one remote object; a Vault package resolves to one authenticated manifest and ordered application parts. The desktop writer uses format v2: each full encrypted frame, including framing, occupies one 512 KiB MTProto upload part; the final part can be shorter. It streams bounded reusable buffers through 60 MiB plaintext containers. Readers preserve v1 and its historical frame geometry. These limits are explicit, independent of account tier.

Completed Vault recovery authority is the remote manifest and parts plus separately protected unlock material. SQLite is a catalog/cache/checkpoint store, not the sole authority. Publish the manifest only after parts verify; finalize a download only after authentication, whole-file verification and atomic non-overwriting publication. Incomplete uploads may require surviving local state and can leave orphan ciphertext.

Schema 10 adds a local Vault output inventory; native completed history is projected directly. It records local availability hints, never remote recovery authority. Bounded multi-file upload plans use a work-conserving pool, defaulting to three files and ten concurrent part RPCs per file. Shared directional connection pools and manually selected upload/download limits replace profile tuning. Native and Vault downloads share one admission gate. [ADR 0036](adr/0036-streaming-upload-and-manual-concurrency.md) supersedes the sequential execution decision in [ADR 0013](adr/0013-local-output-inventory-and-upload-batches.md). A sealed local spool supports immutable replay for 24 hours; restart retires cryptographic identities before new encryption. Normal upload completion uses the local seal and Telegram receipt without content read-back.

All durable encodings are explicit: SQLite migrations, crypto/manifest codecs, recovery bundles, native completion bitmaps and session-log schemas. Internal Rust layout never defines bytes. The [crypto](CRYPTO_FORMAT.md) and [manifest](MANIFEST_FORMAT.md) codecs define version-1 bytes. [ADR 0017](adr/0017-versioned-automatic-migrations.md) requires versioned compatibility and automatic upgrades: each release declares supported source/read/write versions, preserves recoverable originals and provides restartable migration with visible phases. Storage owns SQL migration, Crypto owns format conversion and Runtime coordinates bounded work/events; GUI presents status and gates only conflicting actions. Startup now paints before opening/migrating SQLite on background owners and retains detection/preparation/conversion/verification events with failure/retry guidance. Schema 13 adds search indexes transactionally and is verified before commit; existing schemas and codecs retain their meanings. The remaining cross-format conversion and external-backup coordinator is still implementation work.

## Desktop presentation

`theme.rs`, `components.rs` and `layout.rs` own palette, reusable controls and responsive geometry. The native window has a 900×600 minimum and fits oversized requests to the display. Lists retain flex-height viewports; inspectors have independent scroll handles and occluding hitboxes so wheel events cannot move the list underneath, including at scroll boundaries. Upload dialogs keep their title/actions visible around a scrolling body. Compact layouts retain name, size and state; other metadata stays in details. UI acceptance requirements, including background visibility and graphical diagnostics, live in [AGENTS.md](../AGENTS.md).

Modals trap focus and restore it on dismissal. Escape handles the modal before fullscreen; Return submits only its intended action. The sign-out modal requires an explicit Cancel or confirmation and ignores Escape/backdrop dismissal. QR images use black on white in both themes. Unsupported runtime controls remain unavailable with guidance; native history deletion never deletes user files. About's English release record equals `CHANGELOG.md` through a parity test.

Workspace shortcuts: ⌘1 Transfers, ⌘2 TeleArk, ⌘U Upload, ⌘F Search, ⌘R Refresh, ⌘, Settings, ⌃⌘F Fullscreen, ⌘M Minimize, ⌘Q Quit. Isolated fixtures and review commands are in [Development](DEVELOPMENT.md#isolated-ui-review).

All application data lists, channel/menu entries, selection lists, metadata rows and event summaries share a 24-point row, 12-point primary text, 11-point secondary text and a 16-point text line. Batch summaries use 34 points (about 1.4 rows). `theme.rs` is the sole source for these sizes; `components::list_row`, list buttons and summaries apply them. Raw DataTable rows use the same explicit size and zero vertical cell padding, keeping checkboxes and 22-point actions inside each row. File names, sizes and state stay in the same line, with full truncated summaries available through tooltips and detailed file/transfer views. Lists retain their existing virtualization, identities, independent scroll owners and event retention.

## Localization service

`teleark-i18n` owns Fluent resources at `crates/teleark-i18n/resources/{en-US,zh-CN,ja-JP}/main.ftl`, locale negotiation, formatting and structured-error mappings. The catalogs are the terminology reference and share keys and named variable sets. Messages use semantic kebab-case IDs, complete grammatical units and Fluent selectors/plurals; legacy dotted/underscore aliases remain supported. The wrapper resolves top-level message values; terms/attributes require added lookup and validation support. User content is passed as literal parameters.

Negotiation maps `en-*` to `en-US`, `zh-CN`/`zh-SG`/`zh-Hans-*` to `zh-CN`, and `ja-*` to `ja-JP`; unsupported or invalid tags, including unsupported Traditional Chinese, fall back to English. Tags use the locale parser. Settings changes apply live; explicit overrides persist, System Default removes the override, and a command-line locale wins for that launch. System discovery currently reads locale environment variables; native platform discovery is unfinished. Changing locale never rewrites remote channel descriptions or titles.

Central `format` functions render dates, counts, percentages, durations, speeds and sizes. Human file/storage values use SI (`kB`, `MB`, `GB`, `MB/s`); exact format/configuration sizes use IEC (`MiB`, `GiB`). Timestamps are UTC instants displayed in the OS local time zone. Protocol IDs, hashes, offsets and versions stay canonical ASCII. Unknown rate/ETA is distinct from zero. Catalog validation covers parsing, duplicate IDs, key/variable parity, fallback, negotiation, formatting and error mappings; static caller IDs and actual layout need separate checks.

## Technical references

[Data model](DATA_MODEL.md) · [Index](INDEX_ENGINE.md) · [Transfer and diagnostics](TRANSFER_ENGINE.md) · [Preferences](PREFERENCES_FORMAT.md) · [Crypto](CRYPTO_FORMAT.md) · [Manifest](MANIFEST_FORMAT.md) · [Security and migration safety](SECURITY.md) · [Build, checks and previews](DEVELOPMENT.md) · [Current capabilities and limitations](IMPLEMENTATION_STATUS.md).

Accepted ADRs preserve historical decisions and their original context.

Telegram RPC scheduling uses bounded independent read, transfer and mutable control lanes; login/logout drain older work before publishing replacement state. See [ADR 0021](adr/0021-bounded-telegram-request-lanes.md).

Transfer session-log callbacks stage bounded complete records for a single background file owner. Explicit gaps and a localized cumulative counter disclose diagnostic loss while task and recovery persistence remain independent. [ADR 0022](adr/0022-bounded-background-session-logs.md) records the ownership and compatibility contract.

Managed-directory preparation and destination filesystem probes execute on their existing background Runtime callers. Storage receives only settings/reservation SQL requests, so a blocked mount does not occupy its database actor. Preference updates prepare the filesystem before committing settings; download name validation, historical reservations and atomic no-overwrite publication remain enforced. Telegram destination metadata uses asynchronous filesystem access.

Native transfer presentation has a shared admission/history budget and a separate replay budget. Recovery restores all recoverable tasks and their batch members before selecting recent terminal history; older oversized queues are preserved automatically. Cold historical redownload uses one account-validated database record. [ADR 0023](adr/0023-native-history-and-recovery-budgets.md) specifies limits, omissions and compatibility exceptions.

Catalog loading and authentication have separate GUI owners. A failed complete dialog read cannot invalidate an authenticated session or publish a partial roster. The transient `ApplicationErrorKind::Server` represents Telegram RPC 5xx; transport failures remain `Network`. The initial catalog has cancellable, bounded reads and retries with a shell-visible current-run timeline. Account changes invalidate its monotonic request generation. Native-download codecs preserve their historical `network` encoding for both categories; no stored bytes acquire a new meaning. Initial storage discovery is deferred until the catalog succeeds, and still independently verifies complete remote identity before creation.

Network ownership and application-wide fail-closed egress are defined in [ADR 0024](adr/0024-fail-closed-proxy-routing.md). All Telegram pools use the retained gateway; the framework HTTP client is blocked.

### Fixed channel resilience

[ADR 0025](adr/0025-fixed-channel-and-retained-key-epochs.md) separates fixed peer binding, repairable management metadata, per-file health and key epochs. Telegram validates remote privacy/ownership and implements peer-scoped maintenance. Storage owns schema-15 key retention, encrypted inventory and health observations. Runtime owns typed errors, bounded progress and an independent retained health checker; GUI only confirms, presents and cancels these operations.

### Session key admission and task leases

[ADR 0026](adr/0026-session-unlock-and-task-key-leases.md) separates session access from already admitted background work. The key, encrypted-transfer and scan lanes each have a bounded 16-command queue; admission returns a retained `VaultJob` without I/O. Cryptographic session revocation immediately clears session keys, preserves task leases and synchronization, and uses generations/revisions to reject stale unlocks and invalidate decrypted catalog caches. Explicitly submitted tasks keep their authorization through internal retries; new tasks require unlocking.

Synchronization presentation and navigation independence are specified in [ADR 0033](adr/0033-static-sync-inspector-and-navigation-independent-updates.md), superseding the live clock in ADR 0032 and viewport short polling. [ADR 0034](adr/0034-compact-channel-and-shell-status.md) defines the compact channel toolbar, concise shell status and revision-cached transfer rates.

Application PIN and synchronization presentation contracts are specified in [ADR 0037](adr/0037-application-pin-and-transfer-drain.md). Shell synchronization status is display-only; Settings → Network proxy opens its merged, revision-cached timeline.
