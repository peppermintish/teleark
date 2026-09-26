# Implementation status — v0.5.8

This is the current capability and qualification snapshot. Release changes belong
in [CHANGELOG.md](../CHANGELOG.md), format details in their specifications, and
design history in [ADRs](adr/). Earlier validation notes remain in Git; for example,
`git show v0.5.8:docs/IMPLEMENTATION_STATUS.md` reads the pre-consolidation record.
A local test result or version tag does not establish a successful hosted release.

## Current capabilities

| Area | Implemented behavior | Limits |
| --- | --- | --- |
| Desktop | GPUI Kit, virtualized channel and transfer views, shared compact rows, independent inspectors, keyboard controls, optional application PIN | Full accessibility, backing-scale and platform qualification remains open. PIN gates the UI, not disk encryption. |
| Account | QR or phone/code/2FA login, restored account entry, explicit sign-out and account switching, saved personal API configuration | Protected real-account lifecycle and reconnect qualification remains open. |
| Managed channel | Remote-authoritative discovery, creation and identity verification; repairable metadata; safe replacement after confirmed channel loss | Conflicting or damaged discovery evidence blocks writes. Deleted remote files cannot be recreated by replacing a channel. |
| Keys | One selected key authenticates the channel's proof/manifests; fresh channels receive a persisted key and verified remote proof; import and selected-key export | A channel whose files require multiple keys is unavailable in Files; Raw Files and stored credentials remain intact. |
| Credential storage | Noninteractive macOS Keychain by default; explicit verified backend switching; private SQLite on Windows/Linux | SQLite and its backups expose stored credentials/recovery secrets. Keychain access under other identities, locked login Keychains and user-specific ACLs needs further qualification. |
| Channel index | Local-first browsing, bounded history pages, account-owned push/difference recovery and visible synchronization | Filtered batches cover indexed history, not necessarily all remote history. Generic Index coverage coordination remains separate. |
| Native downloads | Account-scoped tasks/batches, durable bitmap resume, pause/cancel/retry, connection failover, terminal history deletion and retry of missing batch outputs | Exact byte-length verification only; no cryptographic content authentication. Live-network throughput and server-limit behavior are unmeasured. |
| Encrypted transfers | Bounded-memory upload/download, durable admission and controls, authenticated parts/manifests, restart dispatch, source-free replay of sealed manifests and portable pending-upload recovery | Original source is required for unfinished payloads. Legacy summaries lack resumable context. No automatic remote orphan deletion; blocking OS calls are not preemptively cancellable. |
| Local availability | Bounded Runtime filesystem observers, native watch notifications, event-driven UI updates and fallback rechecks | Metadata checks do not authenticate same-size replacement. Watch coverage varies by volume; retained inventory does not discover arbitrary local files. |
| Library | Local/remote views, account-scoped search and paging, bulk actions, non-overwriting destination admission | Library pages are not virtualized; million-record performance is not established. |
| Localization | Ten registered languages, semantic Fluent IDs, English fallback, localized formatting and IEC byte units | Seven catalogs are partial. Native OS locale discovery and native-speaker review remain open. Interface review uses English only. |
| Diagnostics | Bounded phase/event histories, live and historical transfer inspectors, private sanitized logs and explicit omission counts | Logs may omit samples. General disk retention and Index event coverage need further work. |
| Packaging | Windows x64 EXE/ZIP/installer, universal macOS executable/app/PKG, Linux x64 AppImage/archive/DEB | macOS binaries are self-signed; no Developer ID, notarization or Gatekeeper trust. Hosted results must be checked per release. |

## Compatibility

The application version and persistent format versions are independent. Newer or
unsupported data stays intact and receives actionable guidance. Routine supported
upgrades do not require manual exports, resets or scripts.

