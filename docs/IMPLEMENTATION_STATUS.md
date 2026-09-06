# Implementation status — v0.4.0

Updated 2026-09-07. TeleArk is an early desktop alpha with real Telegram/native-download and encrypted private-channel workflows. UI polish and deterministic engine evidence do not establish a security-reviewed, signed commercial release.

## Current capabilities

| Area | Connected behavior | Remaining limits |
| --- | --- | --- |
| Desktop | GPUI Kit 0.6.0 / gpui-pre 0.3.3, Apple-style palette, permanent Transfers/TeleArk sidebar, virtualized channels/raw/managed/transfer lists, inspectors, progressive settings, native menus/shortcuts, localized full About changelog | Full pixel/backing-scale and assistive-technology matrix outstanding |
| Account | Phone/code/2FA and QR, restored avatar/name welcome, explicit Log In/Switch Account, optional personal/distributor API configuration | Platform credential adapter disabled; credentialed end-to-end qualification outstanding |
| TeleArk storage | Create/discover/validate the active account's owned private broadcast channel, account-scoped binding independent of title, candidate selection, Files/Raw Files and help guide | No automatic migration/deletion of legacy Saved Messages; legacy recovery stays in advanced Key Vault settings |
| Vault | Retained bounded owner, password/recovery setup/unlock, password change, recovery rotation, explicit bundle export/database-loss restore, active-window lock policy | OS Credential unavailable; exported old bundles cannot be cryptographically revoked; sleep/logout qualification outstanding |
| Unlock flow | Focused modal preserves browse/upload/download intent and resumes after successful unlock/recovery acknowledgement; cancellation clears secret inputs | Actual login/key workflows require protected credentialed review |
| Native browsing/index | Cached source rows, 200-message cancellable transport pages, up to 5,000 virtualized rows, time/type filters, bounded SQLite/FTS indexing with persisted cursors | Generic CAS/range Index coordinator and live updates are not yet the desktop path; a row cap is not coverage evidence |
| Native transfers | Durable account-scoped tasks/batches, bitmap missing-part resume, pause/resume/cancel/retry, terminal deletion, adaptive P, bounded retries/shared FloodWait, live/replay session logs | Exact byte-count verification only; no cryptographic content hash, physical multi-connection/DC pool or bandwidth control |
| Encrypted transfers | Real file→encryption→Telegram upload with verified parts and final authenticated manifest; managed discovery; authenticated restore and atomic non-overwriting publication | 60 MiB plaintext/8 MiB frames/64 MiB encoded object bounds; memory-only tasks, no durable pause/cancel/retry/restart resume, orphan cleanup or empty-file desktop upload |
| Local Library | Persistent metadata import/search/filter/paging/details, open/reveal original files under Utilities; collections remain Core/Storage capabilities | Paged Library is not virtualized; million-record performance unmeasured; source files may move/disappear |
| i18n | Synchronized en-US/zh-CN/ja-JP, live locale choice, fallback/negotiation/formatting, semantic errors, About/CHANGELOG parity | Native NSLocale discovery and native-speaker review outstanding |
| Diagnostics | Bounded lossy process JSONL plus durable typed transfer session logs, Settings access, safe performance/failure fields | Index event coverage and long-term session-log retention need further work |
| Crypto/formats | Explicit codecs, candidate vectors, bounds/tamper/wrong-key tests, AEAD registry APIs, fake-remote fresh-database recovery | Provisional: independent review, longer fuzz evidence, full identity hydration and protected real-system recovery are release gates |

## v0.4.0 compatibility and scope

[ADR 0012](adr/0012-gpui-kit-and-private-storage-channel.md) governs dependency provenance, private storage and account ownership. Schema 9 adds nullable positive account ownership to native tasks. The first configured Telegram connection atomically attributes legacy unknown rows only to its actual restored authorized session, or permanently leaves them unknown if unauthorized. Later logins cannot claim that work. New tasks require known scope; enqueue/resume/retry and actual network execution validate it. Switching pauses/drains native workers and waits for Vault work. Completed user files/history remain intact.

Versioned settings record account-scoped channel binding and the one-time migration decision. Crypto/manifest/recovery/bitmap/session-log encodings are unchanged. Missing/invalid/truncated channel discovery fails closed, new uploads validate current privacy/ownership before touching plaintext/keys, and manifest cancellation is propagated independently from key-operation state.

