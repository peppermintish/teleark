# Implementation status — v0.5.17

## Version 0.5.17 · Telegram durations and selectable inspectors (2026-10-16)

The Telegram adapter reads audio/video duration attributes from grammers 0.11 and retains optional milliseconds through channel updates, cached catalog records and Library projections. SQLite schema 25 adds `logical_files.duration_ms` and `native_download_tasks.media_duration_ms`; existing rows and media with no source duration remain unavailable. Cached Telegram duration survives restart in both Library and local native-download output lists. Channel and Library rows use the compact `M:SS` / `H:MM:SS` formatter. New local Vault uploads inspect bounded playback metadata using `nom-exif 3.8.0` and `lofty 0.24.0`, with cancellation and visible reading/ready/unavailable/limited phases. Manifest 3.0 encrypts duration, dimensions and available audio sample rate, channels and bit depth. Managed-file rows and the selectable lateral inspector display the authenticated values after upload, restart and recovery. Older manifests and unsupported media retain unknown values. Detail rows, captions, transfer inspector fields and shared inspector summaries use GPUI Kit selectable text and the normal copy action.

Upgrade dependencies to `gpui-kit 0.7.3` and grammers `0.11.0` (Telegram API layer 230). The 0.11 client timestamps use `jiff`; retry policies moved to sender-pool configuration; peer caches accept batches. Session codec v1 fixtures verify old-session reads. Community peer metadata is written as session codec v2; other sessions remain v1. The schema migration is additive and transactional; part readers/writers remain 1.0/2.0, new manifest completion writes 3.0 with readers 1.0/2.0/3.0, and pending-upload/key-wrap/recovery formats remain unchanged. Historical sealed outboxes retain their exact version and bytes.

Local verification for manifest 3.0 (2026-10-16): the workspace/all-targets test run completed with **993 passed, 10 ignored and 2 explicitly excluded**. Crypto has 47 passing tests, Runtime 407, and GUI 228. The excluded Telegram proxy tests are `production_pool_routes_all_datacenters_and_reconnections_through_gateway` and `real_plaintext_and_authenticated_mtproto_bootstrap_bytes_reach_only_proxy`: grammers 0.11 panics while reading this Mac’s system DNS configuration (`invalid IP address syntax`); one reproduction failed and the following bootstrap test stalled. These checks are unresolved and the unqualified full-workspace gate is not claimed as passing. No system DNS settings were changed.

`cargo check --workspace --all-targets --locked`, strict workspace Clippy, formatting, warning-denied documentation, `cargo deny check`, locked fuzz-target compilation and explicit Core/i18n tests pass. Dependency audit reports no advisory/license/source failures (duplicate-version warnings remain). Index-based line-ending validation passes, as do line-ending regression, distribution-credential, Store-configuration/client, workflow-graph, MSIX, macOS-library and Linux-payload synthetic checks. `scripts/test-release-artifacts.sh` was not run because this Mac has Bash 3.2 and lacks GNU `find -printf`; its Linux CI gate remains pending.

Native English/light visual review of the audio/video inspector at 900×600 and actual macOS full-screen is complete, including independent scrolling to restore controls. Automated GUI tests cover those sizes and selectable details. Native platform packaging, live Telegram qualification, tagged release CI and Microsoft Store ingestion remain pending. Store submission requires the configured GitHub release workflow; local tests use synthetic media, fake remotes and temporary data without credentials or a live Telegram account.

## Public website and README presentation (2026-10-05)

