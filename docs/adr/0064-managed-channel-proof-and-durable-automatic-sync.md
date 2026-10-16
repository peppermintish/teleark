# ADR 0064: Proof-selected channel key and durable automatic synchronization

Status: Accepted — 2026-10-02. Supersedes ADR 0057's requirement that every discovered manifest authenticate with one key. Retains one current upload key, newest-proof authentication, account isolation and all existing encrypted readers.

## Problem

Native download segments are 1 MiB, while the encrypted stream reader accepts blocks no larger than 512 KiB. Passing native segments directly to that reader produces an integrity failure for otherwise valid uploaded files. Channel selection also treats an individual invalid manifest as a veto on the entire channel key, hiding unrelated authenticated files. Global silence recovery can be postponed by traffic in other channels, and an unchanged difference leaves no durable freshness observation.

## Decision

The Telegram stream adapter emits ordered blocks bounded by 512 KiB, including a short tail. The encrypted reader retains its bound and authenticates each frame before writing private plaintext. Final publication still requires complete container and whole-file verification. No normal ciphertext staging is introduced; container and codec geometry does not change. Synthetic connected remotes use the same segment-to-block adapter, so large-file lifecycle tests cross the production boundary.

The newest authenticated channel-key proof selects the current upload key. A damaged newest proof never falls back to an older proof. A legacy channel without a proof uses its newest matching authenticated manifest with validated remote name and account/chat locators. If no remote manifest authenticates, bounded retained inventory can supply the same authenticated evidence for that exact account/channel, including a legacy channel whose only remote file is now damaged or missing. Each manifest independently keeps its health/key requirement. Durable authenticated inventory explains invalid or missing remote entries without deleting other entries or final local files. Remote deletion between cached catalog and fetch is recorded for that file and does not abort the remaining catalog. Revalidation preserves the current account/channel projection while awaiting a result.

Key validation and manifest synchronization retry typed transient network/server/conflict failures automatically, honoring server deadlines. Backoff runs on the retained background task after releasing the key/scan owner. Cancellation, backend/key mutation and account replacement fence retries; permanent access/storage/integrity errors retain actionable diagnostics. No error-prose matching or UI service timer is introduced.

Each channel has an independent seven-minute quiet fallback deadline in the retained account owner. Pushed sequence updates continue reconciling from persisted PTS; unrelated channel traffic cannot postpone this deadline. SQLite schema 24 adds an optional successful-observation epoch timestamp. Successful empty differences update it with scoped revision fencing without advancing the projection revision. Failed/cancelled/stale reads never advance it. Restart restores the remaining interval; absent, expired or future timestamps require an automatic fresh check. Ordered migrations from schemas 0–23 preserve all older state and roll back schema/marker changes on verification failure.

The sync inspector displays automatic recovery, phase duration, last activity, deadline, typed reason and bounded history, with no Retry Sync or sync cancellation controls. Genuine credential/access choices retain their existing controls. File transfers retain their independent pause/cancel/retry actions. The upload dialog removes its informational Upload options drawer and unused expansion state; file selection and automatic encryption remain visible.

## Verification

Deterministic tests cover native segment/tail geometry, an 11 MiB + 17 B encrypted round trip across sync and restart, file-local corruption/deletion, retained final outputs, key-owner availability during retry, lock fencing, independent quiet deadlines, clock rollback, scoped stale observations, the actual primary-key observation query plan, supported/skipped upgrades and failed-migration restart with preserved cursor/key bytes. English/light synthetic previews and GUI tests cover 900×600 and full-screen layouts. Release delivery runs the repository and native platform gates; synthetic coverage does not certify live Telegram recovery.

Protocol reference: [Telegram update reconciliation](https://core.telegram.org/api/updates). No incompatible external implementation or new dependency is used.
