# Index and search

The desktop uses an account-owned incremental Channel synchronizer and a local SQLite projection. Selecting a Channel only promotes already-needed work; it never starts a network scan. Storage/legacy raw browsing and the generic `IndexCoordinator` remain separate paths. The generic coordinator is tested through project-owned ports; its concrete desktop mapping and range compaction remain unfinished. [ADR 0018](adr/0018-local-first-channel-sync.md) records this decision.

## Boundaries and identity

```text
Telegram push hints -> Runtime ChannelSync -> channel difference/history/verification
                                               -> Storage transaction -> local GUI query
Generic history port -> Index coordinator -> transactional records/coverage evidence
```

`grammers` stays in Telegram; SQL stays in Storage. Every source, query, cursor, index job and remote upsert carries account/chat identity. Runtime checks the expected authorized account at network execution. Source records preserve message sent time separately from modification time, and revisions upsert idempotently on account/chat/message identity.

Channel synchronization and generic indexing have different authority. `channel_sync_state.pts` records the committed Telegram update sequence; it is never inferred from message-ID continuity. A separate exclusive `history_before` cursor records bounded older-history reads. Neither creates generic policy/range evidence in `telegram_index_state` or `index_ranges`. The Local Library includes these cached file records with original source provenance, not transfer history. Only a successful atomic metadata/deletion/cursor commit advances synchronization. Cancellation discards uncommitted response data; account/source generations reject late GUI callbacks.

## Connected Channel synchronization

- A restored account compares dialog PTS with local state. A previously unseen channel seeds one recent 200-message page, then requests a difference from the PTS captured before seeding. Existing cached rows stay available throughout; legacy cached IDs are verified in batches of 100.
- New/edit/delete push hints coalesce by channel and highest PTS. Explicit server gaps, reconnect or hint overflow require reconciliation. Known-clean channels make no history/difference request on selection. A 15-minute dialog metadata reconciliation detects missed passive subscriptions; an unknown-source push requests debounced metadata discovery. Only a detected mismatch queues channel data synchronization.
- Normal updates use `updates.getChannelDifference` in bounded pages, including edits within the same timestamp second and explicit deletions. `TooLong` retains the old cache, records a separate restartable history-gap cursor back to the prior local boundary, and verifies known IDs in batches of 100. Missing numeric message IDs alone are not gaps.
- Older history loads only on explicit request, one 200-message page at a time. Gap repair does not advance or consume the user's older-history cursor/request. There is no automatic history loading caused by table scrolling. Storage/legacy raw scans retain their existing behavior.
- One retained account owner schedules at most 10,000 sources through a 64-command queue. The selected channel has priority with bounded fairness. Network/persistence failures retry after 1/2/4 seconds, then pause; account-wide FloodWait deadlines survive manual retry. Account changes cancel the active request and release the old owner off the UI thread.
- GUI reads SQLite off-thread, retains an eight-view/16 MiB warm cache and virtualizes up to 5,000 displayed rows. Sync commits trigger local reloads without blanking visible rows. The phase/queue/status remains visible across navigation, with duration, last activity, retry/cancel and an independently scrolling 128-event timeline. Overflow/truncation is disclosed; unmeasured percentage/rate/ETA is not fabricated.
- The Channel Batch action reads matching indexed files in 256-record keyset pages, including rows outside the visible projection. More than 5,000 matches is rejected before queueing; accepted batches each use a separate folder. It does not scan unindexed remote history. Legacy remote batch APIs retain their own bounds. Search in the current projection does not claim coverage of unvisited history. See [ADR 0054](adr/0054-filter-driven-channel-downloads.md).
- Indexing policy and batch sizes are automatic implementation details. Settings has no indexing controls; the legacy persisted batch-size field is retained for codec compatibility and ignored by the desktop.

This implements document projection for broadcast channels/supergroups, not a complete messaging client or common/private-chat update engine. Full-history coverage, generic range compaction and million-record performance still require separate evidence.

## Coordinator state and coverage

| State | Allowed next state |
| --- | --- |
| Queued | Running, Paused, Cancelled |
| Running | Paused, Completed, Failed, Cancelled |
| Paused | Running, Cancelled |
| Failed | Queued (explicit retry), Cancelled |
| Completed / Cancelled | Terminal |

Pause/cancel commit only a safe batch and release owned work. Restart requeues interrupted running jobs with their committed checkpoint. FloodWait/network reasons remain structured errors/scheduler input, not translated state names.

Each job/range has a content-policy version and fingerprint. A file-only or minimum-size scan does not prove coverage for excluded content; broadened policies require new work. Plain text is opt-in in the generic policy. A range represents actual account/chat ordering bounds, partial/complete coverage, policy, generation and checkpoint evidence, not one global last-message ID.

Current storage uses inclusive message-ID bounds and rejects overlapping ranges within the same policy scope. Date/order-rich bounds and compatible-range compaction are targets. Only a source-proven boundary plus committed batches may establish complete coverage; fetched-but-uncommitted pages are safely fetched again. The Channel path has the incremental semantics above; generic policy ranges still require independent edit/delete/gap evidence before claiming completeness.

## Search contract

FTS5 searches normalized copies of names/paths/captions while original Unicode remains available. Parameterized structured facets filter account/channel, kind, dates, size, extension and file states. The current sort is `(modified_at DESC, LogicalFileId DESC)`; query-bound opaque keyset cursors reject stale filters. Stable fixtures traverse ties without missing or duplicating IDs. Other sort modes and snapshot isolation during concurrent insertion are not implemented.

CJK, emoji, combining characters and case behavior need measured tokenizer/product expectations; locale changes must not rewrite indexed source content. Avoid deep OFFSET and per-query remote full scans.

## Ownership and verification

Use bounded fetch/write batches and a limited number of owned jobs. Never spawn one task per message, accumulate a whole source, or block the GUI on storage/network work. Retry uses bounded policy and account-scoped server deadlines. Authorization/access changes need user action; corruption/invariant errors fail safely.

Deterministic tests cover duplicate/out-of-order revisions, policy filtering, atomic/CAS batches, oversized pages, a 10,000-record pause/resume scan, range evidence, replay, state transitions, scoped FloodWait, FTS facets/injection and tied cursors. Benchmark generated 1k/100k/1m/3m-file catalogs before claiming scale; retain measurements as artifacts, not committed user-like databases. See [Data model](DATA_MODEL.md), [Transfer](TRANSFER_ENGINE.md) and [status](IMPLEMENTATION_STATUS.md).
