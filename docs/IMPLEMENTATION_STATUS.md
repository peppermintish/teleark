# Implementation Status

Last updated: 2026-08-31

## Current milestone

TeleArk is a **persistent local-catalog alpha with API ID onboarding, real Telegram login, browsing,
native download, indexing, and a production-adapter encrypted-transfer
foundation**. In addition to local import/search/open/reveal, the desktop
atomically persists a personal Telegram API ID/API Hash pair or uses an
optional distributor-owned pair supplied at build time, performs QR or
code/2FA authorization, lists real
dialogs, browses each selected source in virtualized pages of up to 5,000
messages, filters by time and multiple file types, supports direct and
multi-selected downloads without a per-file save dialog through a retained
bounded worker, creates atomic per-channel selected-file batch downloads, and can
scan source pages into SQLite/FTS5. Batch tasks appear as one expandable
Transfers row, while their message sent time, MIME type, and full caption
survive restart. A configurable managed-files
root keeps `Downloads`, `Cache`, and privacy-bounded `Logs` together for direct
user management. Daily structured tracing covers runtime storage, Telegram,
and native-download operations, while transfer details expose safe performance
and failure diagnostics. Native download rows, checkpoints, timing, and
structured failures persist in SQLite; live Telegram chunks drive progress,
current speed, ETA, pause/resume/cancel/retry, and restart continuation.
Desktop shutdown writes the latest active byte checkpoint without waiting on
an indefinitely stalled Telegram network read.

The native Telegram indexing entry path is real, but it is not yet the complete
CAS/range coordinator and does not include live-update ingestion. The runtime
now composes real native files, SQLite checkpoints, framed encryption, remote
object reconciliation, encrypted Manifest publication, and database-loss
recovery. The desktop Transfers route now renders real native-download tasks,
supports multi-selection/select-all, and opens or reveals completed destinations;
its global sidebar reports live aggregate transfer rates, free destination
space, and bounded background-measured TeleArk disk use;
Upload, encrypted transfer controls, Vault, and key-unlock UX remain previews.

## Implemented capability

| Area | Current evidence | Deliberate limit |
| --- | --- | --- |
| Core application API | Typed Library queries, imports, pages, statistics, repository port, structured errors; 25 Core tests total | Transfer/Index/Vault services are separate foundations, not one application command bus |
| Persistent Library | SQLite migrations v1-v7, strict tables, FTS5, Telegram remote identities/cursors, facets, settings, collections, index rows, encrypted transfer checkpoints, durable native-download history/progress/message metadata, atomic batch creation, and atomic credential-pair settings; 20 temporary-database tests | Vault package/manifest tables, collection editor, and million-row benchmark remain |
| Desktop runtime | Bounded storage/Telegram workers, validated personal/distributor credential resolution, persistent personal API ID/Hash pair, typed atomic `preferences.v1` settings, one configurable managed-files root with created `Downloads`/`Cache`/`Logs` children, bounded non-blocking daily JSON tracing, safe automatic non-overwriting download paths, QR/code/2FA login, dialog discovery, bounded 5,000-item channel batches, idempotent projection, restart index cursor, bounded background disk metrics, and a retained bounded native-download worker with atomic batch identity, chunk progress, current speed/ETA, pause/resume/cancel/retry, durable history, and restart restoration; 38 tests | Native downloads remain sequential and validate Telegram's declared length rather than a content hash; personal API Hashes remain in unencrypted SQLite at the requested alpha tradeoff |
| GPUI desktop | Conditional startup credential prompt with skip path, live-localized API/settings screens, real API-panel action, dual-method QR/phone login, compact channel navigation, a virtualized 5,000-message file table with collapsible time/multi-type filters, per-row/select-all batch selection, direct download, and a fixed right-side message inspector; expandable native transfer groups with multi-select/select-all/Open/Show in Finder plus timing, Trace ID, verification timeline, actionable failure details, live overall rate/disk metrics, diagnostic log disclosure/open action, last-window process termination, and recoverable macOS full-screen routes backed by localized AppKit application/View/Window menus; compact/standard/spacious layouts; 44 GUI tests | Encrypted Upload/Vault workflows remain previews; Library rows remain paged but not virtualized |
| Localization | Complete synchronized Fluent catalogs for `en-US`, `zh-CN`, and `ja-JP`; live switching, persistent explicit override, System Default; 21 tests | Native-speaker, assistive-technology, and pixel-level locale review remain |
| Telegram adapter | `grammers` 0.10 connection, short-lived QR login with DC migration, code/2FA, dialogs, refetch by message identity, bounded cursor scans, upload/download, structured errors, chunk observer/control, resumable private partials, atomic no-replace publication, and a versioned atomic `0600` session cache; 13 tests | Native download checks Telegram's byte length rather than a content hash; ordinary tests use no live credentials and OS credential-store UX remains |
| Historical Index Engine | Desktop Telegram-to-SQLite bounded scan with durable cursor plus the separate CAS/range coordinator and its 12 deterministic tests | Full coordinator repository mapping, live updates, retry owner, pause/cancel UI, and range compaction remain |
| Transfer Engine | Bounded encrypted scheduler plus native positional files, BLAKE3, SQLite checkpoints, encrypted remote objects, Telegram byte-object adapter, reconciliation and safe `.partial` finalization; the desktop separately owns a bounded durable real native-download queue with control and resume; 25 engine tests plus runtime integration | Encrypted engine is cooperative and buffers one application part (temporarily capped at 60 MiB); encrypted desktop ownership, true large-part streaming, and bandwidth control remain |
| Crypto/manifest candidate | Explicit bounded codecs, authenticated remote Manifest publication/discovery, File Key recovery, locator-bound download after fresh-SQLite recovery, 35 deterministic crypto tests, two libFuzzer targets, daily fuzz workflow, and an external audit package | Formats remain provisional: no independent review, production key vault/unlock UX, long campaign evidence, generation-conflict policy, or compatibility promise |
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