The permissive GPUI graph and exact license exceptions are reviewed in ADR 0012, `deny.toml` and third-party notices. No GPL `tdl` or incompatible implementation was used. Core/Runtime remain GUI-free; SQL and grammers remain adapter-owned. Three catalogs, structured domain errors and bounded retained owners remain in place.

## Verification record

Passed: the [development quality checks](DEVELOPMENT.md#quality-gates), including formatting, locked workspace/all-target check, strict Clippy, all workspace tests, warning-denied rustdoc, cargo-deny and diff review. Static GUI message IDs and all documentation links/anchors also resolve. The suite contains 296 tests: Core 25, Crypto 36, GUI 45, i18n 21, Index 12, Runtime 56, Storage 23, Telegram 24 and Transfer 54. Account isolation, one-time legacy attribution/no-rebinding, private-channel identity, cancellation, modal Escape priority and About parity extend existing transition, migration, compatibility-vector and recovery coverage. No required source gate was skipped; the remaining live-system, security and product-review limitations are explicit below.

Actual isolated windows reviewed: English Transfers with 200 channels, authenticated Files, upload, returning-account and fresh login; Chinese 900×600 unlock layout, localized wrapping, Tab cycling within the dialog, Escape/cancel dismissal, Return submission/empty-password feedback and the compact raw file table; Japanese dark About with version/full changelog, Files/Raw switching, 5,000-row raw scrolling and direct return to Transfers. The sidebar refresh action retained the current Transfers view. Visual defects found during review were corrected, including table-header/input contrast and synthetic preview persistence banners. Preview runtime constructors are disabled and fixtures are synthetic; no real sessions, credentials, private files or remote writes were used.

The full locale/size/backing-scale, VoiceOver and real-account matrices remain release qualification. The reviewed layouts and keyboard actions do not imply those broader checks are complete.

Known toolchain debt: upstream `block 0.1.6` emits a future-Rust incompatibility warning; current workspace Clippy is warning-clean. Passing cargo-deny reflects reviewed policy/explicit exceptions, not a blanket claim that every dependency has no maintenance risk.

## Documentation consolidation

The contributor guide now routes to 12 focused top-level documents, down from 15. The guide plus those documents shrank from 3,261 to about 1,200 lines (approximately 63%). Diagnostics moved into Transfer/Security, the audit package into Security, and UI review into this status plus the Development matrix. Redundant `DIAGNOSTICS.md`, `SECURITY_AUDIT_PACKAGE.md` and `UI_REVIEW.md` were removed. Byte-level Crypto/Manifest contracts, both license files and all accepted ADRs were preserved.

Git checkpoints preserve the complete source before the rewrite (`checkpoint/pre-gpui-kit-redesign-20260907`), before document removal (`checkpoint/pre-doc-consolidation-20260907`) and after consolidation (`checkpoint/post-doc-consolidation-20260907`). [Recovery commands](DEVELOPMENT.md#documentation-recovery-checkpoints) support inspecting/restoring documents without guessing prior versions. Source rollback never downgrades user data automatically.

## Next actions and release gates

1. Run protected real-account creation/discovery/rename/privacy-loss, switch-account, upload/manifest scan/download and legacy recovery tests; include timeout, cancellation, reconnect and crash boundaries.
2. Stream larger encrypted parts, persist Vault checkpoints/controls, hydrate all prior AEAD identities, reconcile orphan ciphertext and test rename/database-finalization crash windows.
3. Connect the generic Index coverage coordinator and live updates; benchmark Library and virtualized views from 1,000 to 3,000,000 records before claiming scale.
4. Add an owned physical media-DC connection pool, then measure throughput and server-limit behavior on real networks; synthetic controller tests are not bandwidth or ban-rate claims.
5. Complete minimum/default/display-fitted-large, three-locale, light/dark, backing-scale, native fullscreen, keyboard/VoiceOver, reduced-motion and native-speaker review. Qualify active-window/sleep/logout behavior without relying on synthetic secrets.
6. Review OS credentials, local permissions/SQLite backups, NSLocale discovery, packaging/signing/notarization and deployment/CPU support.
7. Retain longer fuzz campaigns, obtain an independent signed review of the exact candidate and satisfy every [format-stability gate](SECURITY.md#independent-audit-and-format-stability-gate) before removing provisional markers.

Update capability rows and limitations when code changes. Tests against fakes never stand in for real Telegram, independent security review or successful recovery on a user's data.
