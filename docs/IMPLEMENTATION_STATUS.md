# Implementation Status

Last updated: 2026-09-05

## Current milestone

TeleArk is a **persistent local-catalog alpha with API ID onboarding, real Telegram login, browsing,
native download, indexing, and a production-adapter encrypted-transfer
foundation**. In addition to local import/search/open/reveal, the desktop
atomically persists a personal Telegram API ID/API Hash pair or uses an
optional distributor-owned pair supplied at build time, performs QR or
code/2FA authorization, lists real
dialogs, immediately shows cached source rows and refreshes each selected
source through cancellable 200-message chunks in a virtualized table bounded
at 5,000 rows, filters by time and multiple file types, supports direct and
multi-selected downloads without a per-file save dialog through a retained
bounded worker, creates atomic per-channel selected-file batch downloads, and can
scan source pages into SQLite/FTS5. Batch tasks appear as one expandable
Transfers row, while their message sent time, MIME type, and full caption
survive restart. A configurable managed-files
root keeps `Downloads`, `Cache`, and privacy-bounded `Logs` together for direct
user management. Daily structured tracing covers runtime storage, Telegram,
and native-download operations, while transfer details expose safe performance
and failure diagnostics. Native download rows, checkpoints, timing, and
structured failures persist in SQLite; missing 1 MiB logical parts are fetched
concurrently through Telegram-compatible 512 KiB requests, written at their
offsets, and tracked by a versioned bitmap for restart continuation. Live part
events drive progress, current speed, ETA, pause/resume/cancel/retry, and an
adaptive per-file inflight target.
Desktop shutdown writes the latest active byte checkpoint without waiting on
an indefinitely stalled Telegram network read.

The native Telegram indexing entry path is real, but it is not yet the complete
CAS/range coordinator and does not include live-update ingestion. The runtime
now retains a bounded, serialized Vault owner that persists only wrapped key
metadata, holds the unwrapped Master Key outside the GUI, supports creation,
password and Recovery Bundle unlock, database-loss restore, password change,
recovery rotation, explicit lock, and automatic lock when its Settings section
is hidden. Self-contained recovery exports are exact-version secret bundles;
OS Credential is deliberately disabled and labeled as under development.

The desktop sidebar no longer exposes the unused Library, preview Collections,
or standalone Key Vault destinations: authorized Telegram channels appear
directly by source name, Saved Messages has separate raw Telegram-object and
authenticated TeleArk-file projections, and Key Vault is available only within
Settings. The raw inspector explains recognized versioned manifest/part objects
and their relationships. The managed projection asks the runtime to authenticate
remote manifests, restores original logical names and metadata, lists related
objects, and can reconstruct a selected file through verified decryption and
atomic non-overwriting publication. Upload selects a real local file and sends
encrypted, verified parts plus a final authenticated Manifest to Saved Messages.
The desktop Transfers route combines native downloads with real Vault upload and
download snapshots including direction, bytes, part progress, timing, rate,
destination, crypto suite, manifest state, structured failures, adaptive
controller telemetry, and permanent per-transfer session logs. Encrypted upload
uses a bounded reader/encryption-worker/encrypted-queue/uploader pipeline so CPU
encryption overlaps network transfer. The Transfers inspector exposes localized
Live and decision-by-decision Replay views.
The global sidebar reports live aggregate transfer rates, free destination
space, and bounded background-measured TeleArk disk use. The encrypted path is
an alpha: its snapshots are not durable and do not yet expose pause, cancel,
retry, priority, bandwidth policy, or restart resume.

## Implemented capability

