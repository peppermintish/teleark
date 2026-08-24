# TeleArk Agent and Contributor Guide

These rules apply to the whole repository. A deeper `AGENTS.md`, if added later, may add subsystem-specific rules but may not weaken the architectural, security, licensing, compatibility, or internationalization requirements here.

## Before changing anything

1. Read `docs/ARCHITECTURE.md` completely.
2. Read the documents for every subsystem the change touches. Any user-facing text change also requires reading `docs/I18N.md` and `docs/UI_GUIDELINES.md`.
3. Read `docs/IMPLEMENTATION_STATUS.md`; plans and format proposals are not proof of implementation.
4. Inspect `git status`, `git diff`, and recent history with `git log --oneline -n 10`.
5. Preserve unrelated user and agent changes. Never use destructive cleanup commands against work you do not own.

## Non-negotiable engineering rules

- Preserve the Core/frontend boundary. Core, Storage, Telegram, Crypto, Index, and Transfer code must not depend on GPUI or its types.
- The GUI calls frontend-neutral Core APIs. It must not run SQL, call `grammers`, manage Telegram sessions, perform cryptography, or implement transfer checkpoints.
- Keep `grammers` types inside the Telegram adapter and SQL inside storage repositories.
- The primary domain abstraction is `LogicalFile`; users and collections never manage Telegram messages or multipart pieces directly.
- Use structured domain errors and events. Never drive behavior by matching error strings or return localized strings from Core.
- Preserve persistent-format compatibility. Rust in-memory or Serde layout is never an implicit durable format. Format changes require docs, versioning, fixtures, compatibility tests, and usually an ADR.
- Prefer safe, readable Rust. Production filesystem/network/database/crypto/input paths must not casually `unwrap`, `expect`, or panic.
- Long-running async tasks require an owner, bounded concurrency/backpressure, cancellation, error handling, and a retained lifecycle handle. Do not hold locks across network or other long awaits.
- Add regression tests for reproducible bug fixes and deterministic tests for new behavior. Ordinary CI must not need live Telegram credentials.
- Never leave the stable branch uncompilable. Make small, meaningful commits when authorized, and keep code, tests, docs, fixtures, and ADRs synchronized.
- Record unfinished work, limitations, and next steps in `docs/IMPLEMENTATION_STATUS.md`.

## Legal clean-room and dependency policy

TeleArk is `MIT OR Apache-2.0` and must remain suitable for permissive distribution.

**Never inspect, copy, translate, port, adapt, derive from, or use implementation details from the GPL-licensed Go project `tdl`.** This includes its source, tests, internal architecture, schemas, naming, control flow, Telegram RPC sequencing, and summaries produced by another person or agent. Do not ask another agent to inspect it. Do not use superficially rewritten GPL code.

Use clean-room sources: official Telegram/MTProto documentation, public `grammers` documentation/API, Rust crate documentation, public specifications, and original TeleArk design/test work. Verify an external implementation's license before inspecting source for implementation guidance; if it is GPL, AGPL, incompatible, restrictive, or unclear, stop and seek review.

Before adding or upgrading a dependency, review its direct and relevant transitive licenses, maintenance, security posture, and necessity. Prefer MIT, Apache-2.0, BSD, ISC, Zlib, or similarly permissive terms. Document important decisions and do not bypass dependency/security checks merely to make CI pass.

## Internationalization

- No user-facing strings may be hard-coded in GUI, menus, dialogs, accessibility labels, notifications, validation, or end-user CLI output.
- New UI text uses stable semantic message IDs.
- Update `en-US`, `zh-CN`, and `ja-JP` resources together; `en-US` is the canonical fallback.
- Core errors and persisted domain values remain structured and locale-neutral.
- Never translate or modify user/source content such as filenames, captions, channel titles, paths, or collection names.
- Use centralized locale-aware formatting for human-readable dates, numbers, percentages, speeds, and sizes. Keep protocol IDs, hashes, and offsets canonical.
- Run i18n parse, catalog-completeness, variable, fallback, negotiation, and error-mapping validation before commit.
- Update `docs/I18N.md` when localization architecture, terminology, or policy changes.

## Documentation and decisions

Update relevant documents when behavior, boundaries, schemas, state machines, protocol formats, security assumptions, terminology, or developer workflows change. Significant or hard-to-reverse choices require an ADR under `docs/adr/`; supersede old ADRs instead of rewriting their history after adoption.

Format documents currently marked provisional describe intended v1 designs, not shipped compatibility guarantees. Do not remove that marker until code, independent security review, golden vectors/fixtures, tamper tests, and recovery tests exist.

## Required pre-commit checklist

Before a substantial commit, verify every applicable item and explicitly record any justified exception:

```text
[ ] Core has no GUI dependency
[ ] GUI does not directly call SQLite or grammers
[ ] No implementation was taken from tdl or another incompatible source
[ ] New dependency licenses were reviewed
[ ] Domain terminology is consistent
[ ] No user-facing string bypasses the i18n system
[ ] en-US / zh-CN / ja-JP resources are synchronized
[ ] i18n validation passes
[ ] New behavior has tests
[ ] Bug fixes have regression tests where practical
[ ] State transitions are tested
[ ] Database migrations are tested
[ ] Crypto vectors still pass
[ ] Manifest compatibility fixtures still pass
[ ] No unnecessary unwrap/panic was added
[ ] No unbounded spawning/channel was added without justification
[ ] Locks are not held across long awaits
[ ] Persistent formats were not changed accidentally
[ ] Relevant docs were updated
[ ] cargo fmt --all --check passes
[ ] cargo check --workspace passes
[ ] cargo clippy --workspace --all-targets -- -D warnings passes
[ ] cargo test --workspace passes
[ ] git diff was reviewed
[ ] Commit message describes one coherent change
```

If a command is temporarily unavailable because its crate/fixture does not exist in the current milestone, note that fact in `docs/IMPLEMENTATION_STATUS.md`; do not represent it as passing.

## Handoff

Before ending incomplete work, make the stable portion compile, run proportionate checks, update subsystem docs and implementation status, list known issues and precise next actions, and create coherent commits only when the user/session authorizes commits. Never commit secrets, real Telegram sessions, private channel data, phone numbers, recovery keys, or user documents.
