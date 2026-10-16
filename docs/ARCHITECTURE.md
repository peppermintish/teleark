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

After the managed channel catalog is ready, the retained key owner loads readable credentials from the selected backend and authenticates the newest channel-key proof. That proof selects the current session/upload key; individual corrupt, missing or unknown-key manifests never veto it. Legacy channels without a proof select the newest matching authenticated envelope; when remote evidence is damaged or missing, bounded retained inventory may authenticate the key for that exact account/channel. Durable authenticated inventory retains missing/invalid entries with file-local health. Selection and catalog reads retry transient failures on retained background tasks; backoff releases their owners, honors server deadlines and stops after lock/backend/account replacement. Key import reruns selection; export requires a matching selected key. No stored credential or completed destination is discarded. [ADR 0064](adr/0064-managed-channel-proof-and-durable-automatic-sync.md) supersedes ADR 0057's channel-wide manifest unanimity rule; the existing single upload key and authenticated recovery formats remain.

Channel pushes reconcile from durable account/channel PTS, and each quiet channel has an independent seven-minute fallback. Successful empty differences persist observation time without revising unchanged projections. Restart restores the remaining deadline from disk. Sync feedback includes phases, last activity, retry deadlines and bounded event history across navigation; the sync inspector has no manual retry/cancel controls.

The managed Files view reports scoped PTS checks, difference polling, manifest downloading/authentication and list application before readiness. Package-scoped checkbox selections use cached date/type/search projections. Runtime restores bounded batches through the existing authenticated download path, retains counters and a bounded event timeline, supports failed-only retry, and stops remaining files while the current restore finishes safely. The batch selection is session-owned; started files retain durable transfer recovery and verified outputs.

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

Storage, Telegram and Vault operations run off the UI thread behind bounded request queues and retained lifecycle handles. The Vault owner alone holds the unlocked Master Key. GUI callbacks use account/source generations to reject stale results. Long network operations do not hold snapshot/state locks.

Native download Stop applies to the entire selected set before waiting for storage or filesystem cleanup. Admission and worker retirement share a short barrier; one transaction persists all stop intents and cleanup leases. The UI releases its command owner after that commit and follows per-task cleanup separately. Slot waiters and metadata lookups observe Stop, and drained tasks can clean up while another writer is blocked. Final downloads remain intact. [ADR 0049](adr/0049-batch-download-stop-and-selection.md) defines the batch, selection and partial-file contract.

Channel and managed private-channel synchronization share one retained account owner in Runtime. Telegram contributes bounded metadata pushes and official difference/history APIs; contiguous PTS updates commit directly and gaps recover from durable PTS. Storage returns actual deltas plus coherent local baselines. GUI awaits events and patches only the relevant rows; neither a global revision nor a fixed UI poll drives database reads. Selected/private subscription deadlines, retries and passive metadata reconciliation retain their protocol roles. Private edit/delete pushes immediately publish a pending warning; confirmed observations/acknowledgements persist through schema 12. Private Raw Files uses the shared projection; Files authenticates only new/invalidated local manifest candidates and reuses bounded derived metadata. Manifest queues, reads, authentication and terminal outcomes report through the same event feed and retain cancellation/account/generation guards. Runtime transient channel and metadata failures use jittered exponential backoff with a 60-second ceiling and no attempt limit; server FloodWait remains authoritative. Persistence/authorization/permission failures retain committed state for explicit handling. Runtime also owns initial directory discovery, connection-revision supervision and an independently versioned directory restart cache. Four retained normal-channel calls, one reserved managed-channel call and a separate directory slot prevent blocked normal reads from starving managed synchronization. [ADR 0067](adr/0067-paced-automatic-startup-sync.md) adds account-wide page admission, paced directory pagination, durable cooldowns and cursor-based startup catch-up; the read-only sync inspector has no manual recovery or review controls. Telegram also reserves one managed RPC lane beside eight normal reads, with separate bounded pending admission and the same account barriers. [ADR 0065](adr/0065-managed-progress-batch-restore-and-reserved-sync.md) supersedes the shared four-channel execution limit. Scrolling creates one cancellable viewport history demand, while live pushes can preempt that read. Committed metadata updates the sidebar and material changes invalidate the Remote Library projection automatically. The window has one sync status entry; routine refresh controls and Cmd-R are removed. [ADR 0031](adr/0031-automatic-account-synchronization.md) specifies ownership, bounds and cache compatibility. [ADR 0019](adr/0019-event-driven-channel-projections.md) supersedes the earlier polling/private-reader behavior in [ADR 0018](adr/0018-local-first-channel-sync.md).