| Area | Current evidence | Deliberate limit |
| --- | --- | --- |
| Core application API | Typed Library queries, imports, pages, statistics, repository port, structured errors; 25 Core tests total | Transfer/Index/Vault services are separate foundations, not one application command bus |
| Persistent Library | SQLite migrations v1-v8, strict tables, FTS5, Telegram remote identities/cursors, facets, settings, collections, index rows, encrypted transfer checkpoints, durable native-download history/progress/message metadata, atomic batch creation and credential settings, plus one strict singleton wrapped Vault-metadata row | Vault package/manifest tables, collection editor, and million-row benchmark remain; authenticated Telegram manifests are the current managed-file authority |
| Desktop runtime | Existing bounded storage/Telegram/native-download workers plus a retained bounded Vault owner; persisted password/recovery wraps; create/unlock/lock/password-change/recovery-rotation/database-loss restore; concurrent 1 MiB native parts with bitmap resume; bounded overlapping encryption/upload pipeline; adaptive telemetry and required session logs; real encrypted Saved Messages upload, authenticated manifest scan, and verified atomic restore download | Vault transfer snapshots/controls/checkpoints are memory-only; encrypted parts temporarily buffer at 60 MiB; the current grammers adapter does not expose physical media-DC lanes or multiple owned transfer connections; OS credential integration, orphan cleanup, full identity hydration, and credentialed system tests remain |
| GPUI desktop | Existing Telegram/channel/native-transfer UI plus direct named-channel navigation, separate raw and authenticated managed Saved Messages views, real encrypted upload/restore actions, controller Live/Replay with C/W/F/P/E/Qe, rates, BDP, buffers, part state, decisions and session-log path, a persisted Respect/Adaptive Override/Ignore soft-limit selector, and Key Vault lifecycle controls only inside Settings; OS Credential is visibly disabled | Physical DC/lane rows honestly remain unavailable through the current grammers abstraction; the hidden legacy Library route remains paged; Vault accessibility/pixel review and encrypted pause/cancel/retry controls remain |
| Localization | Complete synchronized Fluent catalogs for `en-US`, `zh-CN`, and `ja-JP`; live switching, persistent explicit override, System Default; 21 tests | Native-speaker, assistive-technology, and pixel-level locale review remain |
| Telegram adapter | `grammers` 0.10 connection, short-lived QR login with DC migration, code/2FA, dialogs, refetch by message identity, bounded cancellable cursor scans, upload/download, structured errors, concurrent 1 MiB logical downloads over 512 KiB requests, positional writes, strict `TARKDPM1` resume bitmap, part observer/control, atomic no-replace publication, and a versioned atomic `0600` session cache; 16 tests | Native download checks Telegram's byte length rather than a content hash; physical DC/lane identity is not exposed; ordinary tests use no live credentials and OS credential-store UX remains |
| Historical Index Engine | Desktop Telegram-to-SQLite bounded scan with durable cursor plus the separate CAS/range coordinator and its 12 deterministic tests | Full coordinator repository mapping, live updates, retry owner, pause/cancel UI, and range compaction remain |
| Transfer Engine | Bounded encrypted scheduler, reconfigurable concurrency envelope, adaptive C/W/F/P/E/Qe goodput controller with BDP/soft-limit/FloodWait decisions, bounded encryption pipeline, native positional files, BLAKE3, SQLite checkpoints, reconciliation and safe `.partial` finalization; 36 engine tests plus runtime integration | Current production adapters vary native-download P but truthfully clamp unavailable physical C/W/F and fixed upload E/Qe; encrypted engine still buffers one application part (temporarily capped at 60 MiB); physical media connection pool, true large-part streaming, and bandwidth control remain |
| Crypto/manifest candidate | Explicit bounded codecs, canonical checksummed Recovery Key text, self-contained versioned Recovery Bundle, authenticated remote Manifest publication/discovery, File Key recovery, and locator-bound download after fresh-SQLite recovery; two libFuzzer targets and daily fuzz workflow remain | Formats remain provisional: no independent review, long campaign evidence, full identity hydration/generation-conflict policy, or compatibility promise |
| Packaging | Local macOS development build and baseline release workflow | No signed/notarized installer or production platform-support claim |

## Architecture and data truth

- Core, Index, Transfer, Storage, Telegram, Crypto, Runtime, i18n, and GUI are
  separate workspace crates with one-way dependencies.
- GPUI types do not enter Core; the GUI does not execute SQL, call `grammers`,
  perform crypto, or own transfer checkpoints.
- `grammers` types remain private to the Telegram adapter and SQL remains in the
  Storage crate.
- The user-facing and persisted domain abstraction is `LogicalFile`; Telegram
  messages and multipart pieces are adapter/diagnostic details.
