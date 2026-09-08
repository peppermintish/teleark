# ADR 0023: Budget native history without hiding recoverable work

- Status: Accepted
- Date: 2026-09-08

## Context

A bound on each task's replay does not bound a long-running application's memory. Thousands of finished tasks retained thousands of part events and controller decisions each. Conversely, restoring only the newest 10,000 database rows could omit older queued, paused or retryable tasks. Paging presentation must not discard recoverable work.

## Decision

Native task admission shares a 10,000-record resident budget. Concurrent admissions reserve capacity atomically before persistence, release failed reservations, and commit admission when database insertion succeeds. When needed, the owner removes older completed/cancelled groups from its in-memory view, evicting whole resident batches. A batch with recoverable or scheduled members is protected. Failed, paused, queued and running tasks are never capacity-eviction candidates. Capacity exhaustion returns a structured error before accepting additional work.

Database history restoration selects every recoverable record and its batch members, then fills remaining presentation capacity with recent completed/cancelled history. Older databases may already exceed the new admission budget: those recoverable records are preserved and restored automatically, and no additional over-budget work is accepted while the backlog remains. No data reset or manual migration is required. The initial omission count and subsequent in-memory evictions are disclosed in the transfer list, with explicit local-account scope.

Eviction removes no database row, log or user file. Historical redownload can load one cold database record and still validates its account before allocating a new destination. Existing local-output inventory reads remain independent of the transfer-history view.

The native worker retains full bounded replay for scheduled/running work and the 16 most recently active idle tasks. Older idle tasks keep 20 part events and 32 controller decisions. Part-history observation counts and controller sequence numbers disclose the omitted prefix. Each task retains at most 256 lifecycle events, with an explicit omitted-event count and the most recent outcome. Compaction runs off the UI and preserves the ability to append/retry using the independent part map.

## Consequences and limits

The normal resident task set and detailed replay population no longer grow indefinitely with session duration. Existing oversized recoverable backlogs are an intentional compatibility exception bounded by their stored input; they are not silently truncated. Capacity limits are admission/presentation policy, not a new storage schema. SQLite schema 13 and all existing recovery/bitmap bytes keep their meanings.

Historical totals displayed by the transfer list describe retained items when an omission notice is present. A closed historical batch already partially excluded by initial history selection is not reconstructed into a complete cold replay. Log files provide best-effort diagnostic records under ADR 0022; this decision does not add a log-file disk quota or promise complete replay after restart.

Regression evidence covers 10,020 legacy tasks, older recovery rows and their completed batch members, concurrent admission and failed-reservation release, protected active/retryable batches, database/file preservation, cold redownload account isolation, compacted-history appends and bounded lifecycle outcomes. Retained event counts are memory-workload evidence, not claimed process RSS or UI frame-rate measurements.
