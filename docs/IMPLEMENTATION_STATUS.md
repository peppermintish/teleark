# Implementation Status

Last updated: 2026-08-28

## Current milestone

TeleArk is a **persistent local-catalog alpha with real Telegram indexing and a
production-adapter transfer foundation**. In addition to local import/search/open/reveal, the desktop now
accepts Telegram API credentials, performs code and 2FA authorization, lists
real dialogs, and scans a selected source in bounded pages into SQLite/FTS5.

The native Telegram indexing entry path is real, but it is not yet the complete
CAS/range coordinator and does not include live-update ingestion. The runtime
now composes real native files, SQLite checkpoints, framed encryption, remote
object reconciliation, encrypted Manifest publication, and database-loss
recovery; the desktop Transfer/Vault routes and key-unlock UX remain preview-only.

## Implemented capability

| Area | Current evidence | Deliberate limit |
| --- | --- | --- |
| Core application API | Typed Library queries, imports, pages, statistics, repository port, structured errors; 25 Core tests total | Transfer/Index/Vault services are separate foundations, not one application command bus |
| Persistent Library | SQLite migrations v1-v5, strict tables, FTS5, Telegram remote identities/cursors, facets, settings, collections, index rows, transfer checkpoints; 17 temporary-database tests | Vault package/manifest tables, collection editor, and million-row benchmark remain |
| Desktop runtime | Bounded storage and Tokio Telegram workers, login/2FA, dialog discovery, idempotent document projection, restart cursor persistence, local import and locale persistence; 6 tests | Telegram credentials are entered per connection; OS credential-store integration remains |
| GPUI desktop | Real Library and Telegram connection/source scan routes; compact/standard/spacious layouts; 26 GUI tests | Transfer/Vault routes still use fixtures; large lists are bounded but not virtualized |
| Localization | Complete synchronized Fluent catalogs for `en-US`, `zh-CN`, and `ja-JP`; live switching, persistent explicit override, System Default; 20 tests | Native-speaker, assistive-technology, and pixel-level locale review remain |
| Telegram adapter | `grammers` 0.10 connection, code/2FA, dialogs, refetch by message identity, bounded cursor scans, upload/download, structured errors, and a versioned atomic `0600` session cache; 6 tests | Ordinary tests use no live credentials; OS credential-store UX remains |
| Historical Index Engine | Desktop Telegram-to-SQLite bounded scan with durable cursor plus the separate CAS/range coordinator and its 12 deterministic tests | Full coordinator repository mapping, live updates, retry owner, pause/cancel UI, and range compaction remain |
| Transfer Engine | Bounded scheduler plus native positional files, BLAKE3, SQLite checkpoint/state persistence, encrypted remote objects, Telegram byte-object adapter, reconciliation and safe `.partial` finalization; 25 engine tests and runtime integration coverage | The engine is still cooperative and currently buffers one application part (temporarily capped at 60 MiB); retained async ownership, true large-part streaming, bandwidth control, and desktop route wiring remain |
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
- The local database is an index/checkpoint/settings cache. The fake-remote
  acceptance path now proves recovery from an authenticated remote Manifest;
  live Telegram recovery still needs credentialed system testing and desktop UX.
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
Unlocked macOS captures additionally verify the Telegram login route at
`900x600` in English, `1360x760` in Simplified Chinese, and a requested
`1920x1080` in Japanese. Oversized requests are now fitted to the current
display before centering, preventing off-screen content.

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

The workspace test suite contains 176 deterministic tests: Core 25, Crypto 35,
GUI 26, i18n 20, Index 12, Runtime 10, Storage 17, Telegram 6, and Transfer 25.
The current gate passes format, check, strict Clippy, all workspace tests,
warning-denied docs, dependency policy, and diff validation.

The published GPUI dependency graph currently reports future-incompatibility
warnings for transitive `block 0.1.6` and `proc-macro-error2 2.0.1`; the workspace
itself is warning-clean under Clippy. Existing documented RustSec and license
policy exceptions remain governed by `deny.toml` and third-party notices.

## Known gaps and risks

- Desktop login, source selection, and bounded document indexing are real. The
  richer generic Index coordinator is not yet the production desktop owner.
- Transfer now has concrete native-file, SQLite, crypto, and Telegram adapters,
  plus fake-remote crash/restart and database-loss integration. It still needs
  a retained async desktop owner, streaming beyond the temporary 60 MiB part
  cap, credentialed Telegram system tests, and more finalization crash injection.
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
- Settings other than language are previews. Collections are shown as explicit
  preview-only states rather than leaking unfiltered real Library rows.
- Accessibility, focus trapping, reduced motion, screen-reader labels, and
  native-speaker wording still need dedicated product review.
- Actual screenshots now verify the Telegram login route at `900x600` in
  English, `1360x760` in Simplified Chinese, and display-fitted `1920x1080` in
  Japanese. The full route/locale pixel matrix and authenticated channel-list
  states still need capture.

## Next implementation sequence

1. Map the generic Index coordinator repository and retry/cancellation policy
   onto the working bounded Telegram-to-SQLite desktop scan path.
2. Replace the current bounded per-part buffers with streaming native-file ↔
   crypto-frame ↔ Telegram pipes and give the worker retained async ownership.
3. Connect the proven transfer/recovery composition to desktop Upload,
   Transfers, File Detail, and Key Vault routes with localized controls.
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
