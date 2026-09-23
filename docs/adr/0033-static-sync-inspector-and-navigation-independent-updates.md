# ADR 0033: Fixed synchronization timestamps and navigation-independent updates

Status: Accepted, 2026-09-13. Supersedes ADR 0032's presentation clock, ADR 0031's counter-based inspector and unconditional directory reconciliation, and ADR 0019's viewport/private short polling. The ownership, bounded history, authenticated projection and migration guarantees remain in force.

## Decision

The synchronization inspector shows a current state, the last successful completion time and a chronological activity list with fixed local event timestamps. It has no phase duration, age, countdown, attempt count, active/queued count, scan counter, spinner or presentation timer. Removing the dedicated timing entity and workspace-preparation clock eliminates their recurring notifications. Business events still publish meaningful waiting, running, failure/cancellation and completion states. Successful completion is retained separately from the bounded timeline; a failed, cancelled or stale operation cannot replace it. It is session/account-scoped and is not a persisted cross-launch timestamp.

The narrow inspector uses a neutral summary surface, restrained status color, distinct typography and separated event/source/time lines. The bounded history remains virtualized and independently scrollable; long content has an accessible tooltip, and omitted history is disclosed only when history was actually omitted. These are original GPUI components using the existing theme. Design reference: [Tabler simple timeline guidance](https://docs.tabler.io/ui/components/timeline#simple-timeline), from the [MIT-licensed Tabler project](https://github.com/tabler/tabler/blob/dev/LICENSE). No external component code, imagery or dependency is imported.

## Navigation is presentation

Channel and TeleArk navigation sends no observe/prioritize/refresh command to synchronization. It neither cancels nor starts managed verification. Account-owned managed projection updates run on relevant committed catalog changes, initial storage availability and key-unlock events, regardless of the selected ordinary page. The existing generation/account/channel fences, retained task and explicit cancellation remain. Switching between ordinary pages retains the same managed rows and active work.

Channel views retain bounded local rows together with their revision and exhaustion state, including a known-empty result. Reopening applies retained deltas; only a cold/evicted or invalidated baseline needs a local read. Mounting a table never demands remote history: downward scrolling or keyboard navigation arms near-end history once, and the explicit earlier-history action supports short lists and accessibility. Leaving cancels a viewport-only history request, while live account updates continue.

## Why any network deadline remains

Joined-channel data arrives through the existing transport update feed. Contiguous PTS pushes commit directly; sequence gaps, connection replacement, metadata changes, overflow and explicit retry/history requests have specific recovery work. A completed difference does not schedule another request after one second or after its returned short-poll timeout. The viewport observation API and managed/visible-channel short-poll scheduler are removed.

Telegram documents recovery after a long interval without updates in [Working with Updates](https://core.telegram.org/api/updates). The account coordinator retains one 7-minute **delivery-silence** check to detect missed channel heads/membership through the directory; actual channel delivery postpones that deadline. This replaces the unconditional healthy 15-minute directory refresh. The seven-minute policy is displayed as fixed explanatory text in the inspector and reads the same Runtime constant; there is no countdown. It is independent of the selected tab. A normal unchanged response does not write the directory, change source revisions, replace completion time or publish inspector activity. A slow check becomes visible once after one second; changed results, errors and retry/rate-limit decisions remain visible. Source comparison excludes message PTS because head changes do not rename or reorder the sidebar; PTS still reaches the channel scheduler for recovery.

One-second metadata coalescing occurs only after a metadata event. Failure retries and Telegram FloodWait have operation-specific deadlines; a permanent failure with no remaining demand parks until an event. These are service recovery deadlines, not screen refresh loops. The visible-chat short-poll recommendation is intentionally not used for TeleArk's joined-channel cached-file browser; this is not a promise of immediate delivery when Telegram temporarily suppresses pushes. The silence recovery bounds that case without navigation causing network work. No real-account delivery latency or CPU improvement percentage is claimed.

## Compatibility and verification

No persistent schema, encrypted codec, recovery format or settings version changes. Completion timestamps are transient snapshot metadata. Supported automatic upgrade paths are unchanged. Existing unrelated pending work is preserved.

Deterministic checks cover zero idle app notifications with an open waiting inspector, fixed timestamps and virtualized history, completion preservation across history eviction and failures, stale manifest completions, navigation retaining scan cancellation handles and rows, revisioned empty caches, differences creating no polling or unchanged writes, delivery postponing silence recovery and PTS-only changes causing no sidebar revision. Native review uses synthetic data, English, 900×600 and actual full-screen mode in both themes. Final executed results are recorded in Implementation Status.

[ADR 0037](0037-application-pin-and-transfer-drain.md) supersedes the status-bar entry point: status is display-only and Network settings opens the merged activity timeline.
