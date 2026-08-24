# Implementation Status

Last updated: 2026-08-24

## Current milestone

TeleArk is in **Phase 0 engineering foundation / Phase 1 mock-interface work**. This session is intentionally limited to repository policy, architecture and format design, licensing, baseline CI/release scaffolding, localization foundations, and a GPUI interface driven by synthetic/mock data.

Nothing in the reference UI or design documents proves backend capability. Do not use the prototype for real storage, transfer, encryption, or recovery.

## Repository audit baseline

At the start of this milestone the repository contained one uncommitted Rust package (`teleark`), an empty dependency list, a `Hello, world!` `main.rs`, and a minimal `/target` ignore rule. The `master` branch had no commits. There were no workspace crates, GPUI usage, Telegram integration, SQLite schema, transfer/index engine, tests, documentation, license files, i18n resources, or GitHub Actions.

## Current repository audit

1. **Workspace architecture:** separate `teleark-core`, `teleark-i18n`, and `teleark-gui` foundation crates now exist. Storage, Telegram, Crypto, CLI, and test-support crates do not.
2. **Core/GUI coupling:** the package graph points GUI -> Core/i18n. Core has no GUI, persistence, networking, or localization dependency. The GUI currently consumes i18n but still renders explicit mock view fixtures rather than Core application snapshots; no infrastructure adapter or Core service integration is present.
3. **GPUI usage:** the GUI is pinned to published GPUI/component releases and implements centralized theme/components plus mock routes for Library, Transfers, File Detail, Key Vault, Channel Index, Settings, and Upload. It builds and launches on the development Mac through GPUI's `runtime_shaders` path. All seven routes passed on-screen startup smoke checks across `en-US`, `zh-CN`, and `ja-JP`; automated pixel capture was blocked by host screen-recording privacy controls, so overlay comparison and exhaustive text-expansion review remain.
4. **Telegram:** no `grammers`, authentication, session, history, update, upload, or download implementation exists.
5. **SQLite:** no SQLite dependency, schema, migration, FTS5, repository, or real settings persistence exists.
6. **Transfers/indexing:** Core contains foundation domain/state/range invariants only. There is no scheduler, worker, checkpoint repository, remote transport, historical scanner, incremental sync, or search backend.
7. **Git:** the branch had no commits at milestone start; all initial files were uncommitted. Preserve concurrent changes and do not infer history that does not exist.
8. **Documentation:** mandatory architecture, subsystem, security, i18n, development, status, and ADR documents now exist; crypto/manifest bytes remain explicitly provisional.
9. **CI/release:** baseline GitHub workflows are present, run the ADR-0007 guard plus `cargo-deny`, and pass local `actionlint`; GitHub-hosted execution, signing, notarization, and broader packaging are not validated.
10. **Licensing:** repository code is dual MIT/Apache-2.0. Exact published GPUI pins avoid the reviewed incompatible git-main path. `THIRD_PARTY_NOTICES.md` carries Lucide ISC and Feather MIT attribution for the embedded icon bundle. `deny.toml` enforces the approved license/source/advisory policy and explicitly records the reviewed MPL-2.0 build/platform/helper exceptions (`cbindgen`, `dwrote`, `option-ext`). Exhaustive generated dependency notices and final production-release legal review remain.
11. **Target conflicts:** there is no legacy application/data to migrate, but most target infrastructure is absent and current UI content is synthetic. The durable formats are design candidates, not compatibility guarantees.
12. **Migration approach:** evolve incrementally from the three-crate foundation, add adapters behind Core contracts, and introduce only ordered data-preserving database migrations once a schema exists.
13. **Implementation order:** continue with visual/i18n verification, domain services, storage, clean-room Telegram, Index/Search, Transfer, reviewed Crypto/Manifest/Recovery, then real GUI integration and production packaging.

## Capability matrix

| Area | Status | Evidence / limitation |
| --- | --- | --- |
| Dual licensing and clean-room policy | Implemented as repository policy | `LICENSE-*`, `README.md`, `AGENTS.md`, ADR 0007 |
| GUI dependency/license baseline | Implemented as a reviewed foundation choice | Exact crates.io pins; git-main GPL path prohibited; `cargo-deny` policy passes; Lucide/Feather notice included; MPL transitive declarations recorded; exhaustive generated notices remain |
| Architecture/subsystem documentation | Implemented as target design | `docs/` and ADRs; many details remain provisional until tested |
| Baseline CI/release scaffolding | Implemented | Workspace checks and conservative unsigned macOS tag packaging; no production signing or multi-platform claim |
| GPUI visual shell | Mock/demo-only in this milestone | Synthetic Library/Transfer/File/Vault/Channel/Settings/Upload routes; mock search/facets, queue insertion, pause, vault lock/reveal, locale switching, keyboard route activation, and Escape dismissal; no backend capability |
| Localization foundation | Implemented foundation, not a completed product locale pass | Fluent catalogs/service/formatters; 423 keys in each required locale, static GUI `tr(...)` IDs covered, and 19 locale/catalog/variable/fallback/error/format/thread-safety tests; settings persistence and pixel-level locale review remain |
| Core domain foundation | Implemented foundation | Frontend-neutral IDs, logical-file/package/remote types, collection rules, structured errors/events, index ranges/jobs, transfer task/part invariants; 22 unit tests |
| Core application services | Not implemented | No command bus/use-case orchestration or infrastructure ports/adapters beyond foundation models |
| SQLite schema, migrations, FTS5 | Not implemented | No user database or migration compatibility exists |
| Telegram/grammers auth and transport | Not implemented | No login, channel scan, upload, or download |
| Index engine and global search | Not implemented | Foundation `IndexRange`/job types are not a scanner or search backend; UI values are mock data |
| Transfer scheduler/checkpoints | Not implemented | Foundation task/part transitions are not workers/checkpoints; UI progress, pause, retry, speed, and ETA are mock interactions/data |
| Multipart streaming | Not implemented | 1900 MiB is a documented target, not operational behavior |
| AES-GCM/Argon2id/key vault | Not implemented | `CRYPTO_FORMAT.md` is provisional; no security guarantee |
| Manifest/reconciliation/recovery | Not implemented | `MANIFEST_FORMAT.md` is provisional; no recovery guarantee |
| CLI | Not implemented | Core APIs are designed to permit a future frontend |
| Production packaging/signing/notarization | Not implemented | Release workflow is bootstrap scaffolding only |