The README now leads with platform/Microsoft Store labels, a real synthetic-data app preview, features and platform downloads. The matching dependency-free [GitHub Pages website](https://peppermintish.github.io/teleark/) provides Windows Store access, direct Linux/macOS artifacts, checksums and a host-native macOS build tutorial that needs no paid Apple Developer account. It documents the existing self-signed/not-notarized macOS distribution boundary and connects back to the README and source guides.

Eight deterministic website tests pass: version changes update downloads/build commands; incomplete, duplicate, untrusted, draft and prerelease metadata cannot replace the prior page; remote reads have explicit timeout/size limits; failures report no deployment; template escaping, source protection, public artifact contents and local links are checked. The Pages workflow passes actionlint. All six current Linux/macOS artifacts and SHA256SUMS return HTTP 200. English/light browser review covers 900×600, actual full-screen desktop, 375-pixel mobile, landscape and double-size text without horizontal overflow; native disclosures work from the keyboard and focus is visible. Source-build instructions follow the existing build contract; no macOS host was available for a new local compile. No runtime, dependency, persistent-schema or encrypted-format changes are introduced. [Website maintenance](WEBSITE.md) documents offline previews, bounded release lookup and automatic publication after tagged CI completion.

The [initial hosted Pages run](https://github.com/peppermintish/teleark/actions/runs/37276508482) passed both build and deployment. The live HTTPS page returns HTTP 200 with current stable downloads, the Microsoft Store destination and macOS guide, with no unresolved template values. GitHub renders every README badge and preview image successfully; the repository About panel also points to the Pages website.

## Version 0.5.16 · Managed progress, batch restore and independent synchronization (2026-10-05)

The managed Files footer derives its state from account/channel-scoped synchronization, manifest work, key selection and application of the displayed inventory. PTS checks and difference RPCs publish their phase before transport; manifest downloading and authentication remain visible. A generic key-operation success cannot mark a still-pending file inventory complete. Retained presentation timers update elapsed/last-activity text only while real work is active and never drive service queues or database reads.

Files have package-scoped checkboxes with health-based eligibility, selected/matching restore actions and date/type/search filters. Selection survives filtering and is pruned after inventory changes. Runtime owns bounded batch IDs, counters, a bounded timeline and failed-only retry; individual files reuse authenticated streaming, whole-file verification, non-overwriting destinations, transfer controls and durable recovery metadata. Stop remaining allows the current file to finish safely. Batch selections and group summaries are session-owned; started files retain ordinary durable recovery, and completed files remain intact. Active batches stay visible across navigation.

Managed synchronization has one reserved retained execution slot beside four normal channel calls and the directory slot. Telegram reserves a separate managed read slot and pending admission independently of eight ordinary reads. Account barriers cancel both sets and fence stale callbacks. [ADR 0065](adr/0065-managed-progress-batch-restore-and-reserved-sync.md) records the bounds and presentation contract, superseding ADR 0031's shared four-channel execution limit.

SQLite remains read/write 24 with automatic upgrades from 0–23, including skipped versions. Part/manifest readers remain 1.0/2.0 with desktop writes 2.0; key wraps, recovery bundles, pending envelopes and persisted transfer codecs retain their existing versions. No dependency or license changes. New semantic catalog entries are synchronized in all nine catalogs with English fallback.

Local Windows release checks pass: formatting, locked workspace/all-target checking, strict Clippy, 928 passing full-workspace tests (10 existing ignored tests), explicit Core/i18n tests, warning-denied rustdoc and cargo-deny. After the final minimum-window inspector adjustment and batch-result presentation test, all 203 GUI tests pass (3 existing ignored tests). Focused channel-sync tests recheck the reserved execution bound after managed-binding replacement. The service test restores three files through real crypto/temporary SQLite and a synthetic remote, compares final bytes, stops remaining files behind blocked transport without losing the current output, and retries only a failed file. Distribution/Store configuration, all 29 synthetic Store tests, workflow graph, MSIX/release artifacts, macOS libraries, Linux payloads and LF fixtures pass. English/light native previews verify checkboxes, selected/all actions, filters and independent inspector scrolling at 900×600 and actual full-screen; the inspector no longer covers Download selected. No personal credentials, documents or live Telegram session are used. Metadata is synchronized to 0.5.16. Hosted platform packaging and Microsoft Store ingestion remain pending the tagged release workflow.

## Version 0.5.15 · Managed channel reliability (2026-10-02)

Encrypted download segments were emitted at the native 1 MiB geometry into a reader limited to 512 KiB, causing valid content to fail with an integrity classification. The connected adapter now splits segments into ordered bounded blocks; synthetic remotes use that same boundary adapter. A regression uploads and downloads an 11 MiB + 17 B file, repeats after restart, and compares every byte.

Channel selection now authenticates the newest channel proof independently of file health. Corruption of another file no longer clears the channel key or the entire file projection. Durable authenticated inventory retains damaged/deleted entries with specific health, including deletion between catalog and fetch. Existing keys, encrypted bytes and completed destinations are preserved. Legacy channels without a key proof can authenticate retained account/channel-scoped inventory when their only remote manifest is damaged or deleted. Transient key-validation failures recover automatically while releasing the key owner during backoff; an explicit lock fences the old retry.

Push reconciliation remains account/channel scoped. Each channel gets its own seven-minute quiet recovery deadline, independent of other traffic. Schema 24 adds durable successful-observation times, including empty final differences; restart reuses them without changing row revisions. Ordered automatic upgrades preserve older state, with transactional rollback and retry on verification failure. The sync inspector explains automatic recovery and has no Retry Sync or sync cancellation controls. Empty Files guidance describes automatic discovery. The upload dialog no longer has an Upload options drawer; file selection, automatic encryption and the queue action remain available.

Local Windows source gates pass: formatting, locked workspace/all-target checking, strict Clippy, 919 passing full-workspace tests (10 existing ignored tests), explicit Core/i18n checks, warning-denied rustdoc and cargo-deny. After the final legacy-inventory and upload-dialog changes, all 24 device-key tests, all 199 GUI tests (3 existing ignored tests), all 26 i18n tests, checking and strict Clippy pass. The added legacy regression retains authenticated inventory and completed output after corruption/deletion. Distribution/Store configuration, the 29 synthetic Store tests, workflow dependency graph, MSIX/release artifacts, macOS libraries, Linux payloads and LF policy fixtures pass. English/light synthetic previews verify the automatic-sync inspector and upload dialog at 900×600 and actual native full-screen, with primary actions reachable and inspector scrolling independent. Release metadata is synchronized to 0.5.15; hosted platform packaging and Store ingestion are pending the tagged workflow. Synthetic tests do not establish recovery of the reported user's PDF or live Telegram behavior; no personal credentials or documents are used. Existing encrypted readers/writers, recovery bundles and transfer codecs retain their versions.

## Version 0.5.14 · Independent MSIX Store delivery (2026-09-30)

Replaced the closure-bound PowerShell Store HTTP client with a standard-library Python client based on Microsoft's MSIX submission documentation. Native Linux/Windows/macOS tests exercise its production HTTP path with a synthetic loopback service. CI publishes the verified cross-platform GitHub Release after all quality/test/package gates, then runs a separate protected Store workflow. Store delivery verifies released bytes and manifest identity/version, preserves listing and publication settings, uploads the API-required ZIP in bounded blocks, commits and observes ingestion status. A matching draft can resume by package SHA256; a committed matching draft is only monitored. Unrelated drafts are preserved. Phase output and versioned bounded receipts survive failures, and manual delivery on `main` can retry the same published tag without a version bump or rebuild. [ADR 0063](adr/0063-independent-msix-store-delivery.md) and [Packaging](PACKAGING.md#windows) supersede the old release ordering and Store client instructions.

The root failures were PowerShell transport scope resolution in v0.5.12 and a failing Linux diagnostic assertion in v0.5.13; the native MSIX build itself succeeded. The retired client's JSON/XML constructors also rejected charset parameters in media types, hidden by transport mocks. All 29 current synthetic Store tests pass on Windows and Linux, covering production HTTP framing, the token-success/Store-access rejection boundary, fixed-name validation diagnostics, matching update-body identity and receipt recovery without new draft creation. All 29 passed in the successful hosted Ubuntu Store delivery job; the initial 18 also passed on all three native OS runners. Workflow lint, dependency-graph checks, configuration redaction, MSIX verification, release-asset assembly and line-ending checks pass. Full local Windows source gates pass: formatting, locked workspace/all-target checking, strict Clippy, 915 passing workspace tests (10 existing ignored tests), explicit Core/i18n tests, warning-denied rustdoc and cargo-deny. Application behavior, persistent schema versions, encrypted formats and supported upgrade paths are unchanged from v0.5.13.

The [v0.5.14 tagged run](https://github.com/peppermintish/teleark/actions/runs/36641298549) passed repository quality checks, all three native OS test jobs, all three platform package jobs and release assembly. The [published release](https://github.com/peppermintish/teleark/releases/tag/v0.5.14) contains all eleven expected assets. Its initial Store job passed release verification and credential configuration, then `GetApplication` returned HTTP 401 after token issuance, before draft creation or upload.

Edge inspection after the developer-tenant sign-in confirmed that Partner Center's Microsoft Entra applications list was empty, although the CI registration existed in Entra. The developer explicitly approved Windows Manager access; Partner Center now shows the CI application and service principal with that role. The [post-association retry](https://github.com/peppermintish/teleark/actions/runs/36651788240) passed `GetApplication` and created a draft, confirming that the missing application association caused the Store access rejection. The earlier interactive `AADSTS16000` names a personal account in the Microsoft Services tenant; Packaging distinguishes portal directory access, Entra registration and Partner Center app association.

Live draft updates then failed with HTTP 400 and `InvalidParameterValue` referencing `id`. Keeping the top-level submission ID alone did not resolve the rejection. The client had omitted immutable IDs from existing packages marked `PendingDelete`; preserving those IDs resolved it. The [package identity retry](https://github.com/peppermintish/teleark/actions/runs/36654441135) successfully updated the exact retained draft, uploaded both ZIP blocks, finalized the upload and sent an accepted commit request. Microsoft accepted the submission into `PreProcessing`, and the Store workflow completed successfully with its sanitized receipt retained. Certification, signing and publication remain Microsoft-controlled; this confirms ingestion acceptance, not final Store publication. Verified receipt recovery established ownership of the draft created before its marker update failed, without deleting it or creating another. Missing/invalid old package IDs fail before update, and synthetic HTTP tests enforce preservation. Bounded static validation context redacts every unrecognized token and excludes correlation values.

## Version 0.5.13 · Microsoft Store transport recovery (2026-09-30)

The default Store HTTP transport binding is restored. `.GetNewClosure()` prevented the module-private transport function from resolving through the request closure, so the workflow failed before sending token requests. Store API failures during request setup, request sending or response reading now carry a sanitized transport summary. The summary is limited to the phase, normalized category (`dns`, `socket`, `tls`, `timeout` or `other`), allow-listed exception type and HRESULT. Raw exception text, request bodies, client secrets, access tokens, SAS URLs and upload URLs remain excluded. The Store API authentication and submission flow remains unchanged; app runtime behavior, persistent schemas and encrypted formats are unchanged.

The matching v0.5.13 release tag must rerun the hosted Store submission path to qualify the corrected default transport. No live Store API request or submission has been made for this patch.

## Version 0.5.12 · Cross-platform release validation (2026-09-30)

The v0.5.11 tag run passed all three OS test jobs and all three package jobs. Its Linux quality job failed in the mocked Store upload failure regression because the mocked callback could not see the assertion helper in its PowerShell scope; Store submission and GitHub Release were consequently skipped. Capture the assertion scriptblock in the mock callbacks to make the check scope-stable across Linux and Windows PowerShell. This is test-only. The v0.5.11 tag remains unchanged. The corrected synthetic API test passes locally on Windows and Linux PowerShell 7.6.6, including a GitHub-style dot-sourced wrapper, and actionlint passes. The v0.5.12 tagged workflow must still qualify the release jobs. No app runtime behavior, persistent schemas or encrypted formats changed.

## Version 0.5.11 · Store API release pipeline (2026-09-30)

Release CI now starts repository quality checks, Linux/Windows/macOS tests and all three platform package jobs concurrently after lightweight version and configuration validation. Store submission waits for quality, the OS test matrix and the verified Windows MSIX; GitHub Release assembly waits for quality, tests, all verified package outputs and successful Store submission. The workflow submits later MSIX updates through Microsoft's legacy Store Submission API. It sends the existing client secret only in the OAuth HTTPS form body, wraps the verified package in the API-required ZIP, uploads through the returned SAS URL in bounded blocks, updates and commits the draft, and polls submission status. The API does not require Seller ID, so the existing GitHub secret is retained without being read by CI. Local mocked HTTP regressions cover token redaction, submission metadata, ZIP/block upload, commit, polling and failure handling. No live Store call or certification was performed; the app must have an earlier completed submission and age rating before the API can update it. No application runtime behavior, database schema or encrypted format changed.

## Version 0.5.10 · Reliable channel and encrypted download recovery (2026-09-29)

Channel directory synchronization now retries typed transient network, server and write-conflict failures with bounded backoff and honors Telegram-provided FloodWait deadlines. Directory refresh and cancellation have their own commands, and the global retry action follows the failure event when another channel is active. Authentication, permission, capacity, persistence and explicit cancellation outcomes do not receive automatic retries.

An incomplete remote encrypted-part stream is classified as a transient network failure. Download recovery retries that extent from its start at most twice, waits 100 ms then 200 ms, and records a receipt only after the full expected length and plaintext digest are verified and synchronized. Authentication and integrity failures are terminal. A retry-exhausted download retains its partial file without publishing the destination; whole-file verification still precedes publication.

The Transfers inspector now renders measured download progress, retry attempts and waits, per-part states, and a bounded download timeline and rate chart. Part maps show up to 4,096 parts and disclose when later parts are omitted. Telemetry and log records identify upload and download direction. Release artifact assembly checks each downloaded platform checksum manifest and the exact seven-package set; Windows validates the exact MSIX and its checksum after build and after artifact download.

Synthetic regressions cover directory retry/cancel routing, interrupted multi-part downloads, receipt and publication boundaries, inspector routing/map truncation, and release artifact checksum failures. No SQLite schema, Vault recovery codec or encrypted payload format changed. The release workflow's hosted package run remains the qualification for published artifacts.

## Version 0.5.9 · Remove Hindi interface language (2026-09-25)

Removed `hi-IN` from the supported-locale registry, translation resources, settings and locale negotiation. Windows systems configured for Hindi now use the English fallback unless another supported language is selected. The Windows MSIX declares the remaining nine UI languages. Persistent schemas and encrypted formats are unchanged.

## Windows MSIX-only Store release workflow (historical ordering, 2026-09-25)

Windows builds produce one x64 `.msix`; the old EXE, ZIP, MSI and Inno Setup artifacts are removed. The original pipeline required Store submission before GitHub publication; v0.5.14 and [ADR 0063](adr/0063-independent-msix-store-delivery.md) supersede that ordering. Pull requests and package previews do not submit. The first Store submission and Microsoft's certification requirements remain applicable. The Store package keeps its separate data set under `%USERPROFILE%\TeleArk`. [Packaging](PACKAGING.md#windows) records current delivery behavior.

## Microsoft Store MSIX distribution checkpoint (superseded, 2026-09-24)

The Windows release workflow had an opt-in Microsoft Store upload bundle. It required the exact Partner Center identity name, publisher, and publisher display name as GitHub repository variables, built a separate `msix` feature binary, created one `.msixupload` (including compressed public symbols when the PDB was available), and included it in tagged GitHub Release checksums. CI did not submit the package to Partner Center. The Store build wrote a new, independent data set under `%USERPROFILE%\TeleArk`, outside AppData, so Windows package virtualization did not redirect SQLite/WAL or session files; existing unpackaged Windows data was not imported. Windows-host validation passed for the two data-root tests, the optimized MSIX-feature build, MakeAppx packaging with symbols, test signing, checksum generation and actionlint 1.7.12. Partner Center identity association and certification remained unverified. [Packaging](PACKAGING.md#windows), [security](SECURITY.md#local-data-api-configuration-and-logs) and [ADR 0058](adr/0058-microsoft-store-msix-distribution.md) define the distribution contract recorded at that checkpoint.

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
| Encrypted parts and manifests | Parts read 1.0/2.0 and write 2.0; manifests read 1.0/2.0/3.0 and new completion writes 3.0 | [Crypto](CRYPTO_FORMAT.md), [Manifest](MANIFEST_FORMAT.md) |
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

The v0.5.9 local source run passed formatting, locked workspace checking,
strict Clippy, 914 serial workspace tests (10 manual benchmarks ignored), explicit
Core/i18n tests, warning-denied rustdoc, dependency policy, script regressions and
release-version validation. The [GUI interaction audit](validation/2026-09-26-gui-interaction-audit.md)
records the exact checks and macOS native review. These results do not establish
published installers or live Telegram qualification. Current commands are in
[Development](DEVELOPMENT.md#quality-gates).

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
