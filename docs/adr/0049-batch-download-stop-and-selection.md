# ADR 0049: Stop a native download selection before cleaning its files

- Status: Accepted
- Date: 2026-09-22

## Problem

The frontend called synchronous cancellation once per selected task. Each call
waited for its writer and filesystem cleanup. Queue refill continued between
calls, starting more members of a stopped batch and locking the global action
bar until the last cleanup. Native destination reservation markers were not
included in cancellation cleanup. Batch header selection did not select the
visible child checkboxes, and auxiliary lists had no scrollbar.

## Decision

`DesktopTransfers::stop` accepts the whole bounded selection. The retained cleanup
control owner validates account ownership, fences admission/retirement and signals
all selected controls under one short lock, then publishes per-task cleanup
activity. It drops the lock before committing all stop records and cleanup leases
in one Storage transaction. Acknowledgment means durable stop intent; filesystem
completion is reported separately through the existing revisioned projections.
The existing synchronous single-task `cancel` API retains its completion semantics.

No queued member can acquire a new attempt across the stop barrier. Waiting for
the shared file limit observes stop/shutdown; metadata lookup also observes stop
while its RPC is pending. Stale worker retirement cannot overwrite cleanup state.
Cleanup selects writers that have drained, so one blocked writer does not prevent
ready tasks from cleaning their files. The retained control owner rechecks writer
readiness at its bounded 100 ms shutdown wakeup; no GUI timer services this queue.
Filesystem cleanup remains serialized and never holds the admission lock.

Cleanup removes native partial/map files and the empty shared `.partial`
reservation. It preserves final files and nonempty encrypted resume data. Failure
remains visible and retryable; a second Stop revokes a pending download retry.
Deleting history becomes available after the task's cleanup finishes. New batches
have new task identities, even when their remote file identities overlap old ones.

Batch members sort by running, queued, paused, failed, stopped and completed,
with stable identity ordering within each phase. Group selection updates all
member keys; changing a child recomputes its group's checked state. A partial
selection remains scoped to its selected children when collapsed. Native and
Vault selection namespaces remain disjoint. Auxiliary lists use the shared
virtual list handle and a persistent scrollbar.

Native batch-window close is deferred through GPUI view removal, matching the
application Close Window command. This releases the view/accessibility ownership
before Windows destroys the native handle; immediate native destruction produced
an AccessKit invalid-handle panic during Windows automation review.

## Compatibility and evidence

SQLite remains read/write schema 22, with automatic upgrades from 0–21. Cleanup
codec 1 and all native/encrypted payload and recovery formats are unchanged.
Existing cleanup leases still restore automatically; unknown codecs remain intact.
No schema migration or manual reset is required.

Deterministic tests cover 48 members with three blocked writers, queue draining,
partial/map/reservation removal, late progress, an overlapping replacement batch,
history deletion, retry revocation and restart. Storage tests verify transaction
rollback and no partial stop commit. GUI event tests exercise group/child selection,
state ordering, scrolling to the last of 48 rows, and 900×600/full-screen layouts.
These synthetic tests do not measure live Telegram performance.
