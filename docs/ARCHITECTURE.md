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
- **Channels** browse ordinary Telegram sources. Their account/chat/message identities remain attached to all file and download requests.
- **Local Library**, a direct navigation destination, is the persistent catalog and FTS projection. Import records local metadata; browsing/indexing also adds remote file metadata. These catalog records are distinct from transfer history and do not prove an upload or download.
- **Transfers** is a fixed primary destination. Refreshing or scrolling channels cannot replace the route, selected transfer, filter or transfer scroll owner.
- Existing Saved Messages packages remain available through Settings → Key Vault → Advanced → legacy recovery. New uploads never use Saved Messages, and migration never moves or deletes remote objects.

The GPUI Kit 0.6.0 facade uses the matching gpui-pre 0.3.3 family. `application()` chooses the native platform and `init()` initializes the enabled layers. Base provides behavior/focus/accessibility; Component provides styled controls, segmented tabs, Sidebar and DataTable. Native/raw and managed/transfer lists virtualize visible rows. Presentation owners live under GUI `app/`; screen composition lives under `screens/`. Palette and geometry are centralized. See [UI contract](UI_GUIDELINES.md) and [ADR 0012](adr/0012-gpui-kit-and-private-storage-channel.md).

## Account and storage identity

Startup restores an existing Telegram session and displays its avatar/name with Log In and Switch Account. Entering the workspace refreshes that account's sources before native task scheduling becomes eligible. A new session uses phone/QR, code and optional 2FA. API configuration is an explicit login/settings action.

Private-channel binding is a versioned account-scoped SQLite setting. Discovery checks the exact first description line `teleark:storage:v1` plus current ownership and private broadcast metadata, independently of the title. A valid saved ID survives rename; an unavailable saved binding is never silently substituted. Multiple candidates require selection. Incomplete/truncated discovery fails closed. Creation discovers first and never blindly retries its non-idempotent RPC. [Preferences format](PREFERENCES_FORMAT.md) specifies the encodings and limits.

Schema 9 records the account on each new native download. On the first configured connection, the runtime atomically resolves legacy NULL ownership using the actual restored session, or leaves it unknown permanently if unauthorized. New logins cannot claim unknown old work. Scheduling/resume/retry and actual Telegram execution validate the expected account. Before switching, native tasks pause and their retained workers release requests/partials; running Vault work must finish. Local completed files and history are retained. See [data model](DATA_MODEL.md).

## Ownership and cancellation

Storage, Telegram and Vault operations run off the UI thread behind bounded request queues and retained lifecycle handles. The Vault owner alone holds the unlocked Master Key. GUI callbacks use account/source generations to reject stale results. Long network operations do not hold snapshot/state locks.

Interactive raw scans own cancellation and bounded transport pages. Manifest scans have an independent cancellation token/loading generation, so leaving a source, changing projection, locking or closing the app cannot strand a shared Vault operation state. Cancellation interrupts the Telegram future and stops the recovery loop; it is not counted as an invalid manifest. Channel and transfer refresh owners cannot navigate as a side effect.

A modal keeps the requested unlock intent (browse, upload or download), takes focus and resumes it only after successful unlock/recovery acknowledgement. Changing pages does not lock the Vault. With `lock_vault_when_hidden`, inactive windows clear secret inputs and request locking; active native prompts are exempt. Already running encrypted work may finish with its retained key. Sleep/logout guarantees beyond window activation still need platform verification.

The primary sidebar collapses to a 64-point icon rail or expands to 184 points. Channels have their own contextual list; inspectors own bounded, occluding scroll viewports. A retained local-output observer reads account-scoped pages and performs filesystem metadata work through Runtime off the UI thread. The bottom status bar queries download-volume space independently of directory usage scans.

## Files, durability and recovery

A native file resolves to one remote object; a Vault package resolves to one authenticated manifest and ordered application parts. Crypto frames and MTProto request units are different layers. The compatibility target remains 1900 MiB application parts; the current desktop encrypted adapter uses a conservative 60 MiB plaintext ceiling and 8 MiB frames. These limits are recorded explicitly rather than inferred from account tier.

Completed Vault recovery authority is the remote manifest and parts plus separately protected unlock material. SQLite is a catalog/cache/checkpoint store, not the sole authority. Publish the manifest only after parts verify; finalize a download only after authentication, whole-file verification and atomic non-overwriting publication. Incomplete uploads may require surviving local state and can leave orphan ciphertext.

Schema 10 adds a local Vault output inventory; native completed history is projected directly. It records local availability hints, never remote recovery authority. Bounded multi-file upload plans run sequentially through the existing Vault owner. [ADR 0013](adr/0013-local-output-inventory-and-upload-batches.md) records the decisions.

All durable encodings are explicit: SQLite migrations, crypto/manifest codecs, recovery bundles, native completion bitmaps and session-log schemas. Internal Rust layout never defines bytes. The [crypto](CRYPTO_FORMAT.md) and [manifest](MANIFEST_FORMAT.md) formats remain provisional; independent review, longer fuzz evidence and production recovery validation are release gates.

## Contracts to consult

[Data model](DATA_MODEL.md) · [Index](INDEX_ENGINE.md) · [Transfer and diagnostics](TRANSFER_ENGINE.md) · [Preferences](PREFERENCES_FORMAT.md) · [Security and audit gates](SECURITY.md) · [i18n](I18N.md) · [Development](DEVELOPMENT.md).

Accepted ADRs preserve historical decisions. Supersede them explicitly when policy changes; do not rewrite their original context to look current.
