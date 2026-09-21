# Implementation status — v0.4.8

## Linux release payload verification correction (2026-09-21)

The supplied `v0.4.8` runner log shows successful creation of the AppImage, portable archive and Debian package, followed by an invalid byte comparison between the portable and Debian executables. `linuxdeploy` strips and rewrites the portable ELF's RPATH, while `cargo-deb` processes the original executable separately. Verification now compares AppImage with the portable archive, checks the Debian payload independently and retains the installed-file comparison against that Debian payload. All three executables must be Linux x64 ELF files; metadata, dependencies, checksums and downgrade refusal remain checked.

A fast regression accepts the expected difference between Debian and portable bytes and rejects divergent portable executables, empty Debian payloads and missing library notices. It passed locally under Git Bash, along with Bash syntax and workflow lint. The single workflow runs this regression before compiling Rust and displays Linux packaging and verification as separate steps. This Windows host has no Linux runtime, and the connected browser is not signed into the private repository; native Linux installation and the hosted result remain unverified here.

## Version 0.4.8 (2026-09-21)

The version tag follows the single-run trigger correction below. Workspace crates, lockfiles, macOS bundle metadata, Windows installer fallback and the three About catalogs now agree on 0.4.8. Tagged releases still build nine files for Windows x64, universal macOS and Linux x64. SQLite read/write remains schema 22 with supported automatic upgrades from 0–21; encrypted file and recovery formats are unchanged. Hosted package verification and publication depend on the `v0.4.8` workflow run.

Local 0.4.8 validation passed workflow lint, LF checks, the macOS universal-library regression, formatting, locked workspace check, strict Clippy, the full serial workspace suite, explicit Core/i18n tests, warning-denied rustdoc and cargo-deny. The version resolver accepted `v0.4.8`. A Windows x64 static-CRT release build produced the standalone executable, ZIP and installer; native verification checked checksums, in-place upgrade and downgrade refusal, then removed its test installation. Hosted macOS and Linux package verification remains pending.

## CI trigger correction (2026-09-21)

The [single CI/CD workflow](../.github/workflows/ci.yml) now starts automatically for pull requests and `v*` tag pushes, with manual dispatch retained. It no longer starts on a branch push. Pushing `main` and a version tag together therefore creates one release run, avoiding the second Linux quality run and branch test matrix observed for `v0.4.7`. Direct pushes to `main` require a pull request or manual dispatch for CI; release tags continue to run the full quality gate before packaging. [ADR 0046](adr/0046-tag-only-push-trigger.md) records this trigger choice.

## Version 0.4.7 (2026-09-21)

The release target set is Windows x64, universal macOS with Intel and Apple Silicon slices, and Linux x64. The single workflow now expects exactly nine standalone, portable and installer files before publication; the Windows/Linux ARM64-only jobs and local packaging modes were removed. Universal macOS library verification inspects each architecture separately, and the Debian metadata now includes the copyright field required by `cargo-deb`. A deterministic macOS library-check regression covers both slices, non-system dependencies and missing inspection data. [ADR 0045](adr/0045-focused-release-targets.md) supersedes the wider target list in ADR 0044.

The `v0.4.6` hosted run created macOS and Linux portable outputs but failed before package verification: the macOS check misread a universal-binary heading, and `cargo-deb` rejected missing copyright/authors metadata. Publication was skipped. The `v0.4.7` tag must exercise the corrected macOS package and Linux Debian installer on hosted runners before those artifacts or installer behaviors can be counted as verified. The app remains unsigned and unnotarized on macOS.

Workspace crates, lockfiles, macOS bundle metadata, Windows installer fallback and the three About catalogs now agree on 0.4.7. No persistent schema or codec changed: SQLite read/write remains schema 22 with supported automatic upgrades from 0–21; encrypted file and recovery readers retain the documented compatibility in the [recovery guide](VAULT_TRANSFER_RECOVERY.md).

Local 0.4.7 validation passed workflow lint, LF policy and regression tests, Bash package-script syntax, formatting, locked workspace check, strict Clippy, the full serial workspace suite, explicit Core/i18n tests, warning-denied rustdoc and cargo-deny (existing duplicate-version warnings). The version resolver accepted `v0.4.7`, and all three packaging helpers rejected removed single-architecture targets. A Windows x64 static-CRT release build produced the executable, ZIP and installer; the native verifier checked checksums, in-place upgrade and downgrade refusal, then removed its test installation. Hosted macOS and Linux package verification remains pending until the new tag run.

## Version 0.4.6 (2026-09-21)

Workspace crates, lockfiles, macOS bundle metadata, Windows installer fallback and the three About catalogs now agree on 0.4.6. No persistent schema or codec changed: SQLite read/write remains schema 22 with supported automatic upgrades from 0–21; the encrypted file and recovery readers retain the documented compatibility in the [recovery guide](VAULT_TRANSFER_RECOVERY.md).

One [CI/CD workflow](../.github/workflows/ci.yml) now runs LF, legal and full Linux quality gates for every trigger. Branches and pull requests continue to Windows/macOS tests; a matching version tag instead builds Windows x64/ARM64, universal macOS and Linux x64/ARM64 packages, verifies their install and downgrade behavior, and publishes exactly 15 package files with licenses, notices and checksums. An optional manual package preview uploads artifacts without publishing. GitHub job summaries show stage results, tracked-file LF counts, exact Rust cache hits and asset hashes. The [packaging guide](PACKAGING.md) names every file and platform limit; [ADR 0044](adr/0044-single-workflow-native-release-matrix.md) records the workflow decision.

Local 0.4.6 validation passed workflow lint, PowerShell/Bash package-script syntax, LF policy tests, formatting, locked workspace check, strict Clippy, 769 serial workspace tests (10 existing ignored), explicit Core/i18n suites, warning-denied rustdoc and cargo-deny (existing duplicate-version warnings). The version resolver accepted `v0.4.6` and rejected `v0.4.5`. The Windows x64 static-CRT release build produced a standalone executable, ZIP and installer with three verified checksums. The installer was silently installed; an in-place upgrade preserved an unrelated file and restored the registered version, while a downgrade failed with a logged reason and unchanged executable. The hosted native package matrix remains separate evidence. macOS packages remain unsigned and unnotarized; the Linux portable packages still rely on the host kernel, desktop and graphics drivers.

## Version 0.4.5 (2026-09-21)

The workspace crates, lockfiles, macOS bundle and Windows installer fallback version now agree on 0.4.5. The release record is synchronized with the English, Simplified Chinese and Japanese About catalogs. This version update changes no persistent schema or codec: SQLite read/write remains 22 with automatic supported upgrades from schemas 0–21; encrypted file and recovery format support is recorded in the [recovery format guide](VAULT_TRANSFER_RECOVERY.md). Release artifacts and their upgrade/downgrade behavior are described below and in the [packaging guide](PACKAGING.md). The `v0.4.5` tag selects the release workflow only after it is pushed to GitHub; local tag creation alone does not publish artifacts.

The CI matrix checks Git-index line endings on Linux, Windows and macOS; the release policy job runs the same check before publication. The check rejects CRLF in tracked plain text and all `.cmd`/`.bat` scripts, with counts and file annotations in the GitHub Actions summary. `.editorconfig` directs editors to UTF-8/LF, while the existing `.gitattributes` requests LF checkout. Synthetic checks cover LF text, binary data containing CRLF bytes, actual CRLF text, both forbidden extensions and report output.

Local 0.4.5 validation passed formatting, workspace check, strict Clippy, all 769 workspace tests with serial execution (10 existing ignored), explicit Core/i18n suites, warning-denied rustdoc, cargo-deny (existing duplicate-version warnings), workflow lint and the line-ending policy tests. The default parallel suite timed out twice in the existing `blocked_transfer_does_not_block_lock_unlock_or_preserved_queued_work` runtime test; that test passed alone and in the serial suite. The explicit Windows x86_64 static-CRT release build and packaging produced the standalone executable, portable ZIP and installer with 0.4.5 metadata. Hosted macOS/Linux packaging and GitHub Release publication require the tagged workflow run.

## Release artifacts and installer upgrades (2026-09-21)

The release workflow now resolves one numeric GUI/Cargo version, rejects a mismatched `vX.Y.Z` publishing tag, and builds explicit Windows x86_64, macOS arm64 and Linux x86_64 targets. Each job requires a standalone executable, a portable archive and a native installer; tagged publication checks the exact nine-file set with legal files and unified checksums. Manual runs retain the numeric installer version and identify the source commit in filenames. Windows Inno Setup, macOS Installer and Debian `preinst` guards allow in-place upgrade and reject an older installer with a reason before replacing files. The [packaging guide](PACKAGING.md) lists artifacts and limits; [ADR 0043](adr/0043-release-artifact-and-installer-version-contract.md) records the version contract.

Local validation includes script syntax, packaging dry runs, version/tag mismatch checks, workflow lint, and a real Windows installer compile plus silent install/upgrade/downgrade tests. The blocked Windows downgrade returned a failure, logged its reason and left the installed executable unchanged. The workflow's explicit Windows x86_64 MSVC release command with static CRT passed; packaging that output produced three nonempty artifacts, matching 0.4.4 binary/installer metadata and three verified checksums. All full source gates passed: formatting, workspace check, strict Clippy, 769 workspace tests (10 existing ignored), explicit Core and i18n suites, warning-denied rustdoc and cargo-deny (existing duplicate-version warnings). The first workspace test run had one timeout in an existing runtime concurrency test; that focused test and the complete rerun passed. Hosted macOS and Linux package builds and native installer checks remain to be exercised by a workflow run. The macOS output remains unsigned and unnotarized. This packaging change does not change persistent schemas, codecs or app release version.

## Unavailable private-channel recovery (2026-09-15)

When the account's bound private channel becomes unavailable after deletion or loss of access, automatic management no longer retries its obsolete ID forever. A complete Telegram dialog read and typed remote health result precede one fresh discovery: a unique verified managed candidate is connected, or a new private channel is created and verified. Storage changes the saved account binding only if its old ID still matches. Owner-lost but present channels, unsafe settings, damaged/newer identity, ambiguous discovery, uncertain creation and truncated rosters remain blocked; no arbitrary title or old local cache authorizes adoption. Old channel-scoped history, encrypted inventory, keys and local copies are not rewritten or deleted; files removed with the old Telegram channel are not restored by the replacement. Setup phases and bounded timeline remain visible through loading/failure and completion. [ADR 0042](adr/0042-unavailable-channel-replacement.md) supersedes ADR 0025's no-replacement clause.

Validation: the full locked workspace suite passed (773 tests, 10 existing ignored), including new unavailable/forbidden versus owner-lost decisions, cross-connection compare-and-swap, bounded phase retention and GUI notice/acknowledgement regressions. Workspace check, strict Clippy, formatting, warning-denied rustdoc, cargo-deny (existing duplicate warnings), and diff whitespace checks passed. Native synthetic English/light previews at 900×600 and actual macOS full-screen verified preparation feedback, a visible replacement notice, acknowledgement returning to the file-first workspace and the retained replacement history in channel details. Actual Telegram deletion/recreation has not been performed in this local validation. Existing historical entries below retain their time-local claims.

## Memory streaming, portable recovery and receipt rates (2026-09-15)

Implemented the six-point transfer update and the memory-only payload rule in
[AGENTS.md](../AGENTS.md). New encrypted containers are capped at **1.9 GiB**
(2,040,109,465 encoded bytes including framing), while full encryption/transport
blocks remain 512 KiB. Upload encrypts and sends from bounded memory. Download
authenticates/decrypts in memory, writes plaintext partial output and publishes it
only after complete verification. No new ciphertext spool is created; old ones
remain readable solely for compatible recovery.

Whole-file/container BLAKE3 share one source pass. A separate incomplete remote
announcement precedes that pass and carries authenticated published-container
progress afterward. A second independent database with the recovery key and same
source can discover and resume it in the same account/channel. Wrong sources are
rejected, published containers are verified/reused, and lost unpublished memory
gets a new encryption identity. Pending metadata and local context formats are
specified in [ADR 0041](adr/0041-memory-streaming-and-portable-upload-recovery.md),
[pending format](PENDING_UPLOAD_FORMAT.md) and
[recovery formats](VAULT_TRANSFER_RECOVERY.md). V1 context bytes/keys retain their
meaning; v2 persists geometry. SQLite remains 22 and existing migrations remain.

RPC acknowledgements update shared memory counters immediately. Ordinary numbers
publish once per second using a trailing three-second window with fixed 50 ms
buckets. First usable samples and important transitions publish promptly; silence
expires the rate, outstanding requests show waiting/last confirmation, and restored
progress produces no burst. Checkpoint saves run concurrently and retain progress.
Per-account rates are incremental, stable row identities and changed-row overlays
avoid rebuilding task history, and filters/action scopes are cached. Expanded
upload/download rows show processed/total size and ETA with complete-value tooltips.
Charts retain 96 samples, timelines 128 events, and block maps at most 128 grouped
cells; omission/aggregation is visible.