## Local verification evidence

As of the date above:

```text
cargo test -p teleark-core -p teleark-i18n --locked
  -> 22 Core tests passed
  -> 19 i18n tests passed
  -> 0 failed

cargo clippy -p teleark-core -p teleark-i18n --all-targets -- -D warnings
  -> passed

cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
  -> passed, including 9 GUI argument, fixture, facet, queue, and logical-part presentation tests

target/debug/teleark --screen=<route> --locale=<locale>
  -> Library, Transfers, File, Vault, Channel, Settings, and Upload each opened an on-screen GPUI window
  -> smoke matrix covered en-US, zh-CN, and ja-JP; each process remained responsive until intentionally terminated

GUI/resource catalog audit
  -> 423 keys in each of en-US, zh-CN, and ja-JP
  -> 0 missing statically referenced tr(...) IDs

actionlint .github/workflows/ci.yml .github/workflows/release.yml
  -> passed

cargo deny check
  -> advisories, bans, licenses, and sources passed
  -> duplicate transitive versions are reported as non-blocking warnings

local Markdown link resolution check
  -> passed for README, AGENTS, subsystem docs, and ADRs
```

These checks verify compilation, linting, unit tests, and route startup across the three locales. They do not verify pixel fidelity, every interaction/text-expansion combination, GitHub-hosted Actions, real Telegram behavior, persistence, crypto, or recovery. Host privacy controls prevented automated window capture in this session.

## Known risks and gaps

- The durable crypto and manifest proposals need implementation review, independent security review, strict codecs, golden vectors, compatibility fixtures, tamper tests, and fuzzing before v1 can be declared stable.
- Current crate boundaries must evolve without introducing GPUI into Core or leaking `grammers`/SQL outward.
- Dependency versions, upstream APIs, licenses, and GPUI-supported platform targets must be verified when dependencies are selected.
- The current exact GPUI release pins intentionally avoid a reviewed git-main graph containing GPL-3.0-or-later `ztracing`/`zlog`; upgrades/features require a fresh dependency audit. The icon notice now covers Lucide/Feather, while an exhaustive generated dependency notice remains outstanding.
- The published GPUI 0.2.2/component 0.5.1 graph has transitive crates carrying unmaintained RustSec advisories with no safe in-line upgrade. `cargo-deny` continues to fail vulnerabilities, yanked packages, all unsound advisories, and unmaintained workspace dependencies; the GUI pins must be re-reviewed when an upstream permissive migration path is available.
- The published GPUI graph includes MPL-2.0 `cbindgen`, `dwrote`, and `option-ext`. They are not GPL/AGPL and do not relicense TeleArk, but they are not permissive-only declarations; production distribution must keep the documented license review explicit.
- The Lucide/Feather notice matches the reviewed asset bundle, but its upstream Lucide license reference follows a mutable branch. Pin the exact upstream icon provenance when generating the production asset inventory and exhaustive notices.
- Million-record performance, cursor correctness, index-range reconciliation, scheduler limits, and crash recovery are unmeasured.
- Current foundation `IndexRange` records message-ID coverage but does not yet include the target date/source-bound and content-policy fingerprint semantics.
- The mock UI requires screenshot comparison against both supplied references at representative sizes and all three required locales.
- Several synthetic fixture sizes, rates, dates, durations, and technical values are still stored as preformatted presentation strings; real snapshots must carry typed values and use the centralized formatters throughout.
- The Settings mock switches immediately among the three explicit locales but does not yet expose the specified System Default choice or persist an override.
- GPUI/component 0.2.2/0.5.1 provides focusable controls but this prototype still needs a dedicated assistive-technology audit, focus trapping for modal content, and broader keyboard interaction tests.
- GitHub-hosted CI and release execution remain unverified locally even though both workflow files pass `actionlint` and include the dependency audit.

## Next implementation sequence

1. Verify the centralized GPUI mock reference screens at reference dimensions in `en-US`, `zh-CN`, and `ja-JP`, then add automated interaction/layout smoke coverage where GPUI permits it.
2. Add frontend-neutral Core application services/ports and replace GUI fixtures with typed snapshots incrementally.
3. Add storage migrations/repositories, FTS5, range storage, cursor queries, and temporary-database tests.
4. Implement the clean-room `grammers` adapter using only compatible authoritative sources and fake transport contracts.
5. Implement Index Engine, then real library/search integration.
6. Implement and test the unified Transfer Engine and cross-system reconciliation with fakes.
7. Finalize reviewed crypto and manifest v1 formats, vectors, and recovery fixtures before production use.
8. Add multipart streaming upload/download and end-to-end fake-remote recovery.
9. Replace remaining GUI mocks through Core APIs without changing the established visual system.
10. Validate supported platforms, then extend packaging, signing, release checks, and dependency policy.

## Handoff rule

Update this file whenever code changes a row above. Use only `implemented`, `mock/demo-only`, `design-only`, `not implemented`, or a similarly testable statement. Record the command/test/fixture that supports an implementation claim and never infer backend completion from a rendered screen.
