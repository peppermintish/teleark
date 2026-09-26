# TeleArk architecture

TeleArk presents Telegram content as logical files. `LogicalFile` is the domain
object; messages, encrypted containers and MTProto requests are adapter details.
The [status page](IMPLEMENTATION_STATUS.md) distinguishes connected desktop behavior
from engine foundations and unfinished qualification.

## Boundaries

```text
GPUI Kit frontend
  -> frontend-neutral Core APIs and Runtime composition
     -> Storage / Telegram / Crypto / Index / Transfer ports and adapters
```

| Crate | Owns | Excluded responsibilities |
| --- | --- | --- |
| `teleark-core` | Domain IDs, invariants, commands, errors, events and ports | GPUI, SQL and grammers implementation |
| `teleark-runtime` | Desktop composition and retained background owners | GPUI |
| `teleark-storage` | SQLite, migrations, transactions, FTS5 and cursors | GPUI and grammers |
| `teleark-telegram` | grammers/MTProto, sessions, metadata and byte transport | GPUI and application SQL |
| `teleark-crypto` | Explicit codecs, keys, AEAD and integrity verification | GPUI, Telegram and SQLite |
| `teleark-index` | Cooperative scan coordination and coverage evidence | Concrete SQL and grammers |
| `teleark-transfer` | Scheduling, verification, retries and checkpoints | Concrete SQL and grammers |
| `teleark-i18n` | Fluent catalogs, negotiation, formatting and error messages | GPUI |
| `teleark-gui` | Windows, navigation, input, focus, accessibility and projections | Crypto, SQL, sessions and checkpoint implementation |

GUI calls retained services through frontend-neutral APIs. SQL belongs in Storage;
Telegram owns session and grammers details. Generic Index and Transfer engines are
separate from the connected desktop workflows and cannot alone prove their behavior.

## Product projections

Channel browsing projects an account-scoped local catalog updated by Telegram
pushes and history/difference recovery. Raw Files exposes physical objects; Files
authenticates managed manifests and presents complete logical files. Local and
Remote Library views have distinct sources and filters.

The frontend consumes revisioned snapshots and deltas, materializing visible rows.
Idle renders do not rebuild full collections or query unchanged database pages.
Bounded histories disclose omissions, while recoverable work retains durable state.
[ADR 0023](adr/0023-native-history-and-recovery-budgets.md) defines native history and
replay budgets; [Transfer](TRANSFER_ENGINE.md) defines progress and diagnostics.

## Account and storage identity

Restored sessions require explicit account entry. QR login reacts to Telegram
updates and token expiry; phone/code/2FA remains available. Catalog loading has a
separate owner, so a failed directory read cannot invalidate authentication or
publish a partial roster. Account changes invalidate old request generations.

Managed-channel bindings are account-scoped caches of verified remote identity.
Discovery requires one eligible owned private channel and cross-checks its
versioned identity, description pointer and pinned record. Ambiguity or damaged
identity blocks setup. Confirmed loss can trigger safe replacement under
[ADR 0042](adr/0042-unavailable-channel-replacement.md); it cannot restore deleted
remote files. Batch upload performs complete uniqueness discovery once, then
revalidates each file's target before plaintext access.

The desktop selects one key that authenticates the channel's proof/manifests.
Fresh setup persists its pending marker and key before publishing and verifying a
remote proof. Import or credential-backend changes trigger selection again. Files
remains unavailable if no single key authenticates the discovered files; Raw Files
and retained credentials remain intact. [ADR 0057](adr/0057-single-current-channel-key.md)
supersedes the former simultaneous historical-key desktop policy.

macOS credentials default to noninteractive system Keychain access; Windows/Linux
use private SQLite. Backend migration verifies copies before committing a new
selection and preserves recoverable sources on failure. The optional application
PIN gates interaction without stopping transfers or revoking their key leases.
See [Security](SECURITY.md) and [ADR 0053](adr/0053-selectable-system-credentials.md).

## Ownership and cancellation

Filesystem, database, network, crypto and credential work run off the UI thread.
Long operations have bounded queues, retained handles, cancellation and visible
phase transitions. Short snapshot locks never span network calls or long waits.
Callbacks validate account, connection and request generations before publishing.

Telegram has independent bounded read, transfer and mutable-control lanes.
Runtime owns channel synchronization, directory discovery, retries and recovery;
UI presentation clocks do not drive service queues. Four retained channel calls
and a separate directory slot isolate slow sources. Transient channel failures use
jittered backoff up to 60 seconds; server FloodWait deadlines remain authoritative.
Managed catalog-change signals survive bounded delta eviction.

