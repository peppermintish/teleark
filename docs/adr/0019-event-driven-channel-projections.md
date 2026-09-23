# ADR 0019: Event-driven channel projections and private-channel observation

Status: Accepted, 2026-09-08. Supersedes the selection-only priority, polling and private-projection exclusions in [ADR 0018](0018-local-first-channel-sync.md).

## Context

The account synchronization owner removed navigation-owned history scans, but the GUI still woke every 250 ms and used a global commit revision to reload the selected channel. Unrelated channels and empty differences could therefore cause unnecessary SQLite reads and row formatting. Private Files independently searched and decrypted up to 1,000 manifests on each visit; private Raw Files had a separate interactive history path.

## Decision

Telegram pushes carry bounded plain metadata and channel PTS/counts. Runtime applies contiguous pushes directly, skips duplicates and requests an official channel difference when ordering is missing or transport/queue coverage is uncertain. Transport callbacks release their queue lock before notifying consumers. The account owner parks until a command, push, cancellation, actual retry, server subscription deadline or metadata fallback. No fixed UI/scheduler polling timer remains in this path.

Telegram requires `getChannelDifference` to renew a viewed channel's subscription after the returned timeout (one second when absent), immediately continuing non-final differences. Runtime maintains this interest for the visible Channel and the managed private channel, at most two interests. Leaving the visible channel removes that interest; private protection stays active. Other channels receive passive pushes plus the existing 15-minute dialog-metadata reconciliation. These protocol/failure timers are distinct from UI polling. Server FloodWait remains account-wide and cannot be bypassed by selection or retry. Priority work yields after three turns.

A commit returns actual upserts/deletions, excluding unchanged rows. Empty final differences with unchanged PTS/cursors do not write SQLite. History/search only enrich unknown IDs; PTS-ordered differences and exact-ID verification own edits, including edits within one timestamp second. Storage returns a coherent rows/revision/history snapshot. GUI subscribes before its initial read, then applies only that channel's deltas after the baseline. A retention miss requires another coherent local baseline. Bursts coalesce by message ID; unchanged formatted rows, selection and the table owner survive. Unrelated channels and phase-only/empty updates do not reload the file list.

The managed private channel uses the same scheduler, PTS, recent-history and difference projection as ordinary channels. A one-time bounded exact-caption discovery adds up to 1,000 historical manifest candidates; recent seeding and subsequent pushes/differences cover newly published manifests without waiting for search indexing. Raw Files reads this shared projection and exposes explicit older-history controls.

Private Files derives authenticated metadata from local candidates. A per-message cache version invalidates edited manifests even when visible metadata is unchanged. Gap recovery forces re-authentication of verified candidates without fabricating per-file edit records. The bounded derived cache retains no File Keys or decrypted manifest payloads; lock clears it and source/account changes invalidate its scope. Known-file downloads re-authenticate their manifest using its known locator; upload receipts remain usable before the shared catalog catches up. Normal publication does not create an edit/delete alert.

A private edit/delete push immediately publishes a pending-review warning before queued network/SQLite work completes. Confirmed edits, deletions and uncertain server gaps commit to the durable observation journal with PTS. Acknowledgement covers only the displayed sequence; a concurrent/newer observation stays unread. The warning is visible while locked. A cached binding establishes an observation scope only: complete remote identity/uniqueness checks and fresh upload preflight remain unchanged. Offline delivery is recovered after reconnect; no unconditional instant-delivery guarantee is made.

The status bar belongs to the whole window, including Account and Settings, and opens details without navigating. Message synchronization and manifest queue/read/transport/authentication/completion share the account event feed. Manifest counts distinguish unchanged and rejected candidates; elapsed time updates once a second only while work is active. A newer manifest job cannot be replaced by an older callback; older terminal events remain in the bounded timeline. Failed queue submission and abandoned work have terminal feedback. No unmeasured rate, ETA or percentage is introduced.

## Bounds and persistence

Schema 12 adds `channel_file_versions`, `managed_channel_watches`, `managed_channel_changes` and a candidate lookup index. Supported SQLite sources 0–11 upgrade automatically, including skipped versions, to read/write 12. The migration is additive and transactional; structure/foreign-key checks precede the version commit. Originals, wrapped keys, transfer state and older cursor meanings are retained. Newer databases are rejected intact. Crypto, manifest, recovery, preferences and transfer codecs are unchanged.

The existing 10,000 sources/64 commands/128 event bounds remain. Each push queue is limited to 512 updates and 4 MiB; overflow is visible and forces difference recovery. The delta journal is limited to 128 material pages and 4 MiB, with per-channel retention markers. Unchanged pages do not evict useful deltas. The private journal retains 128 recent observations plus cumulative/unread counts. Derived manifest metadata uses a 16 MiB cache and at most 1,000 candidates per scan; upload locators/GUI receipts are limited to 128. The GUI retains at most eight warm views/16 MiB and displays at most 5,000 raw rows.

## Evidence and limits

Deterministic tests cover direct-push RPC avoidance, duplicate/gap/overflow behavior, blank-difference write avoidance, subscription deadlines and navigation, private priority/fairness, failed registration before network work, immediate warnings during blocked reads, stale/closed owners, coherent baselines and per-channel retention. Storage tests cover same-second invalidation, stale history, normal publications, deletion/acknowledgement races, restart, bounded journals and transaction rollback. Cache fixtures cover unchanged/changed/deleted candidates, transient failures, rejected content, account/lock scope and limits. GUI tests cover delta updates and a bottom-aligned locked warning on seven pages at 900×600 in all three locales and both themes.

Protected real-account delivery, cross-device latency, long outages, real server FloodWait/TooLong and native assistive-technology qualification remain separate work. Bounded discovery is not complete historical recovery or an authenticity proof for the remote channel itself. For very old server gaps, Telegram limits message-history access; the current history backfill plus exact verification of known IDs can omit previously unindexed old files. The persisted gap warning records that uncertainty. Exhaustive exact-ID range reconstruction remains future work.

## References

- [Telegram channel subscriptions and updates](https://core.telegram.org/api/updates#subscribing-to-updates-of-channels-supergroups), including PTS sequencing, subscription deadlines and difference recovery.
- Public grammers 0.10.0 APIs and the repository's existing MIT OR Apache-2.0 dependency graph. No dependency was added and no license-incompatible implementation or derived material was inspected.

## Automatic account supervision (2026-09-12)

[ADR 0031](0031-automatic-account-synchronization.md) supersedes this document's
GUI-owned discovery, single synchronous execution lane and explicit history
controls. Runtime now owns initial discovery, generation-scoped connection
rebinding, independent bounded calls and committed source metadata publication.
Scrolling creates a cancellable history demand; ordinary refresh controls and
the duplicate sync footer are removed. The global inspector remains visible
across navigation, including while locked. Capped jittered retries, authoritative
FloodWait, PTS-based recovery and authenticated private-file projection retain
their existing meaning. The new directory restart cache is independently
versioned; SQLite and encrypted formats are unchanged.
