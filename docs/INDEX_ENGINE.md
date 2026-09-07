# Index and search

The desktop performs authorized source discovery, bounded document browsing/indexing, idempotent remote projection and SQLite restart cursors. The generic `IndexCoordinator` is separately tested through project-owned ports; its full concrete mapping, incremental updates, range compaction and retry-timing owner remain unfinished.

## Boundaries and identity

```text
Telegram history port -> Index coordinator -> transactional records/evidence
                                           -> SQLite/FTS5 -> Core query -> GUI
```

`grammers` stays in Telegram; SQL stays in Storage. Every source, query, cursor, index job and remote upsert carries account/chat identity. Runtime checks the expected authorized account at network execution. Source records preserve message sent time separately from modification time, and revisions upsert idempotently on account/chat/message identity.

Interactive browsing and indexing have different authority. Browsing can show previously cached rows and upsert newly observed files, but does not advance `telegram_index_state` or prove coverage. The Local Library includes these cached/indexed file records and explains that they are not transfer history. Query results retain original channel titles and unambiguous account/chat/message provenance. Only a committed indexing page advances its checkpoint. Cancellation discards uncommitted response data; GUI generations reject late results after account/source changes.

## Connected desktop bounds

- Raw browsing uses bounded exclusive-cursor pages with 200-message transport chunks, cancellation and slow/error feedback. DataTable virtualizes a bounded in-memory projection; it does not instantiate a whole channel.
- Explicit indexing advances a persisted per-source cursor in bounded pages; the configured index batch sizes are 200/500/1000.
- Native batch-download filtering is a separate scan: at most 50,000 examined messages and 2,000 retained matches, with sent-time/file-kind filters.
- Counts distinguish examined messages from eligible files. Search in the current raw/managed projection does not claim to search unvisited Telegram history.

Large-scale full-history coverage and incremental edit/delete synchronization are not implied by a populated table.

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

Current storage uses inclusive message-ID bounds and rejects overlapping ranges within the same policy scope. Date/order-rich bounds and compatible-range compaction are targets. Only a source-proven boundary plus committed batches may establish complete coverage; fetched-but-uncommitted pages are safely fetched again. Edits, deletes, gaps and out-of-order updates require explicit semantics before claiming incremental completeness.

## Search contract

FTS5 searches normalized copies of names/paths/captions while original Unicode remains available. Parameterized structured facets filter account/channel, kind, dates, size, extension and file states. The current sort is `(modified_at DESC, LogicalFileId DESC)`; query-bound opaque keyset cursors reject stale filters. Stable fixtures traverse ties without missing or duplicating IDs. Other sort modes and snapshot isolation during concurrent insertion are not implemented.

CJK, emoji, combining characters and case behavior need measured tokenizer/product expectations; locale changes must not rewrite indexed source content. Avoid deep OFFSET and per-query remote full scans.

## Ownership and verification

Use bounded fetch/write batches and a limited number of owned jobs. Never spawn one task per message, accumulate a whole source, or block the GUI on storage/network work. Retry uses bounded policy and account-scoped server deadlines. Authorization/access changes need user action; corruption/invariant errors fail safely.

Deterministic tests cover duplicate/out-of-order revisions, policy filtering, atomic/CAS batches, oversized pages, a 10,000-record pause/resume scan, range evidence, replay, state transitions, scoped FloodWait, FTS facets/injection and tied cursors. Benchmark generated 1k/100k/1m/3m-file catalogs before claiming scale; retain measurements as artifacts, not committed user-like databases. See [Data model](DATA_MODEL.md), [Transfer](TRANSFER_ENGINE.md) and [status](IMPLEMENTATION_STATUS.md).