Deterministic evidence includes first-block streaming, authentication-before-output,
blocked persistence with continuing acknowledgements, timing/restore/duplicate
behavior, compatible frozen codecs, transactional import rollback, and a real-crypto
two-database resume/download round trip. Full gate and interface results are recorded
in the [validation record](validation/2026-09-15-memory-streaming.md). Tests/previews
use synthetic data; no live Telegram throughput
or reduced-startup-duration claim is made. The complete source baseline still
precedes payload encryption, so this is not a fully one-pass mutable-source upload.
Remote metadata/orphan messages are retained; distributed writer exclusion and
remote cleanup are outside this handoff boundary. Earlier entries below are
historical observations, superseded where this section/ADR 0041 says otherwise.

## Upload byte-count scope (2026-09-15)

The upload activity line now explicitly labels acknowledged encrypted bytes as
the **current container**. A 60 MiB plaintext container appears as approximately
62.9 MB in decimal display units, including its small framing overhead; that
denominator is not the size of an encryption frame. Production continues to stream
512 KiB wire blocks through the bounded queue. Source checking and other phase
counts retain their own scope. No codec, persistence or transfer behavior changes.

A deterministic production-path regression holds the consumer after the first
512 KiB block, verifies that the 60 MiB container is still unsealed and the spool
cannot exceed the bounded queue plus two blocks, then exercises both completion
and consumer failure with producer termination. GUI tests distinguish container
counts from other phases and retain collapsed-batch/terminal coverage. Runtime,
Telegram, GUI and i18n tests passed (533 tests; seven existing probes ignored),
along with formatting, strict affected-crate Clippy and a debug build.
English/light synthetic previews passed at 900×600 and actual native full-screen,
including primary actions and independent inspector scrolling. No credentialed
Telegram upload or installed release replacement was performed.

Known startup limitations, confirmed by source inspection: each upload (including
source-dependent resume) finishes a whole-file BLAKE3 admission pass before starting
the streaming producer. That pass reads with a bounded 1 MiB buffer and records
both whole-file and container digests; it is not encryption, but delays the first
network part. The transfer list still reads `average_bytes_per_second`, and the
top-level total reads controller goodput; both are updated only after an entire
container is uploaded and published. The inspector already receives individual
part acknowledgements and rate samples. Consequently an unavailable list/total
speed does not establish that transport has not started. Open work is to expose
those measured rates promptly and assess incremental source admission while
preserving source-change detection and immutable recovery identities. No live
timings were collected, so these findings do not attribute a specific user's
elapsed wait to disk, hashing, target validation or Telegram latency.

The [upload performance proposal](UPLOAD_PERFORMANCE_PROPOSAL.md) separates
ACK-derived live rates from historical averages, specifies bounded presentation
updates and preparation measurements, and outlines versioned look-ahead admission
and a qualified stable-source fast path. It is proposed only; the current full-file
admission and delayed list/total speed behavior remain unchanged.

## Automatic device keys and optional application PIN (2026-09-15)

Private-channel management now triggers automatic OS-random key preparation on
the retained key owner. The macOS system Keychain protects the existing
self-contained recovery bundle, and restart loads the key automatically. The
optional application PIN only gates the desktop interface; setting, changing or
removing it leaves file keys and ciphertext unchanged. Files, uploads, transfer
metadata/search and settings have no separate Vault password or unlock gate.
Preparation, phase duration, last activity, cancellation and terminal results stay
visible across navigation. Keychain failures expose retry/recovery guidance while
pages and the upload picker remain reachable.

Recovery viewing/export, recovery rotation and explicit new upload epochs remain
available. Viewing a bundle does not restart synchronization or invalidate file
projections. Importing a recovery bundle into an already initialized installation
adds a retained historical key without replacing its active upload key. Keychain
write/readback precedes SQLite commit; failed writes, competing commits,
cancellation and stale logout callbacks preserve committed key authority. Secret
job results redact Debug and zeroize on drop. The explicitly requested incompatible
access-model update assumes no legacy data and adds no legacy-password migration
wizard. Unexpected records are preserved and can use recovery bundles. SQLite 19,
wrap/recovery-bundle/manifest v1 and part-container 2.0 contracts are unchanged.
See [ADR 0040](adr/0040-automatic-device-keys-and-optional-pin.md).

Validation: all workspace targets passed (751 tests; 10 existing manual/performance
probes ignored), followed by affected Runtime/GUI regression reruns (268 and 164
passing tests). Eight new device-store regressions cover no-PIN setup, repeated
admission, restart/file-key preservation through PIN changes, failed/unverified
saves, missing/corrupt entries, recovery rotation/import, competing commits and
controlled blocked read/write cancellation/logout. UI checks cover acknowledgment
before admission, stale completion, direct page/picker access during key preparation
and transfer metadata/search. Test-owner readiness deadlines allow 30 seconds for
CSPRNG/KDF preparation under compilation load; the behavioral ordering uses explicit
channels, with no speed assertions. Formatting, workspace check, strict Clippy,
Core/i18n and warning-free rustdoc gates passed. Cargo-deny passes after updating
the existing Rustls dependency to 0.23.45 for RUSTSEC-2026-0285; license/source policy
is unchanged. The existing `block` future-compatibility notice remains.

English/light synthetic native previews covered 900×600 and actual full-screen:
optional PIN settings, directly accessible Files and upload, recovery controls and
independent scrolling, plus actionable Keychain-failure feedback without a page
lock. No real Telegram account/session or Keychain entry was used or modified.
The system adapter is macOS-only; other desktop platforms remain unqualified.

## Completed download reservation cleanup (2026-09-15)

Native downloads now remove the empty `.partial` destination reservation after
publishing their final output. Previously native transport cleaned its separate
hidden partial/bitmap but left the allocation marker behind. Only regular empty
markers with present, regular outputs of the expected size are eligible; nonempty
partials, symlinks and incomplete/recoverable files remain untouched. Completion
cleanup stays on the download worker, outside snapshot locks. Cleanup failures
retain the successful user file and are retried against restored completed history
by a retained reservation-cleanup worker at startup. The pass covers up to 10,000
restored completed records independently of startup, SQL, new downloads and explicit
cancellation cleanup, without directory walks or periodic polling. Late completion
callbacks cannot overwrite cancellation received during filesystem cleanup.
Paused tasks retain their reservation and resume data. No schema or codec changes.

Validation: 260 Runtime, 79 Telegram and 59 Transfer tests passed (398 total; five
existing manual/performance probes ignored). Regressions use real temporary files,
the managed destination allocator, fake transport, injected cleanup failure and
controlled blocking. They cover publication cleanup, restart repair without
redownload, preservation of paused recovery/outputs/symlinks, worker isolation and
late cancellation. Formatting, affected-crate strict Clippy and the debug app build
passed. No live Telegram download or cleanup of the user's existing files was run.

## Sync-status and unified activity history (2026-09-15)

The lower-left synchronization status is clickable again in the authorized, unlocked
workspace, including Synced and preparation states. It opens the existing merged
activity inspector without starting synchronization work. The application-locked
status remains display-only, locking closes open details, and handlers recheck
current authorization/lock state to reject stale clicks. Background activity and
history continue across locking. Private-channel edit/delete/gap events appear once
in the global Recent activity timeline, ordered together with other messages. The
separate Private channel changes disclosure is removed; viewing events requires no
encryption-key unlock. Review remains available above the timeline, new events
appear without reopening it, and the empty state considers all sources. No
persistence or runtime contract changes.

Validation: 163 GUI tests and 21 i18n/catalog tests passed; three existing manual
GUI performance probes were ignored. Formatting, affected-crate strict Clippy and
the debug build passed. English/light synthetic native previews covered 900×600
and actual full-screen mode: Synced opens/closes details while the application is
unlocked and remains read-only while locked. Private events are visible with an
unavailable encryption key, and the compact inspector scrolls independently. The
`channel-sync-private` and `channel-sync-synced-locked` preview states reproduce
these cases without opening real account data or contacting Telegram.

## Event-driven session revocation (2026-09-15)

Connection/authentication-error events now request one home-DC confirmation; idle
sessions do not poll. Confirmed session loss blocks new work, pauses native and
encrypted owners independently, saves recoverable progress, retires the invalid
connection/session and returns to sign-in. The lower-left reason survives the fresh
QR flow and clears after successful login. Failed pause or cleanup remains blocked
and retryable. Old account/connection callbacks cannot restore or erase a new login.
Existing task pause intent survives sign-in and restart; no stored format changes.
See [ADR 0039](adr/0039-event-driven-session-revocation.md) for event semantics,
retained ownership, failure handling and compatibility.

Validation: all workspace targets passed (734 tests; 10 existing manual/performance
probes ignored), plus the separate Core and i18n/doc-test gates. Formatting, full
workspace check, strict Clippy, warning-free rustdoc and the debug build passed.
Cargo-deny passed all policy gates with existing duplicate-dependency warnings;
the existing `block` future-compatibility notice remains. English/light synthetic
native previews covered 900×600 and actual full-screen, expanded activity details,
retry back to sign-in, retained lower-left reason and blocked Settings access.
No live phone-side Telegram session revocation was performed.

## QR login recovery and signed-out access (2026-09-14)

A failed QR poll now retires the rejected token and its poll owner, preserves the
typed error on the login page, and immediately starts a fresh QR export. The
replacement code resumes normal polling; a successful second scan enters Storage.
A failed replacement exposes Retry without an automatic request loop. Login method,
proxy and account generations reject stale replies, including code/password replies.

Without an authorized Telegram account, the main window exposes only sign-in and
the proxy editor. Settings, About, workspace navigation, upload/search shortcuts and
native workspace menus are gated. Network-error actions open the same proxy editor;
the optional Telegram API setup remains part of sign-in, without exposing Settings.
Existing application PIN protection remains independent. No persisted format changes.

Validation: 155 GUI tests and 21 i18n/catalog tests passed; three existing manual GUI
performance probes were ignored. Affected-crate strict Clippy, formatting and the
debug build passed. English/light synthetic reviews cover 900×600 and actual native
full-screen login, error/replacement QR and proxy editing, including compact scrolling
and blocked workspace shortcuts. `--preview-state=login-refreshing` and
`login-refreshed` reproduce the recovery presentation with synthetic tokens.
Real phone cancellation and live Telegram rescanning were not exercised.

## Application PIN and uninterrupted background work (2026-09-14)

Application Lock now replaces the whole workspace with a PIN sign-in page and
retains synchronization, encryption-key sessions and transfer owners. PINs are
optional, independent of file encryption passwords, and installation-wide.
Locked access is limited to sign-in, account switching and proxy configuration;
auxiliary transfer windows and workspace shortcuts cannot expose the workspace.
Quit/Close now offers **Pause and Quit** and waits for durable pause requests and
writer settlement. Saved queues, including non-visible jobs, remain paused after
restart. Preview closes immediately. Account switching and proxy application keep
the cancellable transfer drain. Native platform shortcuts share the same close/quit
gate and closing a batch window leaves background work running. See
[ADR 0038](adr/0038-native-window-close-and-paused-exit.md).

The bottom synchronization status is display-only. Network settings opens a
merged timeline containing directory, ordinary/private channel, manifest and
private-change events with fixed timestamps and revision-based rebuilding.
See [ADR 0037](adr/0037-application-pin-and-transfer-drain.md).


## Streaming uploads and manual concurrency (2026-09-14)

The production Vault upload path now encrypts one wire-aligned block at a time,
reuses bounded buffers, concurrently saves parts and accepts locally sealed
content plus Telegram publication receipts without routine content read-back.
Default concurrency is three files × ten 512 KiB parts, with two shared upload
connections and a two-block ready queue. The settings UI exposes bounded manual
upload/download limits and AES256-GCM CPU capability; old profile/advisory keys
are retained only for compatibility. Native/Vault downloads share the file gate
and use selected part/connection/attempt values.

Part/manifest readers support 1.0 and 2.0; new writes use 2.0, while wraps, recovery
bundles and local recovery envelopes stay v1. Recovery replays sealed ciphertext;
partial/corrupt local preparation advances the saved instance before reencoding.
At 24 hours, or for unfinished v1 uploads, automatic restart replaces package/File
Key transactionally; sealed manifest outboxes can finish from existing bytes.
The upload inspector shows acknowledgement-rate intervals, 512 KiB part states,
retry waits and bounded replay with omission counts and sanitized logs.

Focused evidence includes ten concurrent RPCs, three blocked file uploads and
work-conserving refill, zero normal content read-backs, immutable spool replay,
lost-spool instance retirement, old/new frozen crypto fixtures and transactional
restart rollback. All eight source gates passed for the working tree (702 tests)
and the isolated task snapshot (673 tests); each excludes ten existing manual
performance probes. Final GUI/strict Clippy checks cover the layout cleanup.
Native English/light review covers 900×600 and actual macOS full-screen, manual
limits, AES-GCM capability, the 24-hour guide and independently scrolling upload
activity. Credentialed Telegram throughput/multi-DC tests are not implied.
See [ADR 0036](adr/0036-streaming-upload-and-manual-concurrency.md).

## Vault upload preparation metadata fix (2026-09-14)

Vault uploads no longer fail solely because filesystem metadata-change time changes
while queued, hashing, sending or resuming. Source path, native file identity, size
and content modification time remain checked, together with the complete source
hash, immutable part digests and authenticated remote verification. Existing v1
recovery bytes and reservations remain unchanged. Previously blocked jobs retain
their terminal state; a newly selected upload uses the corrected preparation path.
Synthetic regressions cover metadata-only updates during preparation/transport and
restart, authenticated download of the original bytes, and same-size modified content
with restored mtime being rejected before any publication or reservation replacement.

