# Index Engine

Status: target design. No Telegram history scanner, SQLite index, FTS5 query path, or resumable index job exists in the foundation/mock-UI milestone; any displayed channel counts/coverage are synthetic.

## Purpose

The Index Engine converts potentially millions of Telegram messages into a durable local file-centric index. Interactive search reads SQLite/FTS5 rather than scanning remote history for every query.

```text
Telegram history/update port
 -> Index Engine
 -> normalized records and coverage evidence
 -> SQLite/FTS5
 -> Core search/query API
 -> keyset-paginated virtual GUI
```

`grammers` types stay in the Telegram adapter. The engine operates on TeleArk-owned history records and emits structured events/data, not localized prose or GPUI types.

## Index request and content policy

Users can request last 30 days, six months, one year, all history, or a custom date range. A content policy selects files/videos/images/audio and optionally plain text (off by default), plus filters such as minimum media size.

Every job and coverage range stores a stable policy fingerprint/version. A range scanned with “files only, >= 1 MiB” does not prove coverage for plain text or smaller files. Only semantically compatible policies may merge; broadening policy creates work for previously excluded history.

## Job state machine

The Core foundation already centralizes this minimal state machine:

```text
Queued -> Running -> Completed
   |         |  \-> Failed -> Queued (explicit retry)
   |         \----> Paused -> Running
   \--------------> Paused

Queued / Running / Paused / Failed -> Cancelled (where allowed below)
```

Exact allowed transitions are `Queued -> Running|Paused|Cancelled`, `Running -> Paused|Completed|Failed|Cancelled`, `Paused -> Running|Cancelled`, and `Failed -> Queued|Cancelled`; `Completed` and `Cancelled` are terminal. Pause/cancel cooperatively finish a safe batch, persist checkpoint/range evidence, and release workers. Structured retry/FloodWait/network reasons may later justify additional explicit states, but Core, tests, and this document must change together. Process restart turns an interrupted running attempt into resumable queued/recovery state rather than silently claiming completion.

## Ordering, cursors, and source identity

Telegram message IDs are scoped to a chat/account and may not encode all temporal semantics. History records use a deterministic source ordering key containing account, chat, message ID, and timestamp/date as required by the adapter. Bounds and resume cursors are explicit and versioned; never use one global `last_message_id`.

The engine must handle empty/non-media messages, edits, deletes, duplicate/out-of-order updates, inaccessible history, and gaps. Upserts are idempotent on `(account_id, chat_id, message_id)` plus version/edit information. File identity and remote-object identity are mapped deliberately; a repeated update cannot create uncontrolled duplicate logical files.

## Index ranges and coverage

`IndexRange` represents evidence of actual coverage for one account/chat and policy:

```text
lower source/date bound
upper source/date bound
boundary inclusivity/order version
coverage state: Partial or Complete
policy fingerprint
durable checkpoint/evidence
scan generation and timestamps
```

Ranges may be non-contiguous:

```text
2018 complete | 2019 complete | 2020 gap | 2021 partial | 2022 complete
```

Only adjacent/overlapping complete ranges with compatible ordering and policy merge. A partial range never expands a complete claim. A crash persists the last committed batch and a precise remaining cursor; fetched-but-uncommitted messages are fetched again and idempotently upserted. Coverage is marked complete only after the source adapter proves the requested boundary was reached and all batches committed.

The GUI coverage map is derived from these stored ranges, not mock percentages or the most recent message ID.

## Initial and incremental indexing

### Historical scan

1. Validate account/chat access and requested scope/policy.
2. Create/recover a job and determine uncovered subranges.
3. Fetch one bounded page through the history port.
4. Normalize eligible messages/media into locale-neutral domain records.
5. Transactionally upsert messages/files/remote objects/FTS rows and checkpoint/range evidence in a batch.
6. Emit throttled progress and continue until the requested boundary is proven.
7. Compact compatible complete ranges and mark the job completed.

