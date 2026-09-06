# TeleArk contributor rules

Read [architecture](docs/ARCHITECTURE.md) and [current status](docs/IMPLEMENTATION_STATUS.md) before changing code. Inspect `git status`, `git diff`, and `git log --oneline -n 10`; preserve unrelated work. Read only the additional documents relevant to the change:

| Change | Required reference |
| --- | --- |
| GUI or user-facing text | [UI](docs/UI_GUIDELINES.md), [i18n](docs/I18N.md) |
| Storage, identity or settings | [Data model](docs/DATA_MODEL.md), [preferences](docs/PREFERENCES_FORMAT.md) |
| Index/search | [Index](docs/INDEX_ENGINE.md) |
| Transfers or diagnostics | [Transfer](docs/TRANSFER_ENGINE.md), [security](docs/SECURITY.md) |
| Crypto, manifests or recovery | [Crypto](docs/CRYPTO_FORMAT.md), [manifest](docs/MANIFEST_FORMAT.md), [security](docs/SECURITY.md) |
| Build, tests or releases | [Development](docs/DEVELOPMENT.md) |

## Invariants

- Core, Runtime, Storage, Telegram, Crypto, Index and Transfer must not depend on GPUI. GUI calls frontend-neutral APIs; it does not execute SQL, call `grammers`, manage sessions, perform crypto or implement checkpoints. SQL stays in Storage; `grammers` types stay in Telegram.
- `LogicalFile` is the domain abstraction. Use structured errors/events and locale-neutral persisted values; never match error strings for behavior.
- Persistent bytes require explicit codecs, versions, documentation, fixtures and compatibility tests. Rust/Serde layout is not a durable format. Significant decisions require a new ADR; supersede accepted ADRs instead of rewriting history. Keep crypto/manifest **provisional** until their independent review and stabilization gates pass.
- Use safe Rust. Production filesystem/network/database/crypto/input paths must not casually unwrap, expect or panic. Long operations need an owner, bounded work/queues, cancellation, error handling and retained handles. Do not hold locks across network or long awaits.
- Every new behavior needs deterministic tests; reproducible fixes need regression tests. Normal CI must not use live Telegram credentials. Keep stable code compiling and preserve existing user data.
- All interface text—including menus, accessibility, dialogs, validation, notifications and end-user CLI output—uses semantic message IDs in synchronized `en-US`, `zh-CN`, `ja-JP` catalogs; English is fallback. Use centralized locale-aware formatting. Never translate filenames, captions, paths, channel titles or collection names.

## Provenance and dependencies

TeleArk remains `MIT OR Apache-2.0`. **Never inspect, copy, translate, port, adapt, derive from, or use implementation details from GPL `tdl`**, including source, tests, architecture, schemas, naming, control flow, RPC sequencing, or other agents' summaries. Do not delegate that inspection. Use official Telegram/MTProto docs, public `grammers` APIs, compatible crate documentation/specifications and original TeleArk work.

Verify an external implementation's license before inspecting it. Stop and seek review for GPL/AGPL, restrictive, incompatible or unclear terms. Before adding/upgrading dependencies, review direct/relevant transitive licenses, maintenance, security and necessity; prefer permissive terms. Keep notices, `deny.toml` and the lockfile synchronized; never bypass gates to obtain a pass. ADRs [0007](docs/adr/0007-permissive-license-clean-room-policy.md) and [0012](docs/adr/0012-gpui-kit-and-private-storage-channel.md) record the GUI provenance decisions.

## Before committing or handing off

Review applicable boundaries, terminology, i18n, state transitions, migrations, crypto vectors, manifest fixtures, concurrency, secret handling and the complete diff. Run the [quality gates](docs/DEVELOPMENT.md#quality-gates). Record justified exceptions and unfinished work with precise next actions in `IMPLEMENTATION_STATUS.md`; unavailable checks are not passes. Update affected contracts with behavior changes. Commit only when the session authorizes it, with one coherent change per commit. Never commit real sessions, credentials, private channel data, recovery keys or user documents.