Automatic managed-manifest synchronization retains a background caller outside the Vault workers. Each attempt acquires a session-fenced key lease; transient failures retry after 2–60 seconds without requiring a new channel event. Server-requested waits are ephemeral typed error metadata and survive the Telegram/object/Vault adapters without shortening. The account event feed reports retries and terminal outcomes. A scan result cannot replace the key-operation result shown after unlock. Cancellation, account replacement and key/session changes stop retries and reject late projections; backoff never occupies the key, transfer or scan worker. Existing on-disk formats are unchanged.

File-key setup and recovery keep their requested intent and only resume after successful key access. Application access is separately gated by an optional PIN across the whole window. Locking leaves all service owners, key sessions and cached projections running. Account switching, application Quit/Close and proxy application use one event-driven transfer drain gate. See [ADR 0037](adr/0037-application-pin-and-transfer-drain.md), which supersedes the earlier application-facing lock behavior.

The primary sidebar collapses to a 64-point icon rail or expands to 184 points. Channels have their own contextual list; inspectors own bounded, occluding scroll viewports. A retained local-output observer reads account-scoped pages and performs filesystem metadata work through Runtime off the UI thread. The status bar spans the bottom of the entire window and displays synchronization activity without click actions. Network settings opens the bounded, independently scrolling activity inspector without changing the current workspace. It also queries download-volume space independently of directory usage scans.

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

`teleark-i18n` owns Fluent resources at `crates/teleark-i18n/resources/<locale>/main.ftl`, locale negotiation, formatting and structured-error mappings. One registry supplies the nine supported locales to both runtime lookup and Settings. English, Simplified Chinese and Japanese catalogs have full key parity; Spanish, French, German, Brazilian Portuguese, Russian and Korean cover common flows and explicitly fall back to English for remaining messages. Every translated message must preserve its named variable set. See [Localization](I18N.md) for coverage and adding a language. Messages use semantic kebab-case IDs, complete grammatical units and Fluent selectors/plurals; legacy dotted/underscore aliases remain supported. The wrapper resolves top-level message values; terms/attributes require added lookup and validation support. User content is passed as literal parameters.

Negotiation accepts registered tags and maps regional variants to the registered catalog for that language, such as `es-MX` to `es-ES` and `pt-PT` to `pt-BR`. Simplified Chinese maps to `zh-CN`; Traditional Chinese is not substituted with Simplified Chinese. It tries later preferences before falling back to English for unsupported or invalid tags. Tags use the locale parser. Settings changes apply live; explicit overrides persist, System Default removes the override, and a command-line locale wins for that launch. System discovery currently reads locale environment variables; native platform discovery is unfinished. Changing locale never rewrites remote channel descriptions or titles.

Use centralized formatters for dates, counts, percentages, durations and byte
quantities. All sizes/rates use IEC units from B through EiB and their `/s` forms.
Unknown rate/ETA is distinct from zero. [Localization](I18N.md) defines catalog
validation and the non-visual checks for every registered language.

## Technical references

The [documentation guide](README.md) indexes the current contracts, design records
and dated validation evidence. [AGENTS.md](../AGENTS.md) defines contributor rules;
[Security](SECURITY.md) defines trust boundaries and redaction requirements.
Accepted ADRs preserve original decisions and identify subsequent changes.