The current process-launch smoke matrix covers:

```text
Library, Upload, Transfers, File Detail, Key Vault,
Channel Index Detail, Settings
x en-US, zh-CN, ja-JP
x 960x640, 1360x760, 1920x1080
= 63 launches
```

All 63 configurations started and remained alive for the bounded smoke period.
This verifies startup and gross layout-policy selection, not actual pixels.
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

The workspace test suite contains 233 deterministic tests: Core 25, Crypto 35,
GUI 44, i18n 21, Index 12, Runtime 38, Storage 20, Telegram 13, and Transfer 25.
The current gate passes format, check, strict Clippy, all workspace tests,
warning-denied docs, dependency policy, and diff validation.

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

## Known gaps and risks

- Desktop QR/code login, source selection, bounded document browsing/download,
  and bounded indexing are real. The
  richer generic Index coordinator is not yet the production desktop owner.
- Transfer now has concrete native-file, SQLite, crypto, and Telegram adapters,
  plus fake-remote crash/restart and database-loss integration. It still needs
  a retained async desktop owner for the encrypted engine, streaming beyond the
  temporary 60 MiB part cap, credentialed Telegram system tests, and more
  finalization crash injection. The real native-download worker now has byte
  progress, durable resume, pause/resume/cancel/retry, but still lacks automatic
  retry policy, parallelism, bandwidth control, and a content hash.
- Crypto/manifest candidate vectors are not a stable released format. The
  fake-remote recovery proof is engineering evidence, not a released recovery
  guarantee or an independent security assessment.
- Scheduled fuzzing is continuous regression pressure, not proof of security.
  An independent reviewer must still sign the exact release candidate, and the
  format cannot be frozen until encrypted database-loss recovery is integrated.
- Library rows are paged but not virtualized; million-record performance and UI
  memory behavior are unmeasured.
- Existing imported source files can move or disappear; open/reveal uses the
  retained original path and the runtime returns structured source errors on a
  later operation.
- Local catalog metadata includes filenames and absolute source paths. The
  per-user data directory is restricted on supported Unix systems, but device
  account security and backups still matter.
- Settings navigation is connected, but Upload defaults currently configure
  the preview form rather than a production encrypted-upload owner. Personal
  API Hashes persist in unencrypted SQLite; distributor hashes are
  extractable from their build binary. Neither is an account session secret,
  but both require appropriate local/release handling. Collections are shown
  as explicit preview-only states rather than leaking unfiltered real Library
  rows.
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
2. Replace the current bounded per-part buffers with streaming native-file ↔
   crypto-frame ↔ Telegram pipes and give the worker retained async ownership.
3. Connect the proven encrypted transfer/recovery composition to desktop Upload,
   File Detail, Transfers controls, and Key Vault; reuse the native download
   queue's durable progress/control presentation where appropriate.
4. Virtualize Library/Transfer lists and benchmark 1,000 to 3,000,000 records.
5. Run unlocked reference screenshot comparison at the matrix sizes/locales,
   then complete keyboard and assistive-technology review.
6. Add account/session/keychain UX and only then enable the currently Preview
   product routes.
7. Run longer recorded fuzz campaigns, obtain independent crypto/security
   review, stabilize versioned fixtures, and validate packaging/signing.

## Handoff rule

Update this file whenever code changes a capability row. Rendered UI is never
proof of backend completion, and passing fake-port tests is never represented as
real Telegram, encryption, or recovery behavior.