- The local database is an index/checkpoint/settings cache. Native desktop
  download task history and chunk progress survive restart; interrupted running
  tasks return to the queue and continue after Telegram reconnects. The fake-remote
  acceptance path now proves recovery from an authenticated remote Manifest;
  live Telegram recovery still needs credentialed system testing and desktop UX.
- The configurable managed-files root owns the user-visible `Downloads`,
  `Cache`, and `Logs` directories. SQLite and the Telegram session intentionally stay in
  the platform application-data location because live database/session
  relocation is not implemented.
- Crypto and manifest encodings are implemented recovery candidates but remain
  provisional until independent review, longer fuzz campaigns, and release
  compatibility fixtures are complete.

## UI and responsive verification

The desktop layout has compact, standard, and spacious policies with a hard
minimum of `900x600`. Deterministic layout tests cover breakpoint edges,
localized primary-control width budgets, dialog/inspector fit, progressive
column visibility, and usable route content budgets.

The last recorded process-launch smoke matrix covered:

```text
Library, Upload, Transfers, File Detail, Key Vault,
Channel Index Detail, Settings
x en-US, zh-CN, ja-JP
x 960x640, 1360x760, 1920x1080
= 63 launches
```

All 63 configurations started and remained alive for the bounded smoke period.
That evidence predates removal of the standalone Key Vault route. The revised
six-route matrix, with Key Vault exercised through Settings, has not yet been
recorded. These launches verify startup and gross layout-policy selection, not
actual pixels.
The v0.2 Settings changes were additionally launched at `960x640` in English,
`1360x760` in Simplified Chinese, and `1920x1080` in Japanese; all three
remained alive through the bounded check, while deterministic tests cover the
new localized toolbar budget and authorized nested-scroll policy.
Unlocked macOS captures additionally verify the dual-method Telegram login and
real Transfers routes at `900x600` in Simplified Chinese and a requested
`1680x960` Telegram route in Japanese. The latest compact capture includes the
native macOS titlebar, prominent global login action, aligned phone/QR panels,
both primary login buttons without clipping, and an English Settings view whose
localized API placeholders and wrapped credential notice remain inside the
card at `900x600`. Visual inspection also found and fixed compact navigation
scrollbar wrapping, transfer-summary overlap, and oversized-window placement.
Oversized requests use desktop-safe insets before centering. Native macOS
windowed/full-screen transitions were also exercised with the traffic-light
control; the app now publishes localized application, View, and Window menus
while retaining Escape and the visible in-app exit-full-screen control.
The batch-transfer/message-detail/sidebar-metrics build additionally remained
alive in bounded launches at `960x640` in Simplified Chinese, `1360x760` in
English, and `1920x1080` in Japanese. These launches verify startup and layout
selection only; authenticated batch content still needs capture-based review.

## Verification evidence

The applicable local gate for this milestone is:

```text
cargo fmt --all --check
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
cargo deny check
git diff --check
```

The workspace test suite contains 259 deterministic tests: Core 25, Crypto 36,
GUI 48, i18n 21, Index 12, Runtime 44, Storage 21, Telegram 16, and Transfer 36.
The current gate passes format, workspace check, strict Clippy, all workspace
tests, warning-denied documentation, dependency policy, and diff validation.

The published GPUI dependency graph currently reports future-incompatibility
warnings for transitive `block 0.1.6` and `proc-macro-error2 2.0.1`; the workspace
itself is warning-clean under Clippy. Existing documented RustSec and license
policy exceptions remain governed by `deny.toml` and third-party notices.
The new direct `base64 0.22.1` and `qrcode 0.14.1` dependencies both declare
`MIT OR Apache-2.0`; QR default image/SVG/PIC features were disabled, so the
locked graph gained only `qrcode` while reusing the existing `base64` package.
The direct `tracing-subscriber 0.3.23` and `tracing-appender 0.2.5`
dependencies are maintained by the Tokio tracing project, declare MIT, and are
used with their documented JSON and bounded non-blocking rolling-writer APIs.
They do not introduce the prohibited GPL `ztracing`/`zlog` dependency family.
The direct `sysinfo 0.31.4` dependency declares MIT, was already present in the
locked transitive graph, and is used only for destination-volume capacity; the
application-owned directory scan remains bounded and does not follow symlinks.
The runtime directly reuses the workspace's locked `blake3 1.8.7` package
(`CC0-1.0 OR Apache-2.0 OR Apache-2.0 WITH LLVM-exception`) so the upload Reader
can calculate the whole-file digest without a second disk pass; no new
transitive package was introduced.