| Contract | Current support | Reference |
| --- | --- | --- |
| SQLite | Read/write schema 23; automatic ordered upgrades from 0–22, including skipped releases | [Data model](DATA_MODEL.md) |
| Encrypted parts and manifests | Read 1.0 and 2.0; new desktop payloads use 2.0 | [Crypto](CRYPTO_FORMAT.md), [Manifest](MANIFEST_FORMAT.md) |
| Key wraps and recovery bundles | Retain version 1 readers/writers and existing bytes | [Crypto](CRYPTO_FORMAT.md) |
| Channel-key proof / pending key marker | Proof codec 1; local marker writes 2 and reads 1/2 | [Crypto](CRYPTO_FORMAT.md#managed-channel-key-proof), [Preferences](PREFERENCES_FORMAT.md) |
| Executable transfer context | Read 1/2; explicit current geometry writes 2; legacy geometry retains its original encoding | [Recovery records](VAULT_TRANSFER_RECOVERY.md) |
| Pending remote upload | Independent codec 1; incomplete until the final authenticated manifest is published | [Pending format](PENDING_UPLOAD_FORMAT.md) |
| Credentials and proxy | API credential codec 1; proxy readers support inline v1 and credential-reference v2 | [Data model](DATA_MODEL.md#credential-storage-schema-23) |
| Native bitmap / transfer logs | `TARKDPM1` v1 unchanged; log readers retain schema 1 and support schema-2 part causes/slots | [Transfer](TRANSFER_ENGINE.md) |

Encrypted containers have a 1.9 GiB encoded ceiling. Each full encoded crypto frame is
512 KiB including headers and authentication tags; plaintext frames are smaller.
The current Telegram adapter sends 128 KiB RPC requests.
Uploads create no new ciphertext payload spools. Restart reuses verified published
containers and reserves fresh encryption identities for unpublished data whose
immutable in-memory ciphertext was lost. Older spools remain read-only compatibility
inputs. See [ADR 0041](adr/0041-memory-streaming-and-portable-upload-recovery.md).

Schema migrations run on retained background owners with detection, preparation,
conversion and verification feedback. Transactions preserve recoverable originals.
Cross-format conversion and external backup orchestration are not a completed
general-purpose migration service.

## Verification record

The v0.5.8 release notes record local formatting, locked workspace checking,
strict Clippy, serial workspace tests, Core/i18n tests, warning-denied rustdoc,
cargo-deny, script regressions and release-version validation. Those are historical
results, not reruns on every later commit. The earlier record is available at the
version tag; current commands are in [Development](DEVELOPMENT.md#quality-gates).

Recovery tests compose Runtime owners, real temporary SQLite/files, crypto and
synthetic wire transports. They cover supported codec fixtures, stale accounts and
execution generations, blocked dependencies, cancellation, lost publication replies,
verified extent reuse and abrupt child-process exit. They do not prove hardware
power-loss safety, every hostile-filesystem race or live Telegram interoperability.
The [recovery guide](VAULT_TRANSFER_RECOVERY.md) describes those boundaries.

Recorded native visual reviews use synthetic previews. Current acceptance requires
English, light mode, 900×600 and actual native full-screen. Earlier reviews of other
locales/themes remain historical evidence, not instructions for new UI passes.

The [2026-09-26 documentation/script audit](validation/2026-09-26-open-source-audit.md)
records corrected guides, packaging regressions, Git-history findings and current
validation. It changes no application release version or persistent format.

The [2026-09-26 GUI interaction audit](validation/2026-09-26-gui-interaction-audit.md)
tracks keyboard submission, dialog cancellation, selection isolation, action
feedback and bounded multi-file download controls. It records automated and
native review evidence separately. The macOS synthetic English/light review covers
900×600 and actual fullscreen; live-account and other-platform qualification
remain open.

## Next actions and release gates

1. Qualify creation/discovery/repair, account switching, upload/download/recovery,
   timeout, reconnect and crash boundaries with a protected real account.
2. Extend partial-file identity and publication hardening across platforms; test
   remaining pathname-substitution, shutdown, power-loss and low-space boundaries.
   Reconcile remote orphans without deleting live or recoverable work.
3. Measure real-network multi-connection behavior and library/index scaling before
   making throughput or million-record claims. A fully one-pass mutable-source
   upload is not implemented; current hashing and encryption use separate passes.
4. Complete English/light native UI review on supported platforms at both required
   sizes, including keyboard, screen readers, backing scales and sleep/logout.
   Test catalogs without opening other-language interfaces.
5. Qualify platform permissions, SQLite sidecars/backups, credential migration,
   native locale discovery, the macOS 11 floor and installer upgrades on clean hosts.
6. Extend versioned vectors, fuzzing and integrated migration evidence. Complete
   cross-format/external-backup coordination where future formats require it.
7. Resolve the difference between the 512 KiB transport-block contributor rule
   and the current adapter's 128 KiB RPC requests. Older temporary RPC bitmaps
   lack explicit geometry; the reader safely retires mismatched checkpoints but
   reports them as corrupt. This audit documents the discrepancy without changing
   payload codecs or transfer behavior.
8. Before public launch, establish a usable private vulnerability-reporting route
   and decide whether to publish the full historical refs or a curated history.
   Historical build artifacts are not removed by deleting them from the current tree.

A third-party audit may add evidence; it is not a mandatory delivery prerequisite.
Open items are concrete product or qualification work, not maturity labels.