Validation: all eight source gates passed with 683 workspace tests and 10 existing
manual probes ignored. The isolated task passed formatting, strict Runtime Clippy
and 228 Runtime tests (2 existing manual probes ignored). The metadata-only native
regression fails with the original code and passes with this fix. No live Telegram
uploads were performed during verification.

## Automatic Vault sync after unlock (2026-09-14)

Unlock remains a local key operation. Its success is preserved while the managed
file projection downloads and authenticates manifests in the background. Transient
manifest failures retry automatically with delays from 2 to 60 seconds and no
attempt cutoff; Telegram-requested waits retain their full duration through the
object adapter. The global inspector shows the waiting reason, scheduled wait,
and cancellation. Authentication, permission, missing-source and persistence errors
remain terminal and actionable. Navigation retains synchronization; cancellation,
account replacement and key/session changes fence further attempts and late results.
Backoff runs outside the Vault workers, preserving independent key, scan and transfer
operations. No persistent schema, encryption or recovery codec changes are required.


## Durable transfer recovery delivery (2026-09-14)

Uploads and encrypted downloads now retain account-scoped recovery context, verified
part/extent checkpoints and generation-fenced pause/cancel/retry state. Eligible
queued work resumes after restart and unlock; explicit pause, cancellation and
failures retain their intent. Upload selections are admitted atomically before
execution. Native cancellation persists cleanup and retry obligations, with a
navigation-independent waiting/failure entry and retained background owners.

Finalization reuses verified published output, reconciles ambiguous upload receipts
and permits a saved manifest to finish without reopening the original source.
Connection replacement cancels retained work; final hashing checks cancellation
between reads. Native and encrypted downloads claim distinct partial names during
allocation. Schema 22 indexes batch stop and prioritizes active/recoverable history
before the bounded visible limit. Unknown metrics stay unavailable, and concurrency
settings are labeled as limits. First-use and task-specific guidance explain what
can resume and when a new upload is necessary.

Delivery checks: the isolated task snapshot passed all eight source gates with
645 workspace tests; 10 pre-existing manual performance probes were not executed
and are not counted as passes. Its native English/light previews covered 900×600
and actual macOS full-screen for cleanup status, independently scrolling recovery
guidance and batch/list geometry. The cleanup entry remained reachable after
navigation. Synthetic blocked-transport, account-replacement, concurrent output
allocation and abrupt-child-process recovery tests passed.

This entry supersedes the earlier incremental “incomplete” notes for these paths.
The implementation and validation scope are documented in
[transfer recovery](VAULT_TRANSFER_RECOVERY.md). Product artifacts remain macOS-only;
ordinary verification uses temporary files and synthetic transport, not private
Telegram sessions. Blocking individual OS calls, hostile concurrent filesystem
replacement and hardware power-loss behavior are not claimed by the automated
process-restart evidence.

## Compact channel interface and macOS Actions repair (2026-09-13)

Channel controls now use a single 48-point heading and 26-point toolbar buttons, with filters expanded on demand, stable table position during selection and a small right-aligned Earlier history action. The 28-point global footer shows concise sync state at the left, a small adjacent lock/background notice, and disk space plus transfer rates at the right. Distinct active-channel counts, queued/waiting states and failures stay truthful; unchanged renders reuse revision/account-scoped rate summaries. The inspector displays the fixed seven-minute delivery-silence recovery policy. No synchronization timer or navigation-triggered work was added. See [ADR 0034](adr/0034-compact-channel-and-shell-status.md).

CI and Release now use native pinned cargo-deny on macOS instead of an incompatible Docker action. Checkout/artifact actions use Node 24 releases. Fuzzing explicitly selects nightly despite the stable repository override, and missing crash files no longer add a secondary artifact warning. Release publication explicitly binds the repository, and its checksum file uses portable basenames. Product artifacts remain macOS-only; the Linux fuzz and publication jobs do not implement another desktop platform.

Validation: all eight source gates passed with 550 workspace tests and 10 existing manual probes ignored. The final GUI/i18n runs passed 128/21 tests; actionlint passed. Local execution reproduced the old stable-toolchain fuzz failure; fixed AddressSanitizer campaigns completed 4,530,083 part-parser executions and 5,444,624 manifest-parser executions in 31 seconds each without a crash. These are short local campaigns, not the scheduled ten-minute Linux runs. Native synthetic English 900×600 and actual full-screen light/dark reviews passed, including selection geometry, filters, the seven-minute policy, waiting state and independent inspector scrolling. The scoped optimized build and the exact Release archive step passed locally, including basename-only checksum verification and app binary/version/icon/license checks; the fresh unsigned `dist/TeleArk.app` replaced the previous bundle, which was retained. Hosted GitHub runs remain unverified: these working-tree edits have not been pushed and the available browser/API session cannot access the private repository's logs.

## Fixed timestamps and navigation-independent synchronization (2026-09-13)

The synchronization inspector now uses fixed event/last-success timestamps with no live counters or presentation timers. Channel/TeleArk navigation sends no synchronization command, preserves managed work/results, and never mounts a remote history request. Runtime no longer short-polls the selected/private channel; only actual update/recovery events and a navigation-independent 7-minute delivery-silence check remain. Normal unchanged silence responses do not write or refresh the inspector. See [ADR 0033](adr/0033-static-sync-inspector-and-navigation-independent-updates.md) for the precise recovery exception, layout, account scope and compatibility.

Validation: all eight source gates passed, including 546 workspace tests with 10 existing manual probes ignored. Deterministic regressions cover zero idle inspector notifications over a simulated hour, fixed timestamps, independent history scrolling, navigation retaining account-owned work and cached rows, stale completions, failure recovery and delivery-silence deadlines. Final synthetic-fixture cache initialization also passed 124 GUI tests (3 existing ignored), formatting and strict affected-crate Clippy. Native English 900×600 and actual full-screen light/dark previews verified the hierarchy, independent scrolling, retained timestamps across navigation/reopen and reachable private-change review controls. The scoped optimized build and fresh unsigned macOS package passed binary/resource checks. No real-account delivery or end-to-end CPU improvement claim is made; completion timestamps are retained for the current account session, not across app restarts.

## Earlier event-driven status presentation (2026-09-13; superseded by ADR 0033)

Historical validation: English native 900×600 and actual full-screen light/dark previews passed, including independent history scrolling, reopen timing and retained accessibility controls. Deterministic tests verify notification/render counts, timer lifetime, quantization and terminal freezing. All eight source gates passed for the working tree (540 tests) and isolated commit snapshot (511 tests), each with 10 existing manual probes ignored. One working-tree blocked-transfer test timed out during concurrent verification; its focused rerun and subsequent full workspace run passed. The scoped release build and freshly assembled macOS app passed binary/resource checks. A 10-second-per-state synthetic dark full-screen process sample with accessibility active measured 0.10 CPU seconds closed and 0.32 CPU seconds open (approximately 1.0% and 3.2% of one CPU core); this is not a before/after comparison or real-account performance claim.

## Batch transfers start collapsed (2026-09-12)

Channel and Remote Library batch-download completion callbacks no longer expand newly queued groups. The transfer list initially shows the batch summary; existing user-controlled expansion, large-batch windows and search/status member discovery remain. No persisted format changes. GUI tests passed (118 passed, 3 existing ignored), including initial header-only projection and manual expand/collapse; strict GUI Clippy and debug build passed. Native English 900×600 light preview confirmed collapsed upload/download groups. Actual full-screen and dark native checks remain unverified because the UI automation service repeatedly timed out, including reconnect attempts; large/default windows are not counted as substitutes.

## Automatic account synchronization (2026-09-12)

Runtime now starts channel synchronization at account readiness, owns initial directory discovery, and rebinds automatically after connection replacement. Bounded independent execution prevents a blocked channel or later metadata query from stopping other channels. Transient failures retry automatically with capped delays; FloodWait and actionable access/persistence errors remain visible without discarding cursors or cached data.

Committed directory revisions update sidebar titles/membership, material file changes update the Remote Library, and private metadata changes recheck storage state. Scrolling requests one cancellable 200-message history page; navigation cancels history demand while live synchronization continues. Routine refresh buttons, the duplicated sync footer and Cmd-R are removed. The global inspector shows active work, queues, durations, failures/retries and its retained timeline across navigation. Three semantic catalogs are synchronized; UI verification uses English only.

A version-1 settings cache restores the last committed channel directory after account readiness. Older installations use existing channel cursor/source rows automatically; unsupported/newer cache bytes remain intact. SQLite stays at schema 16 and encrypted/transfer formats are unchanged. See [ADR 0031](adr/0031-automatic-account-synchronization.md).

Validation: all eight source gates passed on the final working tree, including 537 workspace tests with 10 existing manual probes ignored, strict Clippy, warning-free workspace documentation and dependency/license checks. Deterministic coordinator tests cover blocked independent operations and connection replacement with stale-response rejection; GUI tests cover automatic library replacement and retained selection. Native synthetic English-only 900×600 light and default-size dark previews verified visible wait reasons/timing, independent inspector scrolling, removed routine refresh controls and global status retained across navigation. No real account/network delivery claim is made.

## Upload and download speed limits (2026-09-10)

Transfers and both upload/download Settings sections now expose a shared limit editor, with independent totals in KiB/s and zero for unlimited. Saving updates active transfers and persists automatically across restarts. Native files, encrypted parts, manifests and verification reads share the appropriate directional budget; route replacement retains the same owners. Waiting remains visible across navigation, with duration, last payload admission and a bounded, expandable event timeline. Invalid input is rejected and failed saves retain the previous live policy. Existing settings upgrade through additive version-1 defaults; no encrypted or checkpoint format changes.

