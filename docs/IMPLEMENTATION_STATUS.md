# Implementation Status

Last updated: 2026-08-27

## Current milestone

TeleArk is a **usable persistent local-catalog alpha with tested backend
foundations**. A user can import local file metadata, keep it between launches,
search and filter it, page through results, inspect a real file, open it with
the system application, reveal it in Finder, and persist an interface language
or follow System Default.

Telegram authentication/transport, historical indexing, transfer scheduling,
and provisional crypto formats now have frontend-neutral implementations and
deterministic tests. They are not yet composed into an end-to-end desktop
Telegram/Vault workflow. Transfers, Channel Index, Upload, Key Vault, and most
Settings content therefore remain visibly marked Preview.

## Implemented capability

| Area | Current evidence | Deliberate limit |
| --- | --- | --- |
| Core application API | Typed Library queries, imports, pages, statistics, repository port, structured errors; 25 Core tests total | Transfer/Index/Vault services are separate foundations, not one application command bus |
| Persistent Library | SQLite migrations v1-v4, strict tables, FTS5, facets, exact result counts, keyset cursors, settings, collections, index rows, transfer checkpoints; 15 temporary-database tests | No automatic source watching, deduplication policy, collection editor, or million-row benchmark yet |
| Desktop runtime | Bounded storage-worker lifecycle, default per-user DB, import inspection, path fidelity, locale override persistence; 4 tests | Runtime currently composes only the local Library/settings path |
| GPUI desktop | Real Library import/search/filter/detail/pagination/open/reveal; compact/standard/spacious layouts; explicit Preview states; 26 GUI tests after this milestone | Other product routes still use deterministic fixtures; large lists are bounded by pages but not yet virtualized |
| Localization | Complete synchronized Fluent catalogs for `en-US`, `zh-CN`, and `ja-JP`; live switching, persistent explicit override, System Default; 20 tests | Native-speaker, assistive-technology, and pixel-level locale review remain |
| Telegram adapter | `grammers` 0.10 session connection, authorization/2FA flow, dialogs, bounded history scan, native upload/download, sign-out/shutdown, structured FloodWait/errors; 4 adapter tests | Not connected to GUI/runtime; ordinary tests use no live credentials; production session/keychain UX remains |
| Historical Index Engine | Bounded coordinator, content policy, redacted cursors, CAS checkpoints, deterministic idempotent batches, pause/cancel/retry/FloodWait; 12 tests including 10,000-record resume | Concrete Telegram/SQLite adapters, incremental updates, retry timing owner, range compaction, and real coverage UI remain |
| Transfer Engine | Bounded scheduler, global/direction/account/file limits, priority/FIFO, controls, retry/FloodWait, progress coalescing, checkpoint/I/O ports, reconciliation and safe `.partial` finalization; 22 tests | Execution is cooperative/synchronous against fakes; real async Telegram/SQLite/crypto/filesystem streaming and bandwidth control remain |
| Crypto/manifest candidate | Explicit bounded codecs, AES-256-GCM framing, HKDF/BLAKE3/Argon2id, key wrapping, redacted secrets/metadata, mandatory AEAD-usage registry and candidate fixtures; 35 tests | Formats remain provisional: no independent security review, production key vault, integrated recovery, fuzz campaign, or shipped compatibility promise |
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
- The local database is an index/checkpoint/settings store, not proof that a
  remote encrypted package can be recovered.
- Crypto and manifest encodings are implemented candidates but remain marked
  provisional until external review and recovery validation are complete.

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
This verifies startup and gross layout-policy selection, not actual pixels. The
macOS desktop remained locked/privacy-restricted during automated capture, so
reference-image overlay comparison and exhaustive real text-clipping inspection
must still be completed on an unlocked desktop.

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

The workspace test suite contains 163 deterministic tests after the current GUI
usability additions: Core 25, Crypto 35, GUI 26, i18n 20, Index 12, Runtime 4,
Storage 15, Telegram 4, and Transfer 22. Final gate results for the exact commit
are recorded in the session handoff after all checks are rerun.

The published GPUI dependency graph currently reports future-incompatibility
warnings for transitive `block 0.1.6` and `proc-macro-error2 2.0.1`; the workspace
itself is warning-clean under Clippy. Existing documented RustSec and license
policy exceptions remain governed by `deny.toml` and third-party notices.

## Known gaps and risks

- No desktop flow yet logs into Telegram, selects a real storage channel,
  invokes the Index coordinator, or runs the Transfer engine.
- The current Index and Transfer engines have strong fake-backed behavior but
  still need concrete adapters, async lifecycle ownership, crash/restart
  integration, and end-to-end failure injection.
- Crypto/manifest candidate vectors are not a stable released format and must
  not be treated as a recovery guarantee.
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
- Actual pixel comparison at multiple window sizes remains blocked until the
  macOS desktop is unlocked for capture.

## Next implementation sequence

1. Compose the Telegram adapter, Index coordinator, and SQLite repositories
   behind Runtime services, then replace Channel Index fixtures.
2. Adapt Transfer ports to Telegram, Storage, Crypto, and a streaming native
   filesystem implementation with retained async ownership and bounded queues.
3. Complete fake-remote upload, database-loss recovery, authenticated manifest
   discovery, and exact download-equality tests before enabling Vault claims.
4. Virtualize Library/Transfer lists and benchmark 1,000 to 3,000,000 records.
5. Run unlocked reference screenshot comparison at the matrix sizes/locales,
   then complete keyboard and assistive-technology review.
6. Add account/session/keychain UX and only then enable the currently Preview
   product routes.
7. Obtain independent crypto/security review, stabilize versioned fixtures,
   and validate production packaging/signing/notarization.

## Handoff rule

Update this file whenever code changes a capability row. Rendered UI is never
proof of backend completion, and passing fake-port tests is never represented as
real Telegram, encryption, or recovery behavior.
