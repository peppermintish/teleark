# ADR 0020: Event-driven transfer presentation

Status: Accepted, 2026-09-08.

## Decision

Native and Vault transfer owners publish through frontend-neutral stores with task-ID lookup, immutable shared views and per-record invalidation. The UI subscribes before reading its first view, fetches snapshots off the UI thread and renders its cached view. Unchanged records retain their allocation; phase and terminal updates bypass sample coalescing. A retained, bounded publisher waits for actual mutations and coalesces ordinary samples over 100 ms, including a trailing update when input stops. Only active duration labels retain a one-second UI clock.

Native durable queue admission belongs to its runtime worker. Completing a task or activating its account refills available capacity; GUI presence and redraw frequency never control scheduling. Existing account checks, durable queue records, cancellation and restart semantics remain in force.

The in-memory part replay is bounded and reports omitted events. Its counters update incrementally; total completion follows completed byte progress independently of replay retention. No persistent codec changes are required by these presentation structures.

## Consequences and evidence

This supersedes fixed transfer-view polling and repeated full telemetry copying in the earlier desktop implementation. It does not increase transport concurrency or weaken verification. Detailed telemetry remains available to the inspector. Controlled tests and manual comparison commands are recorded in [the performance audit](../PERFORMANCE.md).
