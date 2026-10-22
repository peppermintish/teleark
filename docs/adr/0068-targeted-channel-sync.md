# ADR 0068: Targeted channel synchronization and server-timed subscriptions

Status: Accepted, 2026-10-22. Supersedes the independent seven-minute channel silence recovery in ADR 0064, ADR 0067's durable-observation fallback for channels, and ADR 0018/0019's selection-only priority policy. The automatic ownership, durability, cancellation, account isolation and admission contracts remain.

## Problem

Quiet channels were independently forced through difference recovery every seven minutes. Shared admission can space starts by up to 30 seconds, so a large round can outlast the fallback interval and appear continuous. Request-admission waits occupied the same snapshot list as executing work. The Telegram adapter discarded the returned difference timeout, preventing protocol-timed subscriptions.

## Decision

Runtime applies contiguous channel pushes directly. Missing channel sequences, explicit channel overflow/too-long hints, advanced remote heads, unfinished durable repair and initial bootstrap retain targeted recovery from committed per-channel PTS. Account-wide lost-update or broken-transport hints request directory reconciliation first; they do not force every channel to fetch a difference. Initial/reconnected discovery and the seven-minute account-delivery silence directory fallback remain. Directory PTS determines which channels lag; an unavailable head requires a targeted initial check. There is no recurring age-based difference sweep over ordinary channels. Account PTS cannot substitute for independent channel PTS.

The visible ordinary channel and managed storage channel have short-poll subscriptions. After a final difference, Runtime schedules the next request using Telegram's timeout, with a one-second default/minimum. Non-final differences continue from their committed cursor under existing admission. Contiguous pushes do not postpone subscription renewal. Closing or replacing an ordinary channel view clears its subscription demand and deadline; actual gap/repair/history demand remains independent. The latest viewed channel is a coalesced atomic identity, so rapid navigation cannot fill the bounded command queue or lose a close event. A worker rechecks that identity after admission and before starting a subscription-only RPC. A response already in flight may still commit valid data, but a stale completion cannot restore a closed subscription. Managed watch replacement retires the previous subscription while preserving its other recovery work.

Server FloodWait and persisted adaptive admission remain authoritative and may delay a subscription beyond its due time. Idle subscriptions are not executing work and create no recurring GUI/database poll. Runtime retains deadlines, workers and account/connection generation checks; UI timers remain presentation-only.

Snapshots separate executing `active` entries from a bounded `waiting` list (at most six retained owners). Admission/server waits leave the active list, remain visible in the inspector and timeline, and count as waiting in the shell. Channel rows reserve one shared icon slot: a sync icon for executing work, a pause icon for admission/server waits and no icon after completion. Full localized row labels/tooltips retain the channel name and phase. Rows retain the shared 24-point height and 12-point text, and only visible rows materialize.

## Compatibility

Release 0.5.19 retains SQLite read/write schema 26 and supported automatic upgrades 0–25 → 26, including skipped releases. Persistent observation timestamps retain their historical meaning; they no longer schedule ordinary channel sweeps. Directory cache and cooldown codecs remain v1. Payload/container/manifest, key/recovery and transfer codecs are unchanged. Subscription demand, deadlines and waiting snapshots are transient; restart discovers remote heads and reconstructs subscriptions without resetting local data. No new dependencies or licensing changes. Release auditing found the existing `inout` 0.2.3 yanked; the lockfile advances only that transitive dependency to compatible 0.2.5 (MIT OR Apache-2.0, Rust 1.85), within the application’s Rust 1.88 floor. Crypto compatibility tests and the full release checks cover the updated graph.

## Verification

Deterministic tests cover direct/duplicate/gapped pushes, changed-head-only startup/reconnect recovery, quiet background channels, returned/default timeouts, pushes preserving deadlines, view registration before discovery, closed queued polls retaining real gap demand, blocked requests, admission cancellation, server cooldowns, stale connection results and separate active/waiting projections. GUI tests cover active/waiting/completed channel indicators, stable row geometry and shell counts at 900×600 and native full-screen flags. Native English/light preview evidence and complete release-check results are recorded in the release validation report.

Protocol references: [independent update sequences](https://core.telegram.org/api/updates#message-related-event-sequences), [channel subscriptions](https://core.telegram.org/api/updates#subscribing-to-updates-of-channels-supergroups), and [channel too-long recovery](https://core.telegram.org/constructor/updateChannelTooLong). No external client implementation was inspected.
