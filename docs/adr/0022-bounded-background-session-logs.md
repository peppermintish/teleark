# ADR 0022: Keep transfer session logging off network and UI owners

- Status: Accepted
- Date: 2026-09-08
- Supersedes: ADR 0009's required synchronous session-log writes and log-failure admission behavior. Its native part-map/recovery decisions remain in force.

## Context

Native download observers run on Telegram's async reactor. Formatting a session record and flushing a file there blocks unrelated network work when storage is slow. Creating a separate writer thread for each transfer would accumulate blocked threads as transfers finish. Diagnostic failures must not interrupt file transfer or overwrite its independent recovery state.

## Decision

One retained process-owned thread writes complete session-log records. Transfer owners open private append-only file handles before network activity; callbacks stage at most 64 KiB per record and submit to a queue capped at both 256 records and 1 MiB. File I/O never holds the queue mutex. Keeping the opened handle means a queued write cannot recreate a log removed by task deletion. One writer preserves FIFO ordering across attempts that append to the same file.

On saturation, the queue evicts oldest complete records. Oversized records are omitted as a whole. It never emits a truncated JSON object intentionally. The next successful write prepends an additive schema-1 `log_records_omitted` record with `scope: "all_transfers"` and an aggregate `count`; the gap is process-wide, not attributed to that file alone. Open/write failures emit sanitized diagnostics and contribute to the same cumulative omission count. Settings and transfer details show that count with explicit process-wide scope in all three locales. Logging failure cannot fail the transfer. Live event projections and acknowledged task/recovery persistence have separate owners.

Session-log schema 1 retains every existing field's meaning. Readers tolerate this additive event and must display it as a gap, never interpolate missing decisions. No existing log or bitmap needs conversion. Current readers/writers support schema 1; unsupported future schemas remain unsupported rather than being interpreted as version 1.

## Consequences and limits

Callbacks, flush and logger drop do not wait for file writes. The thread and queue are bounded; queue memory excludes at most one in-flight 64 KiB record and each active producer's bounded staging buffer. File handles are retained by active producers and bounded queued/in-flight records. Loss counts are observable even if a destination cannot be written; a persisted gap requires a subsequent successful write.

Session logs are best-effort diagnostic replay with explicit gaps. An abrupt process exit can lose the queued tail, including its not-yet-written gap marker. It does not lose the independent part map or acknowledged task checkpoint. Session files still have task-based retention; this change does not add a disk quota or claim complete historical replay after process termination.

Tests block an artificial destination before submitting 1,001 more records, enforce both queue bounds, verify full JSONL/gap/terminal records, reject oversized records atomically, continue after storage-full errors, and complete a real fake-remote download despite an unusable log directory. No real Telegram speed claim is derived from these tests.
