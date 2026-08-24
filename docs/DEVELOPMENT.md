# Development Guide

This guide describes the intended contributor workflow. TeleArk is currently at a foundation/mock-UI milestone; commands for future crates or fixtures are requirements to add as those components land, not evidence that they already exist.

## Start-of-work audit

Before editing:

```bash
git status
git diff
git log --oneline -n 10
```

Then read `../AGENTS.md`, `ARCHITECTURE.md`, `IMPLEMENTATION_STATUS.md`, and every relevant subsystem document. Preserve unrelated uncommitted changes. Do not use `git reset --hard`, `git clean -fd`, `git checkout .`, or equivalent destructive cleanup on user work.

For repository audits, distinguish four categories explicitly:

1. implemented and tested;
2. mock/demo-only;
3. documented target design;
4. not started or blocked.

## Baseline toolchain and commands

Use the repository-selected toolchain. `rust-toolchain.toml` selects the stable channel with Rustfmt and Clippy, while the workspace manifest declares the minimum supported Rust version. Change these deliberately and keep local/CI policy synchronized.

```bash
cargo fmt --all --check
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

CI and local milestone checks also prove the frontend-independent `teleark-core` package independently:

```bash
cargo test -p teleark-core
```

Do not add a check to status documentation until it is actually runnable. The GitHub workflows use workspace-level commands so the repository can evolve from one package to multiple crates.

### macOS GUI prerequisites

The current GPUI target renders through Metal on macOS, but this workspace enables GPUI's `runtime_shaders` feature. The development build therefore compiles and launches with Apple Command Line Tools even when the standalone `metal` compiler is unavailable. Keep that feature enabled unless the replacement build path is validated. Full release/signing/notarization work will still require an explicit Xcode-based packaging environment.

Upstream has not published a standalone minimum macOS version for this exact TeleArk dependency pair. Define and test TeleArk's own deployment floor before release rather than inheriting claims from the Zed application. Both Apple Silicon and Intel need explicit build/run verification.

## Build boundaries

- Core/domain code contains no GPUI imports or concepts.
- GUI calls Core-facing services; it does not issue SQL, call `grammers`, perform crypto, or manage checkpoints.
- Storage centralizes SQL and migrations.
- Telegram integration maps `grammers` data/errors at the adapter edge.
- Crypto codecs and keys remain independent of GUI and OS credential-store implementations.
- User-facing prose is resolved through the i18n layer from structured data/errors.

Use traits where they define a meaningful infrastructure seam or enable a deterministic fake. Avoid a trait for every struct and avoid a global `Arc<Mutex<AppState>>` as an architectural shortcut.

## Adding dependencies

Before adding or upgrading a crate:

1. verify its current license from authoritative package/repository metadata;
2. inspect relevant transitive licenses;
3. assess maintenance and security posture;
4. confirm the dependency provides material value;
5. record a significant decision in an ADR;
6. update dependency-deny policy when that configuration is introduced.

Prefer permissive licenses. GPL/AGPL, unclear, unusual, reciprocal, or restrictive dependencies require explicit review and may not be adopted merely for convenience. Never inspect GPL `tdl` source for guidance; use official Telegram/MTProto and `grammers` documentation.

The reviewed GUI baseline is pinned to crates.io `gpui = 0.2.2`, `gpui-component = 0.5.1`, and `gpui-component-assets = 0.5.1`. Do not replace these with the current Zed/git-main dependency path without a fresh license audit: the reviewed git graph pulled GPL-3.0-or-later `ztracing`/`zlog`, contrary to TeleArk's distribution policy. Keep `THIRD_PARTY_NOTICES.md` with distributions that embed the icon bundle. The current graph's MPL-2.0 `cbindgen`, `dwrote`, and `option-ext` declarations are documented compatibility exceptions, not evidence of a permissive-only graph. See ADR 0007.

## Testing strategy

Use many focused unit/domain tests, a smaller number of integration tests, and few end-to-end tests. The ordinary test suite is deterministic, isolated, free of real credentials, and does not require live network services. Dependency resolution and advisory updates are separate CI concerns.

High-risk test areas include:

- domain invariants and state-machine transitions;
- migrations and data-preserving upgrades;
- FTS/search semantics and cursor pagination without gaps/duplicates;
- partial index range merge/resume and out-of-order updates;
- scheduler limits, retry/FloodWait, pause/resume/cancel;
- source mutation and cross-system crash injection;
- frame tamper, wrong key/password/recovery key, reorder, omission, and truncation;
- fixed crypto vectors and manifest compatibility fixtures;
- `.partial` safety and whole-file recovery;
- localization parse/completeness/fallback/variables/negotiation.

Tests own temporary databases, files, fake clocks, fake credential stores, and fake Telegram transports. They never use a developer's actual database or session. Avoid wall-clock sleeps; use notifications, injected time, or controlled async primitives.

Long fuzzing, million-row benchmarks, real Telegram tests, and packaging smoke tests belong in manual/nightly/protected workflows. Real-network tests use dedicated accounts and secrets and never commit sessions or private content.

## Database changes

Every schema change is an ordered migration. Test empty-to-latest and every supported released upgrade path, including data preservation and schema invariants. Keep SQL in storage repositories. Use transactions and prepared/batched writes; choose WAL, busy timeout, indexes, and batch sizes based on measured behavior rather than folklore.

Do not delete or recreate user databases to simplify development. Synthetic large-data generators are preferred over committed multi-million-row database blobs.

## Persistent-format changes

Before implementing or changing crypto, manifest, vault-container, or other durable bytes:

1. update the relevant format document and ADR;
2. define supported/unsupported version behavior and parser limits;
3. keep persistent schema separate from Rust in-memory structs;
4. add or update golden vectors/fixtures;
5. test tamper, truncation, malformed lengths, unknown algorithms, and old-version reads;
6. obtain cryptographic/security review for security-sensitive changes;
7. update recovery tests and implementation status.

Never silently reinterpret released bytes. A codec refactor is complete only when old fixtures remain readable.

## Internationalization workflow

Read `I18N.md` before changing UI or user-facing CLI text. Add one semantic ID to all three required catalogs (`en-US`, `zh-CN`, `ja-JP`), use parameters/selectors instead of concatenated fragments, map structured errors at the presentation boundary, and run catalog validation/tests. Check the main screens at all three locales for clipping and overlap.

User/source content—filenames, captions, channel titles, paths, collection names—must never be translated. Store stable enum/state values rather than localized strings.

## UI workflow

The supplied reference images are the visual source of truth. Implement against centralized design tokens and reusable controls, then run and capture the actual app at reference-like dimensions. Compare geometry, typography, spacing, borders, radii, tables, status colors, progress bars, and three-locale text expansion. Stock components may be replaced with custom GPUI components when necessary for fidelity or virtualization.

Mock data must be clearly separated from Core contracts and must not be represented as backend integration. Heavy I/O, SQLite, hashing, crypto, and Telegram calls never run on the GPUI thread.

## Commits and handoff

When commits are authorized, keep each commit coherent, compiling, tested in proportion to risk, and synchronized with documentation. Review the complete diff and use domain-oriented messages such as `docs: establish clean-room policy` or `feat(index): add resumable index ranges`.

Before handing off unfinished work:

- stabilize and check the implemented portion;
- update `IMPLEMENTATION_STATUS.md` with facts, known issues, and next actions;
- record deliberately skipped checks and why;
- ensure no secrets or private fixtures are present;
- leave precise continuation guidance in code/docs rather than relying on chat.

## CI and releases

`ci.yml` runs formatting, workspace check, Clippy, tests, an explicit frontend-independent Core test, and i18n package validation without Telegram credentials. It also verifies both repository licenses and runs the repository's `cargo-deny` license, advisory, source, and ban policy. The policy explicitly rejects the `ztracing`/`ztracing_macro`/`zlog` GUI graph from ADR 0007 and records the reviewed MPL-2.0 exceptions. Keep `deny.toml` and the locked dependency graph under review together.

Known unmaintained advisories in the published GPUI 0.2.2/component 0.5.1 transitive graph are maintenance debt rather than reported vulnerabilities. The audit follows cargo-deny's recommended scope by failing unmaintained workspace dependencies, all unsound advisories, all vulnerability advisories, and yanked packages. Re-evaluate the exact GUI pins and this policy together when an upstream permissive release provides a migration path.

`release.yml` is a conservative unsigned macOS bootstrap because supported GPUI platforms and packaging/signing are not yet validated. Tagged releases must pass quality checks before packaging, publish SHA-256 checksums, and use least-privilege permissions. Add platforms only after the GUI is built and tested there; signing and notarization secrets belong in protected GitHub environments.