## Known gaps and risks

- Desktop QR/code login, source selection, bounded document browsing/download,
  and bounded indexing are real. The
  richer generic Index coordinator is not yet the production desktop owner.
- Transfer now has concrete native-file, SQLite, crypto, and Telegram adapters,
  plus fake-remote crash/restart and database-loss integration. A retained
  desktop Vault owner now connects encrypted Saved Messages upload, scan, and
  restore, but its snapshots and controls are not durable and it still
  needs streaming beyond the temporary 60 MiB part cap, credentialed Telegram
  system tests, orphan reconciliation, and more finalization crash injection.
  Both paths permanently write safe controller session logs. The real
  native-download worker now has byte/part progress, out-of-order bitmap resume,
  adaptive P, pause/resume/cancel/retry, but still lacks automatic retry policy,
  physical multi-connection/DC-lane ownership, bandwidth control, and a content
  hash. The current upload transport is serialized, so its fixed E/Qe and
  connection bounds are reported honestly rather than presented as adaptive.
- Crypto/manifest/recovery-bundle candidate vectors are not a stable released
  format. Lifecycle and fake-remote recovery proofs are engineering evidence,
  not a released recovery guarantee or an independent security assessment.
- Scheduled fuzzing is continuous regression pressure, not proof of security.
  An independent reviewer must still sign the exact release candidate, and the
  format cannot be frozen until encrypted database-loss recovery is integrated.
- The hidden legacy Library route remains paged but not virtualized;
  million-record performance and UI memory behavior are unmeasured.
- Existing imported source files can move or disappear; open/reveal uses the
  retained original path and the runtime returns structured source errors on a
  later operation.
- Local catalog metadata includes filenames and absolute source paths. The
  per-user data directory is restricted on supported Unix systems, but device
  account security and backups still matter.
- Key Vault is now confined to Settings, while OS Credential remains disabled
  pending a reviewed platform adapter. Recovery rotation cannot revoke old
  exported disaster-recovery bundles. Personal API Hashes persist in unencrypted SQLite; distributor hashes are
  extractable from their build binary. Neither is an account session secret,
  but both require appropriate local/release handling. The obsolete sidebar
  Collections preview has been removed; collection persistence remains an
  internal Core/Storage capability without a current desktop entry point.
- Accessibility, focus trapping, reduced motion, screen-reader labels, and
  native-speaker wording still need dedicated product review.
- Actual screenshots now verify compact Chinese login/Transfers layouts, a
  display-fitted spacious Japanese login layout, and the local development
  binary's native full-screen transition. The full route/locale pixel matrix, a
  packaged-app native full-screen transition check, and authenticated
  QR/channel/file/download states still need capture.

## Next implementation sequence

1. Map the generic Index coordinator repository and retry/cancellation policy
   onto the working bounded Telegram-to-SQLite desktop scan path.
2. Add a project-owned physical media-DC connection pool above raw grammers
   MTProto calls so C/W/F and lane-scoped FloodWait can vary in production, then
   connect the existing adaptive controller and soft-limit modes to it.
3. Replace the current bounded per-part buffers with streaming native-file ↔
   crypto-frame ↔ Telegram pipes and move Vault snapshots onto durable
   checkpoints with pause/cancel/retry/reconciliation controls.
4. Hydrate all prior wrap/manifest/part encryption identities on restart and add
   cleanup/reconciliation for encrypted parts left before Manifest publication.
5. Virtualize Library/Transfer lists and benchmark 1,000 to 3,000,000 records.
6. Run unlocked reference screenshot comparison at the matrix sizes/locales,
   then complete keyboard and assistive-technology review.
7. Add the reviewed OS credential adapter and keep it disabled in Settings until
   its platform and recovery tests pass.
8. Run longer recorded fuzz campaigns, obtain independent crypto/security
   review, stabilize versioned fixtures, and validate packaging/signing.

## Handoff rule

Update this file whenever code changes a capability row. Rendered UI is never
proof of backend completion, and passing fake-port tests is never represented as
real Telegram, encryption, or recovery behavior.