### Incremental synchronization

Process live/polled updates through the same normalization/upsert path. Track a durable update cursor separately from historical coverage. Edits update filename/caption/search content; deletes/tombstones update remote availability according to retention policy. If an update gap is detected, schedule a bounded reconciliation scan rather than assuming continuity.

Historical and incremental work may overlap. Idempotent keys and transaction ordering prevent duplicate logical files and stale edits from winning over newer known versions.

## SQLite writes and query strategy

Use prepared statements and explicit transactions with profiling-driven batch sizes (initial experiment: 500–2000 records). Do not commit per message. Configure foreign keys, WAL, busy timeout, checkpoints, and indexes deliberately and test crash/lock behavior.

FTS5 serves filename, caption, and tag text where appropriate. Original Unicode content is preserved; tokenizer/normalization behavior for Chinese, Japanese, case, emoji, and combining characters must be measured and documented. `LIKE '%term%'` is not the primary million-row full-text strategy.

Global search uses structured facets for channel/account, type, date, size, extension, local/remote/encryption/multipart/verification states. Build parameterized queries in storage repositories; never concatenate user input into SQL.

## Cursor/keyset pagination

Library/search ordering includes a deterministic unique tie-breaker, for example `(modified_at DESC, LogicalFileId DESC)`. An opaque versioned cursor carries the last ordering tuple and query/sort identity. The next query uses keyset predicates, not deep `OFFSET`.

Cursors are invalidated/rejected when query/sort schema does not match. Concurrent inserts may appear according to defined snapshot/freshness semantics; pagination tests assert no duplicate/omitted IDs over stable fixtures, including ties in primary sort values.

The GUI consumes small pages with prefetch and virtualizes only visible/nearby rows. It never instantiates one element per indexed item.

## Progress and status

Index snapshots/events expose real, locale-neutral values:

- requested and proven coverage;
- messages scanned and files indexed;
- bytes indexed and new/updated/deleted counts;
- current historical date/bound;
- latest successful sync;
- scan rate and ETA with explicit calculation semantics;
- current state, retries, and structured error.

Raw per-message updates are aggregated to a controlled interval while terminal/state changes are immediate. Counts distinguish “examined messages” from “eligible files” and must not imply full coverage from a partial-policy scan.

## Resource and failure policy

The engine uses bounded fetch pages, bounded normalization/write channels, and a limited number of jobs per account. It never spawns a task per message or holds an entire channel in memory. Database/network work stays off the GPUI thread.

FloodWait is structured and account-scoped; authorization/access changes require user action; transient network/database-busy conditions use bounded retry; corruption/invariant errors fail safely. Cancellation and pause persist only committed truth. Logs include safe job/account/chat IDs and bounds, never Telegram sessions or private content unless an explicitly safe diagnostic policy exists.

## Testing and benchmarks

Use fake history/update sources and temporary databases. Required scenarios include:

- empty channel, messages without media, mixed content, Unicode filenames/captions;
- duplicates, edits, deletes, out-of-order updates, and update gaps;
- partial/custom ranges, disjoint ranges, overlap/merge, incompatible policies;
- 10,000-message scan interrupted after a known committed batch and resumed without gaps/duplication;
- failure before/after batch commit and process restart;
- incremental updates concurrent with historical scan;
- FTS/facets/date/size/type/extension combinations and injection-safe construction;
- cursor traversal yields every expected ID exactly once with sort ties;
- pause/resume/cancel/FloodWait/network states and throttled progress;
- catalog-neutral status storage and Chinese/Japanese/emoji/combining-text search behavior.

Benchmark synthetic datasets of 1,000, 100,000, 1,000,000, and 3,000,000 logical files for batch ingestion, FTS, facets, cursor queries, range compaction, memory, and UI page latency. Large generated databases are artifacts, not committed user-like blobs. Performance thresholds are established from evidence, not assumed in this document.