Limits pace payload admission, allowing bounded bursts and already-admitted transport buffers; they are not an exact socket-rate guarantee and exclude protocol framing/internal retries. See [ADR 0030](adr/0030-shared-transfer-speed-limits.md) and the [transfer contract](TRANSFER_ENGINE.md#aggregate-upload-and-download-speed-limits). Validation: full source gates passed for the current worktree (524 tests) and independently built commit snapshot (508 tests), with 10 existing manual probes ignored in each. Final pacing and modal-scroll fixes passed a fresh current-worktree full run plus isolated GUI 106 tests and strict affected-crate Clippy. English-only UI regressions cover 900×600/default windows and both themes; native English compact light/default dark previews verified editing, saved feedback, wait history, pinned actions and scroll isolation from the background list. The local release build and fresh unsigned macOS package passed binary/resource checks. No live Telegram transfers or socket-rate benchmark was performed.

## Clear folder/application-bundle upload rejection (2026-09-10)

Upload selection now distinguishes directories (including `.app` bundles and directory symlinks) from missing files. The frontend-neutral admission error displays an explicit localized instruction to compress the folder into ZIP, fixed above the upload composer’s scrolling content. Invalid selections preserve the existing draft; ordinary files with an `.app` suffix remain allowed. File selection validation still runs in the background. This error is returned before task admission/persistence, so no schema or persisted codec changes are introduced.

Validation: all eight source gates passed for both the current worktree (510 tests) and isolated commit snapshot (494 tests), with 10 existing manual probes ignored in each. The new picker regression verifies draft preservation, explicit reason, valid retry, localized mapping and pinned error/footer bounds in all three locales, both themes and 900×600/default windows; the current worktree’s existing native-drop regression also asserts the new directory error. Native synthetic previews checked English compact light/default dark plus dedicated Chinese compact light/Japanese compact dark wrapping. The folder message and footer actions remain fully visible. Local release build and fresh macOS packaging passed; no live uploads were performed.

## Default window fits the desktop work area (2026-09-10)

Startup uses GPUI Kit’s OS-reported visible display bounds, excluding the menu bar and Dock/taskbar, instead of fixed screen-edge estimates. The default content size is 1120×680. A separate native-titlebar allowance and outer margin keep the full frame inside the work area, and small work areas lower the initial minimum rather than forcing overlap. Explicit preview sizes remain supported and fit the same bounds.

Validation: current-worktree GUI 113 and i18n 21 tests passed (3 existing manual GUI benchmarks ignored), with formatting and strict affected-crate Clippy. The isolated commit snapshot passed GUI 102 and i18n 21 tests and the same checks. Native English default and 900×600 light/dark previews, plus an oversized 1920×1080 request, were visually reviewed. Actual native frames including titlebars stayed inside the OS work area; bottom status bars remained visible. Local release build and fresh macOS packaging passed. No runtime, schema, locale-message or dependency changes.

## Upload history after restart (2026-09-10)

Transfers → Uploads now restores durable account-scoped task summaries and batch identity from SQLite schema 16, including while locked and before remote catalog access. Queue/start/terminal writes run on retained background owners; completion waits for persistence acknowledgment. Unfinished rows become explicit interrupted history, with instructions to check Storage and choose the original source for a new upload. Bounded recent views retain whole boundary batches and disclose omitted rows. Restored telemetry is labeled unavailable; existing sanitized session logs remain accessible. Automatic schema 0–15 upgrades preserve data/key wraps and retry safely after rollback. No encrypted automatic resume, old upload-page browser or reconstruction of already-lost pre-v16 RAM-only rows is claimed. UI tests/previews now default to English by contributor rule.

Validation: 506 workspace tests passed, with 10 existing manual probes ignored; formatting, all-target compilation/strict Clippy, Core/i18n, warning-free rustdoc and dependency checks passed. Final English 900×600 light/dark previews show restored batches and interruption guidance without scrolling. The isolated commit snapshot also passed all source gates and 490 tests; remaining pre-existing worktree changes are outside that snapshot.


Updated 2026-09-10. TeleArk targets a mature desktop product with Telegram/native-download and encrypted private-channel workflows. This record distinguishes implemented capabilities, validation evidence and remaining product work.

## Channel row selection visibility (2026-09-10)

Table selection now uses a translucent blue overlay (12% light, 20% dark), preserving filenames, icons and checkboxes. The previous opaque theme override covered the cells because GPUI Kit paints its selection layer above their content. Row focus/details and checkbox bulk selection remain independent. The `channel-selected` synthetic preview exposes a focused, checked row for visual review.

Validation: the new GPUI regression fails with the old opaque color and passes with the fix. It checks the painted selection token, composited normal-text contrast, stable row geometry, mouse/keyboard selection and independent checkboxes across three locales, both themes and 900×600/1360×760. GUI 108 and i18n 21 tests passed (3 existing manual benchmarks ignored); strict affected-crate Clippy, formatting and the debug build passed. Native synthetic Chinese light and Japanese dark previews were visually reviewed at default and compact sizes. No runtime, data-format or Telegram behavior changed.

## Storage repair cards and locked layout (2026-09-09)

Channel identification repair now has a highlighted card with the specific missing/invalid/unpinned reason, the exact identification-message/pin/description operations, and an explicit confirmation of description replacement. The card identifies the current channel by its unchanged name and ID, explains saved account-specific ID lookup versus initial identification discovery, and describes the explicit one-time mute/archive action and later Telegram preference changes. Restoring identification does not silently mute or archive. When channel health is normal and no work, retry, failure or confirmation is pending, this card defaults to its connected-title row only. Selecting the title expands the retained details; new work resets that manual expansion, stays visible while active and automatically collapses after successful completion. Other channel actions have a separate footer. Stale discovery-success notices no longer display a contradictory Complete card. The copy distinguishes identification repair from file-data recovery and avoids claiming that locked files are verified safe.

Locked content uses a bounded-width, naturally scrolling card column. The compact lock card has intrinsic height and a fixed-size icon; the unlock action stays inside its border. Unlocked file views retain a bounded overview and an independently usable file region. About release text is synchronized with the canonical changelog. No runtime, schema, encryption or Telegram mutation behavior changed.

Validation: 583 workspace tests passed (10 existing manual probes ignored), full check/strict Clippy/rustdoc/cargo-deny/formatting passed. The new GPUI regression checks child containment, nonshrinking icon size, review/confirmation/unlock actions and scrolling at 900×600 and 1440×900 for all three locales and both themes. Actual synthetic native previews also covered all six locale/theme combinations at 900×600 and macOS fullscreen. The unsigned local macOS app was rebuilt and packaged; no real channel was modified. The follow-up location/notification disclosure passed all 99 GUI and 21 i18n tests plus strict affected-crate Clippy. Its scroll regression now measures the actual viewport instead of the scrollbar component’s intrinsic content, and asserts that the viewport itself remains inside the window. Chinese native compact/fullscreen inspection confirmed the longer text and reachable bottom controls. A subsequent completed-repair regression exposed an underestimated auto-height timeline viewport that clipped the recheck/archive footer. The timeline now has an explicit 104-pixel scroll viewport; the regression covers completed history, full button containment and archive confirmation in all locale/theme/size combinations.

## Session unlock and uninterrupted admitted work (2026-09-09)

Vault unlock lasts for the account session across navigation, window inactivity and file selection. Explicit lock immediately hides decrypted names/paths and clears secret input, while admitted uploads, downloads, queued batch members, synchronization and internal retries retain their task keys and continue. New operations need a fresh unlock. Browsing unlock stays on the current page; upload unlock returns to the draft for explicit confirmation. Ordinary unlock has one password field. [ADR 0026](adr/0026-session-unlock-and-task-key-leases.md) records bounded owners, task key leases, stale-callback rejection and the deprecated preference field. Schema 15 and encrypted/recovery codecs are unchanged.

Validation: 582 workspace tests passed (10 existing manual probes ignored), including blocked transfer/scan isolation, queued key leases, key-reference release, stale unlock callbacks, historical-key restrictions, draft preservation and locked presentation. Full check, strict Clippy, warning-denied rustdoc, cargo-deny, formatting and diff checks passed. Synthetic native previews verified lock/redaction, retained progress, the single-password dialog and reachable cancellation at 900×600 and actual macOS fullscreen for all three locales and both themes. All preview windows were closed. Release was rebuilt using the scoped local build script and packaged as an unsigned macOS app; no live Telegram task or biometric interoperability was exercised. The tagged source includes the preceding runtime, synchronization, proxy and recovery dependencies required by this implementation.

## Fixed-channel recovery and retained key versions (2026-09-08)

An existing account binding keeps its original channel. Missing management information is repairable independently of file contents; an explicit confirmation repairs the identity record, pin and description pointer after fresh ownership/privacy checks. Missing access and unsafe configuration remain typed blocking errors. Source-file permissions are reported separately from destination failures. Mute/archive is an explicit one-time peer operation and respects later Telegram preference changes. [ADR 0025](adr/0025-fixed-channel-and-retained-key-epochs.md) supersedes the earlier intact-marker and single-local-key restrictions.

The encrypted manifest inventory preserves authenticated envelopes across remote deletion. File health distinguishes unchecked, indexed presence, missing parts, missing manifest, unavailable key and invalid manifest. A cancellable independent owner checks remote messages in bounded batches and keyset-pages retained inventory; partial successful observations persist without treating unexamined files as missing. Missing parts affect only their file. The GUI displays health beside the filename and in an independently scrolling inspector, with recheck and local-copy upload actions. Revisioned copy-on-write catalog projections reuse unchanged rows and selected-file lookups during idle rendering.

Schema 0–14 automatically upgrades to read/write 15, preserving old wrapped keys and encrypted inventory through transactional migration. Explicitly creating a new key version preserves old wrappers and ciphertext and uses the same channel for future uploads. An original recovery bundle can unlock a retained historical version without replacing the current upload key, subject to retained recovery-rotation revocation. Existing manifest and recovery bundle v1 readers remain supported. Files deleted before successful inventory enrollment cannot be reconstructed; losing every original unlock path still makes old ciphertext unrecoverable.

Validation: 573 workspace tests passed (10 existing manual probes ignored), covering migration chains/rollback, atomic epoch switching, authenticated historical-key routing, file-local loss, cancellation, stale presence updates, blocked health-owner isolation and projection reuse. Workspace check, strict Clippy, warning-denied rustdoc and cargo-deny passed. Native synthetic previews covered repair, mute/archive confirmation and new-key confirmation in all three locales and both themes at 900×600 and actual macOS fullscreen; inspector scrolling and primary actions were checked. Preview review also fixed an initial-frame startup stall and health text layout. No live-account repair, mute/archive or key-change qualification was performed. At that checkpoint this revision was reviewed as a debug preview; the subsequent session-unlock checkpoint above rebuilds and packages the cumulative implementation.

## Fail-closed proxy settings (2026-09-08)

Settings → Network now supports SOCKS5/HTTP CONNECT with numeric IPv4/IPv6 endpoints, optional authentication, apply-and-test, retest and cancellation. The configured route covers every Telegram pool/datacenter and reconnect; independent framework HTTP is blocked. Proxy failures remain globally visible across navigation and never enable direct fallback. Applying replaces the complete old socket owner before activating the new route. Explicit disable-and-apply is the only route back to direct access. The bounded timeline shows phases, elapsed time, last network event and omitted history in all three locales.

Schema 0–13 automatically upgrades to read/write 14; independent network-policy codec 1 is validated before network startup, and unknown/corrupt records fail closed. Existing encrypted/recovery formats and keys retain their meaning. [ADR 0024](adr/0024-fail-closed-proxy-routing.md) records the ownership, compatibility and test contract. Loopback tests include reachable direct-destination traps, real grammers MTProto bootstrap traffic, proxy failure modes, old-socket shutdown, persistence failure/restart, diagnostic cancellation and framework HTTP blocking. Real-account proxy interoperability, hostname/TLS-proxy support and full platform/accessibility qualification remain outside this revision's evidence.

Validation: all eight development gates passed, including 455 workspace tests (10 existing manual probes ignored), explicit Core/i18n tests, strict Clippy, warning-denied rustdoc and cargo-deny. Debug app build passed. Native proxy previews covered Chinese/Japanese/English, light/dark themes, 900×600 scrolling and a display-fitted 1360×760 English layout; apply/retest/cancel controls and the global failure banner were checked, including navigation to Library. No credentialed proxy interoperability claim is made.

## v0.4.4 background state cards

TeleArk setup separates explanatory content, the current background task and its state history. A shared GPUI Kit card header identifies the task with an icon and localized phase badge; the latest response has a separate inset panel, and timing/attempts sit beside the primary action in a distinct footer. State changes use separated timestamped rows, newest first, with the latest three shown initially and an explicit expansion control for the retained current-run history. The independently scrolling inspector uses the same cards. Private-storage discovery, retry and completion use the shared card treatment as well.

This is a presentation change: request generation, cancellation, retry limits, account routing and database/crypto/session/checkpoint formats keep their existing behavior. The v0.4.4 package includes the earlier local projections and performance work recorded below. At that checkpoint, database upgrades were schema 0–12 to read/write 13; the proxy revision below supersedes this with schema 14; existing encrypted and recovery readers remain available. The catalog RPC 500 incident is still an open availability issue: a subsequent old/current/old read-only comparison also reproduced RPC 500 with the pre-performance baseline, without identifying the original trigger.

All eight required source gates passed, including 426 workspace tests (10 manual probes ignored), strict Clippy, warning-denied rustdoc and cargo-deny. Deterministic GPUI checks cover six task phases across all three locales and both themes at 900×600, separated response/actions/history, primary-action visibility, inspector scrolling and history expansion without changing the active task. Native preview inspection confirmed Chinese and English light-mode cards at 900×600, the Chinese inspector and Japanese dark-mode cards; native capture routing intermittently lost its window, so this is not a complete native size/accessibility matrix. The optimized binary and unsigned macOS bundle were rebuilt at version 0.4.4 with matching binary hashes. External dependency versions and checksums are unchanged.

## v0.4.3 upload activity

Upload progress previously advanced only after an entire encrypted application part (up to 60 MiB) uploaded and passed remote readback verification. The GUI therefore displayed 0% during real transfer, and its refresh signature ignored phase-only changes. Typed memory-only activity now identifies storage discovery, destination checks, reading/encryption, Telegram queue wait, byte streaming, message confirmation, readback verification and final manifest publication. Rows, collapsed batches and a persistent status line show actual stages; the status line/details include elapsed time and current object bytes. The UI uses transport-read counters for in-flight progress while retaining verified bytes/parts separately. This includes transport buffering, not server acknowledgement; only successful finalization displays 100%. No fixed startup-time or measured-throughput claim is made.

The release includes the pending account, Library, private-channel management and bounded-preflight changes described below. New deterministic coverage checks partial stream reads before completion, verification/manifest phase boundaries, no late updates after failure, preflight visibility, running-member selection in collapsed batches and no premature 100%. Passed all required development quality gates, including strict Clippy, 335 workspace tests, explicit Core/i18n tests, warning-denied rustdoc and cargo-deny. Debug and optimized Release builds and the unsigned macOS bundle pass; 520 static GUI message references and 89 local documentation links/anchors resolve. Actual isolated 900×600 windows were inspected in Chinese, English and Japanese dark mode; the active phase, object bytes, elapsed time and unknown-speed dash remain readable, including Japanese upload details. GPUI covers all eight phases, collapsed-batch selection, failure cleanup and completion capping. Full VoiceOver/backing-scale qualification remains outstanding. Required protected follow-up: first/repeated small-file and 60 MiB-plus uploads on a real account with many channels and a slow network; confirm byte activity, readback, publication, batch stop and disconnection behavior. No real credentials or user documents are used by source tests/previews.

## Current capabilities

| Area | Connected behavior | Remaining limits |
| --- | --- | --- |
| Desktop | GPUI Kit 0.6.0 / gpui-pre 0.3.3, Apple-style palette, collapsible icon navigation with direct Library access and a resizable contextual channel list with saved width, virtualized channels/raw/managed/variable-height transfer lists, independently scrolling inspectors, visible download-disk free space, progressive settings, native menus/shortcuts, localized full About changelog | Full pixel/backing-scale and assistive-technology matrix outstanding |
| Account | Automatic centered QR with secondary phone/code/2FA, restored avatar/name welcome, explicit Log In and sign-out confirmation for Switch Account, optional personal/distributor API configuration | Platform credential adapter disabled; credentialed end-to-end qualification outstanding |
| TeleArk storage | Remote-authoritative discovery/verification and automatic setup of one eligible owned private channel, pinned identity/warning, shared message catalog for Files/Raw Files, persistent edit/delete observations visible while locked | No automatic migration/deletion of legacy Saved Messages; legacy recovery stays in advanced Key Vault settings |
| Vault | Retained bounded owner, password/recovery setup/unlock, password change, recovery rotation, explicit bundle export/database-loss restore, session unlock with explicit lock and retained task authorization | OS Credential unavailable; exported old bundles cannot be cryptographically revoked; sleep/logout qualification outstanding |
| Unlock flow | Focused modal preserves browse/upload/download intent and resumes after successful unlock/recovery acknowledgement; cancellation clears secret inputs | Actual login/key workflows require protected credentialed review |
| Native browsing/index | Local-first Channel projection, account-owned contiguous push commits and PTS difference recovery, per-channel GUI deltas, edit/delete updates, restartable gap repair, explicit older history, visible phases/queue/timeline; up to 5,000 virtualized rows | Generic policy/range Index coordinator remains separate; full-history coverage and live-account qualification outstanding |
| Native transfers | Durable account-scoped tasks/batches, bitmap missing-part resume, pause/resume/cancel/retry, terminal deletion, explicit new-task re-download, local-file presence checks, adaptive P, bounded retries/shared FloodWait, live/replay session logs | Exact byte-count verification only; no cryptographic content hash, physical multi-connection/DC pool or bandwidth control |
| Encrypted transfers | Real single/multi-file upload with review, queued members and stop-after-current-file; live phase/byte activity and elapsed time; verified parts/final authenticated manifest; authenticated restore with atomic non-overwriting publication and a durable local output inventory | 60 MiB plaintext/8 MiB frames/64 MiB encoded object bounds; memory-only tasks, no durable or mid-file pause/cancel/retry/restart resume, orphan cleanup or empty-file desktop upload |
| Library | Separate Local files / Remote files tabs, independent type filter, cancellable local disk observations over current-account completed downloads and imports, account-scoped remote catalog search/paging, trailing Reveal/Download actions and checkbox-based bulk actions | Paged Library is not virtualized; local view covers retained native/Vault inventory and imports rather than arbitrary directories; million-record performance unmeasured |
| i18n | Synchronized en-US/zh-CN/ja-JP, live locale choice, fallback/negotiation/formatting, semantic errors, About/CHANGELOG parity | Native NSLocale discovery and native-speaker review outstanding |
| Diagnostics | Bounded lossy process JSONL plus durable typed transfer session logs, Settings access, safe performance/failure fields | Index event coverage and long-term session-log retention need further work |
| Crypto/formats | Explicit version-1 codecs and fixtures, bounds/tamper/wrong-key tests, AEAD registry APIs, fake-remote fresh-database recovery | Full restart identity hydration, larger recovery/fuzz evidence and protected real-system recovery remain unfinished |
| Version upgrades | Ordered verified SQLite migrations through schema 13 with a responsive startup window and phase timeline; schema-11/13 disk-full and schema-11/12/13 rollback/restart coverage; existing preference and crypto/manifest/recovery readers | Cross-format conversion, external backup orchestration and full protected migration qualification remain unfinished |

## v0.4.0 compatibility and scope

[ADR 0012](adr/0012-gpui-kit-and-private-storage-channel.md) governs dependency provenance, private storage and account ownership. Schema 9 adds nullable positive account ownership to native tasks. The first configured Telegram connection atomically attributes legacy unknown rows only to its actual restored authorized session, or permanently leaves them unknown if unauthorized. Later logins cannot claim that work. New tasks require known scope; enqueue/resume/retry and actual network execution validate it. Switching pauses/drains native workers and waits for Vault work. Completed user files/history remain intact.

Versioned settings record account-scoped channel binding and the one-time migration decision. Crypto/manifest/recovery/bitmap/session-log encodings are unchanged. Missing/invalid/truncated channel discovery fails closed, new uploads validate current privacy/ownership before touching plaintext/keys, and manifest cancellation is propagated independently from key-operation state.

The permissive GPUI graph and exact license exceptions are reviewed in ADR 0012, `deny.toml` and third-party notices. No license-incompatible external implementation was used. Core/Runtime remain GUI-free; SQL and grammers remain adapter-owned. Three catalogs, structured domain errors and bounded retained owners remain in place.

## v0.4.1 usability scope

[ADR 0013](adr/0013-local-output-inventory-and-upload-batches.md) adds schema 10 for account-scoped restored Vault output paths and projects completed native history directly. A bounded observer distinguishes present, missing, size-changed and unavailable files without rewriting history or triggering downloads. The main status bar independently refreshes available space for the actual download volume. Same-size content replacement is not detected; pre-v10 Vault outputs and a crash before post-publication registration can remain untracked. The in-memory observation cache holds 10,000 recent paths while durable inventory remains complete.

Batch headers show names/source/counts/time, expand independently from inspection and expose actual members. Search and status filters reach collapsed members. Multi-file upload has no fixed selection-count limit. The existing owner schedules bounded internal windows with per-file source/channel checks, typed partial results and cancellation after the current file; see ADR 0028. Upload title and actions stay fixed while its body scrolls. Original application artwork and a native unsigned `.app` bundle are included. Versioned sidebar preferences are additive; previous release tags remain unchanged.

Channel pagination now defers the owner callback until TableState is released, with cancellation and account/chat/generation guards and no automatic retry after failure. A GPUI regression reproduced the reported nested-update panic before the fix and passes afterwards. Local Library query results now expose original channel titles and scoped native message IDs, label unverified remote records as indexed, and explain how browsing populates the catalog. Placeholder part rows were removed; persistent formats and user data are unchanged.

## Library list interaction follow-up

The sidebar toggle now sits at the top left; decorative hash symbols were removed from channel navigation and headings. Library and Transfers list rows retain Reveal without a separate Open File button; Library actions occupy the trailing column. Stable-identity checkboxes and a selection toolbar operate on up to 5,000 loaded actionable files. Local bulk reveal deduplicates parent folders. Remote bulk preparation reserves distinct names across channels, creates one runtime batch per channel off the UI thread, supports cancellation between files/batches, and keeps unqueued selections after partial failure. Changing source/type/search clears selection. Existing runtime account authorization and non-overwrite checks still apply; no dependencies, durable formats or user data changed.

New deterministic GUI tests cover loaded-row selection/filter clearing, per-channel grouping, shared destination reservations, wrong-account rejection, cancellation and partial enqueue failure. Chinese 900×600 preview review confirmed the top-left control, trailing local/remote actions, select-all toolbar and selection reset when switching tabs. Japanese dark display-fitted channel review confirmed titles without hash symbols. Native automation intermittently lost its window/capture handle; successful observations are recorded here without claiming a complete expanded-navigation, keyboard or screen-reader matrix for this follow-up. Live Telegram bulk downloads remain a protected credentialed qualification task; deterministic checks do not substitute for that review.

## Account login interaction follow-up

Switch Account opens a focused modal before any sign-out operation. It explains re-login and retained files/history, has no close control, ignores Escape/backdrop dismissal, and requires an explicit Cancel or Sign out and switch action. Active Vault work remains blocked with feedback in the modal. Successful sign-out returns to QR mode. Fresh unauthorized restoration, entry to the Account page and newly saved API credentials automatically start QR generation when eligible; errors retain explicit retry. The login page centers a larger black-on-white QR code with an underlined 11-point phone-method link below and a retained 34-point click target. Phone switching invalidates outstanding auth generations and stops QR polling; QR expiry refresh remains in the existing runtime owner. No session formats or dependency versions changed.

Three new deterministic GPUI tests cover confirmation/no premature sign-out, ignored Escape/backdrop, cancel/confirm, default QR geometry and method switching, and automatic QR startup after unauthorized restoration. The obsolete two-column login test was removed. All source quality gates were rerun. The native preview exposed Settings and the phone-method link directly, with no QR-generation button; screenshot review was blocked by the locked Mac. Next visual check: unlock the Mac, launch isolated login/returning previews in all three locales, inspect minimum-size and dark layouts, and exercise modal focus/Tab and both method links. Real QR acceptance, phone code and 2FA remain protected credentialed qualification tasks; the preview QR encodes only a non-login demonstration string.

## Automatic private-channel management

[ADR 0015](adr/0015-remote-authoritative-storage-identity.md) makes Telegram the authority for discovery and identity on every device. Local bindings are caches only; there is no local creation-intent setting. Exactly one candidate is required. Full owner/private metadata, one member/admin, no bots/discussion/TTL, a versioned description pointer and a pinned original account/channel-bound identity record are cross-checked. Legacy marker/title channels upgrade from remote evidence only. Recognizable damaged channels and multiple candidates stop setup and Vault preflight. The remote title, description and pinned warning identify app management and explain edit/delete consequences. The UI retains automatic setup without channel-choice controls.

Deterministic coverage includes local-cache independence, remote candidate conflicts/order, explicit remote text fixtures, wrong account/channel/version, malformed/overflowing pointers, bounds and ownership/privacy flags. No live Telegram channels or sessions were touched. Required next qualification: protected separate-device login with an empty/stale local database; legacy upgrade; copied/edited/unpinned/deleted identity messages; public/member/admin/TTL changes; concurrent first creation; interrupted create/send/pin/about writes; delayed remote visibility. Telegram lacks atomic account/application uniqueness in createChannel, so exactly one physical channel under simultaneous first creation is not guaranteed. All remote traces erased and owner-forged markers cannot be distinguished without independent evidence. Do not claim cryptographic app provenance or distributed exactly-once creation.

All source quality gates passed with 328 tests, and the unsigned release bundle was rebuilt. Isolated native Chinese 900×600, Japanese dark 900×600 and English display-fitted setup previews confirmed readable remote-verification guidance and edit/delete warnings with no channel-choice controls. Conflict/error branches are localized but their live-server triggering was not exercised. Remote title/description/pinned-message changes have not been executed against a real account; the protected qualification cases above remain outstanding.

## Upload completion visibility

Files previously discovered manifests solely through Telegram search, while Raw Files read message history. A search snapshot without the newly published manifest could report no authenticated files until a later scan. An older asynchronous GUI scan could also replace freshly inserted upload receipts. Manifest discovery now supplements indexed results with exact-caption matches from the latest 512 history messages, deduplicates by message ID, and retains the existing 1,000-candidate cap and full cryptographic validation. Ordinary part lookups are unchanged. Upload completion cancels/increments the older scan generation, deduplicates receipts and validates login/account/channel/lock scope before displaying them.

Regression tests cover an empty/stale search index supplemented by recent history, duplicate IDs and bounded newest-first results, plus old-scan invalidation and account/channel/locked completion guards in the GPUI harness. No persisted formats or dependencies change. Live search visibility timing was not reproduced with credentials; protected follow-up is to upload a file and a mixed-size batch, immediately switch Files/Raw Files and refresh, then repeat on another logged-in device. Recent supplementation is bounded to 512 messages and is not a full history scan. All required source gates passed (330 workspace tests); the unsigned release app was rebuilt and packaged.

## Upload startup responsiveness

The remote-identity follow-up placed a full channel scan before queue publication and repeated it for every batch member. Upload now shows a preparation notice immediately, publishes queue rows and cancellation ownership before validation, performs complete discovery once per batch with at most four concurrent candidate inspections, and still freshly checks target identity for each file. SQLite bindings no longer gate upload authorization. Shared verification failures and cancellation produce visible per-file terminal outcomes, and account switching is blocked during pending submission/preflight. [ADR 0016](adr/0016-bounded-upload-preflight.md) records the uniqueness observation window.

Regression tests inspect queue/cancellation state inside an injected verification boundary, assert one batch validation and failure/cancellation propagation, and render the GPUI waiting notice before runtime rows exist. Live timing against a real account remains unmeasured: qualify first/repeated single-file and 128-file batches on a protected account with many owned channels and injected latency, and check queue visibility, cancellation, target metadata changes and RPC counts. No fixed startup-time claim is made. All source quality gates passed (332 tests) and the release bundle was rebuilt. Actual isolated Chinese, English and Japanese dark 900×600 upload-preflight previews confirmed the preparation notice and Waiting empty state before runtime rows exist. The GPUI test also covers duplicate submission prevention and account-switch blocking while preflight is pending.

## Verification record

Passed: the [development quality checks](DEVELOPMENT.md#quality-gates), including formatting, locked workspace/all-target check, strict Clippy, all workspace tests, warning-denied rustdoc, cargo-deny and diff review. Static GUI message IDs and all documentation links/anchors also resolve. The suite contains 335 tests: Core 25, Crypto 36, GUI 59, i18n 21, Index 12, Runtime 70, Storage 28, Telegram 30 and Transfer 54. Actual GPUI wheel-event isolation at both boundaries, collapsed-batch search/filtering, batch stop/failure policy and whole-batch history eviction, external deletion/recreation/size checks, mixed-source inventory pagination, account isolation, one-time legacy attribution/no-rebinding, private-channel identity, cancellation, modal Escape priority and About parity extend existing transition, migration, compatibility-vector and recovery coverage. No required source gate was skipped; the remaining live-system, security and product-review limitations are explicit below.

Actual isolated windows reviewed: English Transfers with 200 channels, authenticated Files, upload, returning-account and fresh login; Chinese 900×600 unlock layout, localized wrapping, Tab cycling within the dialog, Escape/cancel dismissal, Return submission/empty-password feedback and the compact raw file table; Japanese dark About with version/full changelog, Files/Raw switching, 5,000-row raw scrolling and direct return to Transfers. The sidebar refresh action retained the current Transfers view. Visual defects found during review were corrected, including table-header/input contrast and synthetic preview persistence banners. Preview runtime constructors are disabled and fixtures are synthetic; no real sessions, credentials, private files or remote writes were used.

Additional usability review covered Chinese 900×600 collapsed/expanded navigation, the 200-channel contextual browser, batch expansion/member inspection, missing/changed file labels, a real native six-file picker using only generated temporary text fixtures, file removal, fixed upload actions, advanced-body scrolling and Escape. English display-fitted wide Raw Files and Transfers showed responsive columns and richer batch rows. Raw/transfer inspector wheels were exercised through long content while the underlying lists remained stationary; choosing another raw file returned its details to the top and showed the correct availability/actions. Japanese dark About showed both release records and scrolled through their final paragraphs; its page-height bug was corrected so the disk status bar stays visible. The unsigned app bundle, Info.plist and standard/Retina icon generation were assembled and checked.

The pagination/source-record follow-up reproduced the exact GPUI entity-update panic with isolated unconfigured runtime owners and verified deferred loading plus cancellation, route, channel and account changes. Storage tests cover same message IDs in different accounts, Unicode channel titles, pagination look-ahead and ambiguous multi-object sources; runtime tests verify that caching adds catalog provenance without creating download tasks. Chinese 900×600 and Japanese dark 900×600 source details were checked, including independent scrolling and complete ID labels; the optimized release binary also passed the English display-fitted Library/details review. No live Telegram call was needed for these regressions.

The full locale/size/backing-scale, VoiceOver and real-account matrices remain release qualification. The reviewed layouts and keyboard actions do not imply those broader checks are complete.

Known toolchain debt: upstream `block 0.1.6` emits a future-Rust incompatibility warning; current workspace Clippy is warning-clean. Passing cargo-deny reflects reviewed policy/explicit exceptions, not a blanket claim that every dependency has no maintenance risk.

## Documentation consolidation

Contributor rules are centralized in the English [AGENTS.md](../AGENTS.md), with on-demand technical references instead of compulsory reading and change-scoped local validation instead of unconditional full reruns. `UI_GUIDELINES.md` and `I18N.md` were removed; reusable rules moved to AGENTS and the compact presentation/localization reference to Architecture. The ten remaining top-level documents describe architecture, current status, development commands, threat model and technical formats/engines. Duplicate process/provenance instructions were removed from Development/Security. Crypto/Manifest bytes, license files and CI gates are unchanged. Historical ADR bodies are retained, with explicit supersession links for replaced policy.

This revision follows OpenAI's [GPT-6 Astra prompting guidance](https://developers.openai.com/api/docs/guides/latest-model#prompting-best-practices) on auditing conflicting instructions, autonomous follow-through and proportionate testing, and its [AGENTS.md guidance](https://learn.chatgpt.com/docs/agent-configuration/agents-md) on concise repository rules (consulted 2026-09-08). Earlier consolidation removed `DIAGNOSTICS.md`, `SECURITY_AUDIT_PACKAGE.md` and `UI_REVIEW.md`.

The new acceptance policy requires visible feedback for every long operation and background state transition, plus graphical live/history diagnostics in upload/download inspectors. This is a documentation change: the existing v0.4.3 phase displays and native Live/Replay remain the implementation baseline. A complete cross-feature event timeline and graphical upload/download history are not yet established. Next: inventory each operation's emitted transitions, route gaps through Runtime snapshots/events, add inspector charts/timelines from real telemetry with visible truncation, and verify slow/blocked/error/cancellation cases and bounded UI cost. Local validation covers documentation links/anchors, stale-reference search and whitespace/diff review; the updated Crypto crate documentation is also built with warnings denied. Rust tests are not rerun for this documentation-only change.

[ADR 0017](adr/0017-versioned-automatic-migrations.md) replaces stage-based provisional-format rules and mandatory third-party-review gates with product acceptance and versioned automatic migration. No format bytes, executable behavior or app version changes in this revision; the migration capabilities in the table above remain the implementation baseline.

Git checkpoints preserve the complete source before the rewrite (`checkpoint/pre-gpui-kit-redesign-20260907`), before document removal (`checkpoint/pre-doc-consolidation-20260907`) and after consolidation (`checkpoint/post-doc-consolidation-20260907`). [Recovery commands](DEVELOPMENT.md#documentation-recovery-checkpoints) support inspecting/restoring documents without guessing prior versions. Source rollback never downgrades user data automatically.

## Next actions and release gates

1. Run protected real-account creation/discovery/rename/privacy-loss, switch-account, upload/manifest scan/download and legacy recovery tests; include timeout, cancellation, reconnect and crash boundaries.
2. Stream larger encrypted parts, persist Vault checkpoints/controls, hydrate all prior AEAD identities, reconcile orphan ciphertext and test rename/database-finalization crash windows.
3. Connect the generic Index coverage coordinator and qualify Channel push/difference behavior with a protected real account; benchmark Library and virtualized views from 1,000 to 3,000,000 records before claiming scale.
4. Add an owned physical media-DC connection pool, then measure throughput and server-limit behavior on real networks; synthetic controller tests are not bandwidth or ban-rate claims.
5. Complete minimum/default/display-fitted-large, three-locale, light/dark, backing-scale, native fullscreen, keyboard/VoiceOver, reduced-motion and native-speaker review. Qualify active-window/sleep/logout behavior without relying on synthetic secrets.
6. Review OS credentials, local permissions/SQLite backups, NSLocale discovery, packaging/signing/notarization and deployment/CPU support.
7. Extend versioned vectors, recorded fuzz campaigns and integrated recovery/crash evidence under [validation and migration safety](SECURITY.md#validation-and-migration-safety).
8. Extend the [automatic migration contract](adr/0017-versioned-automatic-migrations.md) beyond the connected SQLite startup path: qualify cross-format conversion, external backups, encrypted data/key preservation and protected interruption/low-space scenarios.

Capability rows describe connected behavior. Tests against fakes never stand in for real Telegram or successful recovery on a user's data.

## v0.4.1 Library action follow-up

Library rows now expose Open for imported local originals and Download for uniquely identified native Telegram records. Details provide Download with localized account/source/managed-file guidance alongside existing local Open/Reveal. A retained single preparation task prevents repeated clicks, rechecks source ownership after destination allocation and opens Transfers only after successful enqueue. Failure remains visible in details. Downloads use the catalog record’s identity independently of the currently browsed channel; no storage or crypto encoding changes were needed. Regression coverage includes source identity, signed-out/different accounts, missing message IDs and encrypted/package records.

Passed all development quality gates: formatting, locked all-target workspace check, strict Clippy, 309 workspace tests, explicit Core/i18n tests, warning-denied rustdoc, cargo-deny and complete diff review. The debug application and isolated native bundle build successfully. Chinese 900×600 Library rows and file details were visually inspected, including accessible Download labels and row-to-details navigation; preview intentionally disables transfer execution. The full locale/size/accessibility matrix and real-account download qualification remain pending as listed above. No live Telegram credentials or user documents were used.

## Library category tabs

Before v0.4.2, Library reused Transfers' segmented TabBar for all nine existing categories, including its sliding selection indicator and single-row horizontal overflow scrolling. Category IDs, translations and refresh behavior are unchanged; collection previews leave all category tabs unselected. Removed the wrapping button row and its obsolete helpers.

Passed formatting, locked workspace/all-target check, strict Clippy, all 309 workspace tests, explicit Core/i18n tests, warning-denied rustdoc, cargo-deny and complete diff review. The isolated English 900×600 Library preview builds and starts, but CUA cannot resolve the unbundled debug application, so this change has no completed visual/interaction review. Next: inspect the isolated bundled Library preview in all three locales at minimum width, exercise horizontal scrolling and category selection, and compare light/dark presentation with Transfers.

## Library download after external deletion

Reproduced a completed managed download followed by external deletion and a fresh Library-style enqueue: filesystem-only destination selection reused the old path, violating the native history destination uniqueness constraint and returning Persistence. Managed allocation now also queries the indexed native-history destination column, including terminal/other-account/legacy rows, and selects a numbered free candidate. Old history and existing files remain intact; no schema, persisted codec, localization or account-authorization changes are required. The shared allocator also serves explicit native re-downloads. Selection remains separate from insertion; concurrent collisions continue to fail safely.

The new deterministic fake-backend regression failed before the fix and now restores the exact bytes under a fresh task/path while retaining history. Additional coverage checks reservation lookup in queued/completed states, storage-error propagation and the 10,000-candidate bound. Passed all development quality gates: formatting, locked all-target workspace check, strict Clippy, 311 workspace tests, explicit Core/i18n tests, warning-denied rustdoc, cargo-deny and complete diff review. No real sessions or user documents were used. Real-account UI qualification remains pending: download from All Files, delete only the resulting local fixture, return to All Files and download again using a rebuilt application.

## v0.4.2 Library and Transfers

Library defaults to Local files and separates the current account’s indexed Telegram catalog into Remote files. The type menu filters either source independently. Runtime pages retained native/Vault completed outputs and imported originals, checks current filesystem metadata, excludes missing/inaccessible/non-file paths and cooperatively skips empty pages. Typed ephemeral cursors bind the account and filters; navigation/filter changes cancel local scans and stale account/generation results cannot replace the active view. Local-copy identities do not masquerade as LogicalFile IDs, and repeated paths are deduplicated. Counts explicitly describe visible rows. This is a view of retained inventory/imports, not an arbitrary disk scan; deleted history and unregistered legacy outputs cannot be rediscovered here.

At this checkpoint, transfer rows, batch headers and inspector member rows shared Library’s 42-point height; the batch presentation update below supersedes the header height. Two-line summaries preserve source, file names and completion counts; batch time and full members remain in details. The primary Transfers icon pairs upload/download arrows. The native re-download path fix described above is included.

Passed the development quality gates, including strict Clippy, 317 tests across the workspace (full workspace suite plus the final 51-test GUI suite), explicit Core/i18n tests, warning-denied rustdoc, cargo-deny and complete diff review. Actual GPUI tests verify source-tab clicks, retained type filtering and rendered file/batch row heights. Runtime regressions cover deleted/changed/imported files, account isolation, native/Vault identity separation, pagination over 128 deleted outputs, cancellation, cursor binding, and the failed-before/fixed-after download path collision. GUI tests also check local-copy deduplication and native bundle/package version parity.

Isolated native previews reviewed: Chinese 900×600 local/remote tabs, type-menu filtering, compact transfer rows and batch expansion with missing/changed states; Japanese dark 900×600 with collapsed/expanded navigation and both pages; English display-fitted 1360×760 launch with both source tabs, compact Transfers and the 42-point batch-member inspector. Local Open and remote Download accessibility labels are distinct. The sidebar label is now simply Library. No real sessions or user documents were opened. Full keyboard/VoiceOver/backing-scale qualification and protected real-account transfer validation remain the release gates listed above.

Version 0.4.2 is synchronized across workspace packages, both lockfiles, localized About/CHANGELOG and macOS bundle metadata. Debug and optimized Release builds pass, and the unsigned release bundle is generated at dist/TeleArk.app (ignored build output). Existing database schema, crypto/manifest formats and old tags remain unchanged.

## Local-first Channel synchronization

[ADR 0018](adr/0018-local-first-channel-sync.md) replaces selection-triggered Channel network scans with one retained account synchronization owner. Selection changes only the local presentation. Cached rows render locally and stay available during new/edit/delete differences, initial catch-up, gap repair and explicit older-history reads. All network/SQL work runs behind frontend-neutral owners; account/chat/generation guards reject stale callbacks. Runtime publishes event state, fixed timestamps and structured failure; the global status and independent bounded timeline stay available across navigation. Repeated selections do not request remote history or restart verification; actual scrolling or the earlier-history action requests one generation-scoped page as specified in ADR 0033.

Schema 11 adds atomic PTS/history/repair state and deletion tombstones. Long-offline `TooLong` recovery backfills intervening new messages to the prior local boundary and separately rechecks known IDs; both cursors survive restart. Old local outputs, wrapped keys and existing index cursors are preserved. Startup now creates a visible responsive window before opening/migrating local state; transaction protection, conversion and verification phases precede work. SQLite 0–10 automatically reaches read/write 11, and future schemas remain intact with guidance. No dependencies, licenses, app release version or encrypted codec bytes change.

Protected real-account qualification remains outstanding: push delivery, reconnect, silent subscription expiry, same-second edits, deletes, server `TooLong`, account switching and actual FloodWait. The passive Channel path also compares dialog metadata every 15 minutes and after unknown-source hints; private/common-chat updates and generic range coverage remain separate work. No live latency/throughput claim is made. Native CUA preview capture became unavailable with repeated timeouts after the initial Chinese 900×600 inspection; this follow-up does not claim a completed final native three-locale/light-dark visual matrix.

Validation for this follow-up: full locked workspace/all-target check, strict Clippy, 353 workspace tests (GUI 64, Runtime 77, Storage 31, Telegram 33, with the other crate suites), explicit Core/i18n tests, warning-denied rustdoc, cargo-deny, formatting and diff checks pass. The debug executable was rebuilt. All 535 static GUI message references and 93 local documentation links/anchors resolve. GPUI exercises local-only reads, stale callbacks, three-locale 900×600 controls, Japanese dark mode, independent timeline scrolling, startup failure controls and the first-frame-to-background-to-app handoff. These tests use isolated databases, synthetic rows, fake remotes and controlled scheduling; they are not live-account or final native screenshot evidence.

## Resizable Channel list

The Kit divider between the channel list and file workspace now supports live
horizontal dragging. Its completed width is saved through the background
preferences owner and restored across navigation and restart. Window resizing
and primary-navigation changes reapply that preference with bounds of 180–720
logical pixels and at least 500 pixels reserved for content at supported window
sizes; temporary clamping does not overwrite the preference. Interrupted drags
are discarded on navigation or window deactivation. The list includes a
localized resize hint and save/completion/failure feedback with retry; saving
and failures remain visible in the global status bar after navigation.

The additive version-1 preference defaults to 208 for existing databases;
invalid reads report a persistence error and invalid writes preserve prior
settings. Regression tests cover the actual divider drag/release, navigation
during a drag, window shrink/restore, failed saving and retry, legacy defaults,
save/reopen and invalid settings. GPUI bounds checks cover long synthetic titles
and primary actions at 900×600 in all three locales, both themes and both
primary-navigation widths. Native preview capture was attempted but CUA timed
out; native screenshot and assistive-technology qualification are not claimed.

Validation: all full source quality gates pass, including 358 workspace tests
(GUI 68, Runtime 78), strict Clippy, explicit Core/i18n checks, warning-denied
rustdoc and cargo-deny. The debug executable was rebuilt. Direct GUI message
references resolve and the three locale catalogs are synchronized.


## Event-driven updates and shared private-channel projection

[ADR 0019](adr/0019-event-driven-channel-projections.md) supersedes the earlier fixed GUI/scheduler polling and private-reader exclusions. The runtime parks until an event or actual deadline; contiguous pushes commit without a follow-up RPC, duplicate pushes do nothing, and unchanged final differences do not write SQLite. A coherent local baseline followed by per-channel deltas avoids unrelated-channel reloads and reuses unchanged formatted rows. The visible/private channel retains Telegram-required subscription renewal; passive metadata reconciliation and real retry/FloodWait timers remain.

Private Raw Files and managed Files now share message discovery and update coverage. Bounded one-time manifest discovery is followed by local-candidate derivation; unchanged manifests reuse authenticated metadata, while explicit same-second edits and gap revalidation invalidate it. Private edit/delete pushes publish pending-review feedback even during a blocked read, and schema 12 preserves confirmed observations/unread counts across restart. Acknowledgement cannot hide a newer event. Cached observation scope does not replace fresh remote identity/uniqueness checks or authorize writes. Manual refresh requests shared difference recovery. Legacy Saved Messages remains a separate explicit recovery path.

The 28-pixel status bar spans the whole window and opens details without navigation. Locked private warnings, message synchronization and manifest queue/read/transport/authentication/completion are visible through one event feed. Timelines/queues/caches are bounded and expose retention loss; durations update only during active work. Initial manifest discovery remains bounded to 1,000 candidates plus recent/new updates, with no full-history or unconditional live-delivery claim. Very old server gaps can omit previously unindexed old files; known IDs are verified and the durable gap warning preserves that uncertainty. Exhaustive exact-ID range reconstruction remains future work.

Validation: all full source quality gates pass, including 380 workspace tests (GUI 70, Runtime 91), strict Clippy, explicit Core/i18n checks, warning-denied rustdoc and cargo-deny. The debug executable was rebuilt. All 1,092 message IDs are synchronized across the three catalogs; 546 direct GUI message references and 98 local documentation links/anchors resolve. GPUI checks cover the global footer and locked warning on seven pages at 900×600 in all three locales and both themes. Isolated native previews additionally verified Chinese/light and Japanese/dark layouts, status persistence on Settings and Account, opening details without navigation and independent timeline scrolling. Native assistive-technology qualification and credentialed Telegram cross-device delivery remain untested. No release, commit or tag was made; earlier totals above describe their historical changes.

## Performance audit follow-up

The [performance audit](PERFORMANCE.md) records isolated reproductions and per-fix source checkpoint tags. The first fix replaces native part-event history reconstruction/shifting with a bounded ring and incremental counters, avoids unused hot-path snapshot copies, and exposes replay truncation. Total completed parts no longer fall with replay eviction. Full source gates pass with 381 workspace tests; one separately run manual algorithm benchmark compares the old and new path without a timing-based assertion. The second fix replaces transfer polling and full telemetry copies with event subscriptions and reusable immutable views, moves bounded queue refill into the retained runtime worker, and removes runtime reads from list/detail/status rendering. Validation includes 385 workspace tests after the additional GUI regression (Runtime 95, GUI 71), two separately run manual benchmarks, and full source gates with a targeted GUI follow-up. The third fix defers transfer and batch-member formatting to visible list rows, caches lightweight identities and preserves search/selection semantics. The latest GUI suite passes 73 tests, bringing the cumulative workspace tests to 387; three manual benchmarks remain separately runnable. The fourth fix defers channel sidebar menu/title/listener allocation until visible rendering, with stale-identity rejection and an actual click/scroll regression. GUI tests now pass 74 cases (388 cumulative workspace tests); four manual benchmarks are separate. The fifth fix reuses the displayed channel file projection for batch counters and select-all, avoiding repeated classification and time-boundary drift. GUI tests pass 75 cases (389 cumulative workspace tests). The sixth fix removes unused recursive disk scans, suppresses unchanged probe notifications, indexes bounded local-output candidates, backs off hidden idle probes and releases app owners before timer waits. A temporary-catalog/controlled-clock test covers idle notifications and closing a window. GUI tests pass 79 cases (393 cumulative workspace tests), with five separate manual benchmarks. These are intermediate checkpoint counts; the final audit result follows below.

Performance audit checkpoint 07 replaces the serialized Telegram command loop with bounded independent read/transfer/control lanes and session barriers (ADR 0021). Controlled scheduling tests cover blocked work, queue limits, cancellation, preserved creation guards and stale results. Live-network timing is outside this isolated audit; subsequent storage/checkpoint findings follow below.

Performance checkpoint 08 removes per-operation account lookup RPCs from runtime routing. Identity is scoped to successful authentication and cleared at session barriers; remote private-storage validation remains unchanged. Deterministic coverage includes all auth result classes, mismatched accounts and replacement identity.

Performance checkpoint 09 uses packed download bits and constant-time completion counters, preserving the exact version-1 resume codec. A fixed compatibility fixture and out-of-order/duplicate/tail regressions pass. Cumulative normal test count is 398, with six manual benchmarks excluded from normal runs.

Performance checkpoint 10 adds schema-13 indexes matching Library timestamp fallback and an actual keyset range seek. Cursor semantics remain version 1. Migration coverage includes all prior schemas, disk-full/DDL rollback and preserved wrapped keys/settings; query-plan and complete-pagination fixtures verify search behavior. Subsequent checkpoints cover exact counting and other background I/O paths.

Performance audit step 11 (2026-09-08): unfiltered Library search retains exact live totals while allowing SQLite's bare COUNT optimization. Filtered/FTS semantics are unchanged. Storage's 38 normal tests and strict package Clippy passed; counting-only timings and scope are in `docs/PERFORMANCE.md`.

Performance audit step 12 (2026-09-08): periodic native-download checkpoints no longer wait for SQLite on Telegram's async reactor. Bounded submission, acknowledged lifecycle writes and account/attempt/state guards cover saturated storage and stale samples. Full workspace gates pass (403 normal tests, eight manual benchmarks ignored); see `docs/PERFORMANCE.md`.

Performance audit step 13 (2026-09-08): session logging uses one bounded background file writer with explicit diagnostic gaps and localized cumulative counts. Disk failure no longer fails native transfers. ADR 0022 supersedes synchronous log admission/writes; contributor rules cover the recurring performance failure patterns. Full workspace gates pass (407 normal tests, eight manual benchmarks ignored). Diagnostic tail delivery at abrupt exit and broader log-file disk retention remain limited as documented.

Performance audit step 14 (2026-09-08): Storage/Telegram facade destruction and native-transfer shutdown no longer synchronously wait for queue capacity, SQLite or a running worker. Owned work retires cooperatively; guarded progress submission and existing part maps preserve recovery behavior. Controlled saturation/blocked-owner and restart tests pass. Full workspace gates pass (409 normal tests, eight manual benchmarks ignored).

Performance audit step 15 (2026-09-08): managed-directory preparation/path observations run on existing background callers, leaving the shared SQL actor available; Telegram destination metadata is asynchronous. Blocked preparation, settings preservation, collision/reservation and existing import tests pass. Full workspace gates pass (411 normal tests, eight manual benchmarks ignored).

Performance audit step 16 (2026-09-08): native admission and replay have session-wide budgets; older recoverable tasks and their batch members are no longer hidden by startup's history limit. In-memory terminal eviction preserves database/files and supports account-validated cold redownload. Lifecycle/replay/history omissions are localized and disclosed. ADR 0023 defines the normal 10,000-record admission limit and automatic preservation of older oversized recoverable backlogs. Full workspace gates pass (417 normal tests, eight manual benchmarks ignored).


Performance audit close (2026-09-08): 17 reproduced findings are fixed with individual local source checkpoint tags. The final change releases global scheduling locks before retirement persistence/cleanup while retaining per-task ownership and deferring an already-owned retry to its worker. Blocked-owner and stale/retry ordering tests pass. Final full workspace gates pass: 419 normal tests, ten manual probes excluded from normal runs, formatting, all-target check/strict Clippy, Core/i18n, rustdoc and dependency policy. Closing controller-copy and temporary-disk probes did not justify another algorithm rewrite. See `docs/PERFORMANCE.md` for measured scopes, recovery boundaries and remaining unmeasured workloads; this is not universal or live-network qualification.

## Catalog server failure isolation (2026-09-08)

A real read-only login incident reproduced successful authentication and independent reads alongside RPC 500 `RPC_CALL_FAIL` from `messages.getDialogs`, including outside the runtime dispatcher. The adapter now preserves a separate transient `Server` category. Catalog loading has independent activity, a globally reachable scrollable timeline, localized three-language errors, cancellation, a 20-second execution deadline and three bounded attempts with 2/4-second waits. Startup storage discovery waits for a successful catalog; failures never publish a partial roster, clear local data, create a duplicate private channel or replace the global login status. Navigation does not restart a failed/cancelled automatic cycle. Existing download retries and ambiguous upload outcomes keep their previous handling; durable codecs and dependency versions are unchanged.

The final read-only runtime probe still observed `Server` for the catalog, with successful authorized connection before it and successful independent read afterwards. Therefore remote enumeration and private storage availability remain blocked by this live Telegram response; this change does not claim that the server fault itself has recovered. Previously loaded in-memory sources and local library data are retained; cross-launch roster reconstruction has not been added. See [incident 18](PERFORMANCE.md#18-catalog-rpc-failure-incorrectly-invalidated-the-whole-workspace) for sanitized evidence and deterministic coverage.

Validation for this follow-up: all eight full source gates passed on the final source (424 regular workspace tests, 10 intentionally ignored manual performance probes). Chinese and English light plus Japanese dark 900×600 native isolated previews were inspected; the actual shell button opened the independent inspector, English cancel changed both action buttons to Retry, and the authorized Chinese account page had no catalog-derived global error. The optimized unsigned `dist/TeleArk.app` was rebuilt and packaged; its executable hash matches `target/release/teleark`. This does not constitute a complete accessibility/platform or protected real-account workflow matrix.

### Optional custom Telegram API configuration

Settings keeps custom API inputs collapsed until explicitly enabled, with a localized explanation before saving. Existing saved pairs retain their opt-in state. Disabling removes the pair through the background persistence owner and restores bundled credentials; failures remain visible and leave the controls enabled for retry. Builds without bundled credentials report the missing configuration. No persistent codec changes are required: the existing saved pair records the committed opt-in, and an unsaved editor choice is transient.

## Local packaging identity

Local release builds use `scripts/build-local.sh` to load and validate `.env.local`; the public `.env.example` is a template only. MTProto connections report TeleArk’s workspace version instead of the transport library version. The Telegram application name still belongs to the selected API ID; saved personal credentials override the embedded pair.

Encrypted transfer failure inspectors use transfer-specific reasons instead of native-download destination errors. Failed upload preflight retains its last phase, shown in the inspector; generic permission failures do not assert an OS or location without evidence.

## Storage workspace and repair defaults (2026-09-09)

[ADR 0027](adr/0027-storage-workspace-and-repair-defaults.md) moves healthy idle channel details into a header-triggered dialog and gives the file list the remaining workspace height. Active/failed work and repair confirmation remain visible. Repair now requires notification muting and archive placement, including retries after identity restoration; completion waits for readback of both settings. The separate archive action is removed, with synchronized English, Chinese and Japanese explanations. Schemas and encrypted formats are unchanged. Synthetic layout and adapter orchestration tests cover this contract; live Telegram mutations are not exercised by these tests.

## File drop and unrestricted upload selection (2026-09-09)

The upload composer accepts native external file drops, adds to the draft with canonical deduplication, and explicitly states that folders are not supported yet. File picker and drop preparation share background validation. Invalid/cancelled selections preserve the draft, concurrent selection work is rejected, and account generations reject stale callbacks. Dropping files never starts an upload without confirmation.

[ADR 0028](adr/0028-upload-selection-without-count-cap.md) removes the 128-file user limit at selection and Runtime admission. The worker schedules internal 128-file windows so its 256-member history cannot silently drop later uploads. Whole-selection counters, phase duration, last activity, cancellation and bounded timeline remain visible across navigation; history retention is disclosed. Recent successful manifest receipts stay bounded and the authenticated catalog refreshes when receipts are truncated. The composer virtualizes selected rows and caches total size instead of scanning the draft every frame. Data codecs, schemas and dependencies are unchanged.

## Distinct batch rows and auxiliary member window (2026-09-10)

Single-file upload/download rows use one line (42 points), including singleton runtime batches. Batch headers use 1.5 rows (63 points), retain ordinary collapsed surfaces, and show completed/total and failure counts. Expanded groups now form one continuous neutral container: a darker semibold title, lighter member surface, shared outline and left marker, indented child checkboxes/names, rounded ends and outside spacing separate each batch from other tasks. This supersedes the earlier separator-only presentation; members still have no individual boxes. Hover/selection preserve the title/member hierarchy. Group edges use adjacent visible batch identities without formatting or scanning additional rows, so filtering cannot connect unrelated tasks. The same surfaces and continuous marker connect the fixed title and scrolling member list in larger batches' auxiliary windows. Repeated clicks reuse the window; closing it preserves work. Account changes, parent-window close and parent release invalidate it; member rendering does not retain the parent owner. No background transfer or persistent schema behavior changes.

Deterministic GUI coverage checks 9/10-member routing, window reuse/close/reopen, account invalidation, projection updates, singleton upload/download presentation, contiguous 42/63-point content rows, visible group boundaries, painted hierarchy surfaces, checkbox indentation and selection/scroll behavior across three locales and both themes at 900×600/1360×760. It also verifies the auxiliary title/rail/member geometry and visible-only materialization for 10,000 members. GUI 110 and i18n 21 tests passed (3 existing manual benchmarks ignored). Native synthetic previews reviewed Chinese light adjacent upload/download groups and expand/collapse, Japanese dark compact groups, and the English 12-file auxiliary window with completed/active/waiting members. No live account transfers are used for these layout checks.


## Compact transfer rows and fixed list summaries (2026-09-13)

Transfer file rows use 24-point content height and batch headers use 34 points (about 1.4 file rows). Inline controls and progress labels fit within those bounds; Library file rows keep their existing height. Transfer, Library, managed-file summaries and raw-channel loading/retry/history bars use one shared 24-point fixed footer with no wrapping. Overflow can scroll horizontally without consuming list height. Footer actions fit inside the bar.

This supersedes the earlier 42/63-point transfer geometry and compact-window footer minimum heights. Visual previews and layout reviews now use English and light mode only, at 900×600 and actual native full-screen; existing automated dark-theme coverage may remain. No persistent formats or transfer execution behavior change.

Validation: the current checkout passes 130 GUI tests (3 existing manual probes ignored); the isolated commit snapshot passes 111 (3 ignored). Strict GUI Clippy and formatting pass. Deterministic coverage verifies fixed footer/list geometry, horizontal overflow and action containment at compact and larger sizes. Native synthetic English/light previews cover Transfer, Library, Storage and Channel history at 900×600 and actual full-screen. No live account or transfer data is used.


## App-wide list density (2026-09-13)

All data lists now share 24-point rows: Library, raw Channel files, managed Storage files, upload selections, Transfer members, primary/contextual/settings navigation, metadata rows and activity/history summaries. Batch headers remain 34 points. Primary/secondary text is 12/11 points, with shared 14-point icon, 18-point badge and 22-point action tokens. Standard row actions share the same ghost-button treatment. Long summaries retain their complete values in tooltips and detail views. List headings and fixed footers use the same row height. This supersedes the previous Library42/Channel30/Storage60/upload48 and history/menu-specific sizes. No transfer, persistence or network contract changes.

Visual reviews continue in English/light only at 900×600 and actual native full-screen. Each completed repository task is now committed with an annotated local Git tag; unrelated pending changes are preserved and pushes remain explicit.

Validation: all eight development gates passed (format, workspace check, strict Clippy, workspace tests: 555 passed/10 existing manual probes ignored, focused Core/i18n, warnings-denied docs and dependency policy). After final typography/metadata adjustments, the checkout GUI suite passes 134 tests (3 existing probes ignored), and the independently exported task-only commit passes 115 (3 ignored); both pass strict GUI Clippy. Four new native-layout regressions cover row continuity, text/action containment, menu geometry and metadata labels. Existing history/footer/upload/batch regressions also pass. English/light synthetic native reviews cover Library, raw Channel and managed Storage lists, Transfer groups, settings/navigation, upload selection, file metadata and sync/event history at 900×600 and actual native full-screen. The auxiliary batch window is reviewed at its smaller default size and actual full-screen. No live account or transfer data is used.


## Durable encrypted transfers — in progress (2026-09-13)

Schema 17 now provides account-scoped immutable task context, generation-fenced state transitions, durable pause/cancel intent and immutable part reservations/receipts. Tests cover real-file reopen, stale workers, pause/completion races, explicit retry versus blocked failures, future-context preservation, indexed bounded queue pages, legacy-history preservation and failed-upgrade rollback/restart. Version-1 context/part/receipt codecs preserve wrapped keys and immutable scope with frozen compatibility coverage. New DesktopVault uploads now preflight source digests, admit their recovery context and use durable reservations/verified receipts through the bounded parallel encryption pipeline. Stable Telegram publication IDs and exact-ciphertext reconciliation cover ambiguous part sends. Cold startup now recovers ledger intent before owners start; active-key unlock/history readiness dispatches queued uploads in bounded pages. Explicit resume reuses original context, validates source bytes/identity and preserves manifest identity. Schema 18 now adds immutable manifest commitments, exact encrypted envelopes, stable publication IDs and verified receipts in the actual upload path. Independent per-task upload controls are connected. New encrypted downloads also admit recovery contexts and persist fsynced verified extents, retaining failed partial output; the executor supports local reopen verification and already-published output validation. Explicit download resume and shared per-task controls are connected to the transfer list, retaining original manifest/task/destination identity. Schema 19 adds indexed locked download history, unavailable-context explanations and mixed queued dispatch after unlock. Crypto cancellation now reaches bounded frame checks, and restored history yields to live execution with fail-closed projection admission. Pending-batch admission, recovery-queue/retention qualification, source-independent upload finalization, full cancellation/finalization boundaries, explanatory UI and end-to-end qualification remain incomplete. The durable/mid-file/restart capability is not yet delivered and legacy upload history remains non-resumable.

- Durable-transfer UI work in progress: task inspectors now prioritize capability-aware retry/blocked/history guidance, and the storage introduction explains pause, cancellation and restart limits. The expanded guide retains a bounded, independently scrollable area. Pending batch admission, remaining recovery races and full end-to-end qualification still prevent declaring the durable-transfer objective complete.

- Upload resume now has a source-independent manifest-finalization path when a complete encrypted envelope was saved. It reuses the publication identity, verifies readback, persists inventory, respects durable stop intent and leaves part health unchecked rather than inventing a fresh verification. Missing envelopes still require the original source; full DesktopVault/Telegram restart acceptance remains pending.

- Fixed a reproducible native download pause/resume race: old paused-worker retirement no longer overwrites a resume awaiting persistence, and checkpoint acknowledgment always retries deduplicated scheduling. A gated regression verifies no early restart before acknowledgment and completion without an external wake.

- Native transfer workers now reject stale queued starts after pause/cancel and consume resume/retry controls atomically. Pause/resume retirement no longer replays already-persisted command state over newer controls; deterministic regressions protect newer stop/retry commands and verify that stale starts do not enter filesystem/transport work.

- Native cancellation cleanup now waits for the old backend owner to exit. A per-task barrier prevents reopen/delete races while accepting a queued immediate retry; cleanup acknowledgment precedes its durable queue write and admission. Gated success/failure tests verify exclusive cleanup, retained bytes on failure, and preserved Cancelled recovery state. Interrupted-cleanup restart handling and waiting-phase presentation remain under audit.

- Resumed uploads now register queued controls before source preflight and check stop signals around each bounded source read, independently of throttled progress updates. Preparation preserves newer Pause/Cancel projections. Fresh-upload pre-admission durability and the corresponding individual controls are still pending; batch stop-after-current behavior is unchanged.

- Encrypted-download restart verification now retains the published file handle, rejects symlink/non-regular final entries and checks the named file around reuse and completion. Unix regression tests reject identical-byte path replacement and preserve foreign output. Cross-platform identity parity and initial-open substitution remain under qualification.

- Schema 20 adds bounded metadata-only pending upload admission and atomic promotion into the executable ledger, with restart, conflict, stale/stopped owner and migration rollback coverage. Runtime batch/source-codec integration is still pending; this storage foundation alone does not make unstarted selections recoverable.

- Pending-upload codec v1 now binds native source identity, task/batch and vault/key scope without reading contents or allocating encryption identities. Pending control persistence fences old preflight results across pause/resume and retry; source-codec and reopen tests preserve changed/unsupported work. Actual batch/startup/control-owner wiring remains pending.

- Actual batch submission now saves every pending source before remote validation, with bounded groups and a visible committed-file counter. Queued upload preflight checks the saved generation/source and atomically promotes into the executable ledger; selection-wide failures settle durable pending entries. The 513-file selection regression verifies all windows at the validation boundary. Pending resume/control dispatch, history hydration and interrupted-prefix acceptance remain incomplete.

- Pending uploads now reach independent pause/cancel controls, explicit resume, account-scoped startup dispatch and locked history hydration. Promotion races hand control to the formal ledger; startup preserves concurrent pauses. History prioritizes queued work and counts the union of durable representations. Synthetic failure-path, control-handoff and 300-row history tests pass; successful remote restart composition remains pending.
- The native restart test’s Running/Queued failure was traced to its helper activating the account before asserting Queued. A gated backend reproduces that ordering deterministically. The corrected test checks inactive hydration first, then account activation, Running and completion without an extra scheduling call; broader shutdown/checkpoint ownership remains a separate audit.

- Pending uploads preserve the admitted display filename when resuming through a canonical source path, including a selected symbolic-link alias with a different basename. Full successful pending-upload restart composition remains under qualification.

- Successful synthetic DesktopVault composition now covers pending alias restart,
  upload/download mid-transport pause, production-size multi-part local reuse,
  network retry, terminal cancellation, source-free manifest completion and lost
  publication replies. The wire fake preserves real owners, crypto and SQLite;
  it does not qualify real MTProto. Resumed upload part geometry is refreshed to
  avoid false persistence failures, and authenticated download parts are fetched
  once instead of discovery plus a second download.

- Complete upload selections now commit atomically with one encoded context in
  memory at a time. Saved counts acknowledge commit, and cancellation/iterator
  failure rolls back all newly admitted rows. Independent-reader and unclean
  subprocess-exit tests prove that interruption does not expose a runnable prefix.
  This supersedes the earlier bounded-group admission description; full feature
  completion still requires the remaining ownership, cleanup and UI qualification.

- Schema 21 adds durable native cancellation cleanup obligations and retry intent.
  Atomic Storage operations fence old task writes and history deletion, preserve
  future codecs, and combine cleanup acknowledgment with queued retry. Reopen,
  transaction-failure and migration-conflict tests cover this contract. Runtime
  cancel/retry, startup cleanup dispatch and waiting/error UI remain unconnected;
  this storage layer alone does not resolve the native cleanup crash window.

- Native cancellation cleanup now uses the schema21 obligation through retained
  control/filesystem owners. Cancellation is saved before signalling; restart
  restores unfinished cleanup and durable retry intent before new downloads may
  start. Blocked cleanup does not block unrelated downloads, and frontend Drop
  does not join it. Runtime tests verify restart, no premature retry, preserved
  final files and shutdown; native list/batch/inspector presentation exposes
  waiting, removal, failure, elapsed time and supported actions. Filesystem
  substitution and cross-platform directory-sync qualification remain separate.

- 2026-09-14: Upload history hydration now reflects validated terminal ledger
  outcomes and materializes a bounded page of formal uploads missing compatibility
  summaries while locked. Completed progress is restored without inventing speed
  or duration; damaged contexts and unknown failure codes remain visible with
  recovery disabled. Native cleanup waiting/failure layouts were reviewed in
  English/light at 900×600 and actual macOS fullscreen, including the global
  cleanup link and independent inspector scrolling. Full durable-transfer
  delivery still requires the remaining account/filesystem/crash audits and
  final repository gates; these checks do not establish real MTProto behavior.