Native Stop records the selected set's durable intent before waiting for writers
or filesystem cleanup. Independent cleanup ownership prevents a blocked path from
stalling unrelated downloads. Encrypted controls likewise persist generation-fenced
intent before signalling execution. Destructors do not join slow workers on UI or
network reactors. [Recovery](VAULT_TRANSFER_RECOVERY.md) specifies restart and
partial-file ownership; [ADR 0049](adr/0049-batch-download-stop-and-selection.md)
defines native batch stopping.

Account switching, proxy changes and Quit/Close share a visible transfer-drain
gate. Application locking leaves background owners and cached projections running.
Cryptographic session revocation still rejects new admissions and stale callbacks;
existing task leases retain their explicitly granted authority. See
[ADR 0026](adr/0026-session-unlock-and-task-key-leases.md) and
[ADR 0037](adr/0037-application-pin-and-transfer-drain.md).

## Files, durability and recovery

A native file maps to one remote object. An encrypted logical file maps to an
authenticated manifest and ordered containers. New payloads use codec 2; readers
retain codec 1 geometry. The encoded-container ceiling is 1.9 GiB and a full encoded
crypto frame is 512 KiB including framing and authentication. The current Telegram
adapter fragments uploads into 128 KiB RPC requests; frame and request sizes are
distinct. [Crypto](CRYPTO_FORMAT.md) and [Transfer](TRANSFER_ENGINE.md) own the details.

Uploads inspect the source with bounded buffers, then stream encryption without
creating ciphertext payload spools. Downloads authenticate frames in memory and
write only verified plaintext to a private partial. Whole-container and whole-file
verification precede atomic non-overwriting publication. Native downloads enforce
length but provide no cryptographic content authentication.

Recovery metadata and authenticated receipts persist separately from payloads.
Restart can reuse verified published containers. Lost unpublished ciphertext gets
fresh encryption identities; sealed manifest outboxes replay their exact bytes.
Legacy spools remain read-only compatibility inputs. Portable pending descriptors
allow eligible same-source/account/channel recovery, but only a final authenticated
manifest represents a completed file. See [ADR 0041](adr/0041-memory-streaming-and-portable-upload-recovery.md).

Transfer tuning defaults to two files per direction, sixteen in-flight parts per
file and eight shared connections per direction. Native and encrypted downloads
share admission and bandwidth limits. Values describe configured limits, not
measured physical concurrency or throughput.

Local availability is independent of historical completion. A Runtime observer
watches retained destinations, responds to filesystem/completion events and uses
bounded fallback checks after startup, wake, missing coverage or quiet periods.
Same-size metadata does not authenticate content. [ADR 0056](adr/0056-event-driven-local-availability.md)
supersedes regular polling; [ADR 0051](adr/0051-retry-deleted-batch-downloads.md)
defines explicit retry of missing batch outputs.

SQLite reads/writes schema 23 and upgrades supported versions 0–22 in ordered,
verified transactions. Startup paints acknowledgment before background migration
and retains its phases and failures. Persistent codecs are versioned independently
of releases, retain compatible readers and preserve unsupported newer bytes.
[Data model](DATA_MODEL.md) and [status](IMPLEMENTATION_STATUS.md#compatibility)
record the current matrix. General cross-format conversion and external backup
coordination remain unfinished.

## Desktop presentation

`theme.rs`, `components.rs` and `layout.rs` centralize palette and geometry. Data
rows use 24 points, primary text 12 points and batch headers 34 points. Lists and
inspectors have independent scroll owners; truncated values remain accessible.
Small windows retain primary actions. The native minimum is 900×600, adjusted
only when the display's work area cannot accommodate it.

Modals trap focus and restore it on dismissal. Sign-out requires an explicit
choice. QR images are black on white. The synchronization footer is display-only;
Network settings opens its activity inspector. Vault key and cleanup activity have
their own actionable status entries. About's English release record is checked
against `CHANGELOG.md`.

Visual review uses synthetic English/light previews at 900×600 and actual native
full-screen. [Development](DEVELOPMENT.md#isolated-ui-review) lists fixtures and
interaction checks; [access design](ACCESS_UI_DESIGN.md) records layout rationale.

## Localization service

`teleark-i18n` owns the locale registry, semantic messages and display formatting.
English, Simplified Chinese and Japanese have complete key parity; seven other
catalogs use explicit English fallback. User content remains literal and stored
values remain locale-neutral. Native OS locale discovery is unfinished.

Use centralized formatters for dates, counts, percentages, durations and byte
quantities. All sizes/rates use IEC units from B through EiB and their `/s` forms.
Unknown rate/ETA is distinct from zero. [Localization](I18N.md) defines catalog
validation and the non-visual checks for every registered language.

## Technical references

The [documentation guide](README.md) indexes the current contracts, design records
and dated validation evidence. [AGENTS.md](../AGENTS.md) defines contributor rules;
[Security](SECURITY.md) defines trust boundaries and redaction requirements.
Accepted ADRs preserve original decisions and identify subsequent changes.
