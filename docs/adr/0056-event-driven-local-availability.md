# ADR 0056: Observe local outputs from filesystem and application events

- Status: Accepted
- Date: 2026-09-23
- Supersedes: ADR 0050's regular three-second/30-second checks and 100 ms controller wake

## Problem

The GUI subscribed to local-availability changes, but Runtime still woke every
100 ms and repeatedly read file metadata. Selected-channel destinations were
checked every three seconds and other retained destinations every 30 seconds,
even when nothing had changed. Similar short idle loops remained in transfer
cleanup and the bandwidth presentation. Active startup, storage and key feedback
also repainted on short timers without waiting for phase transitions.

QR login also asked for a new state every 750 ms despite native login updates.

## Decision

Runtime watches the nearest existing parent of each retained output with the
platform's native filesystem notification backend. New downloads, account and
channel changes, completion revisions, filesystem changes, watcher coverage
changes, errors and worker completions wake the controller through bounded
channels. It waits until an event or the next actual deadline; no fixed 100 ms
controller loop remains. The watcher runs on a separate owner so registration
and mount checks cannot block the controller or UI. Paths reported through a
resolved directory alias are mapped back to the recorded destination path.

An initial probe and a second probe after watcher registration close the gap in
which a file could change before notifications are armed. Events immediately
remove a positive claim and coalesce probes for 200 ms. A result started before
a newer event is discarded. A watch error, dropped event batch or lost coverage
causes a bounded recheck; failed registrations retry after one minute. Existing
account epochs and per-path tokens reject stale results, and the 10,000-path,
four-worker and 32-request bounds remain.

The selected channel gets one more check two seconds after a new watch is
registered or selected. This covers platform delivery startup under load without
adding recurring short checks across the retained inventory.

When a watcher is healthy but quiet, a five-minute reconciliation is the
fallback for missed notifications. An unwatched destination falls back to 30
seconds in the selected channel or two minutes elsewhere. Startup and account
selection check immediately. A 30-second lightweight clock compares wall and
monotonic time; a sleep gap rechecks retained paths after wake without a short
filesystem polling loop. These are scheduling limits, not guarantees for an
unavailable volume or a blocked filesystem call. A probe taking 15 seconds or
longer cannot publish an old positive observation.

Transfer cleanup now wakes on writer retirement, with a 30-second fallback only
while a writer is outstanding. Bandwidth snapshots publish bounded revision
events, so the GUI parks while idle and coalesces active updates. Startup,
storage maintenance/setup, Vault key and filtered-batch operations publish
phase/terminal revisions. Their one-second timers only advance visible
elapsed-time labels while work is active; they do not read Storage or the
filesystem. Keychain feedback uses the same presentation interval.

QR login waits for Telegram's native update or token expiry. Queued download
admission and shutdown waits wake on control or retirement, and part retries use
their actual deadlines. A compatibility observer without a control subscription
retains a five-second active cancellation fallback.

## Compatibility and limits

All watcher state is ephemeral. SQLite read/write schema 23 and automatic
upgrades from 0–22, application payloads, credentials, transfer checkpoints and
recovery codecs are unchanged. Existing download history and final user files
are never removed because an observation is missing or unavailable. The
metadata check still observes type and size rather than cryptographic identity;
same-size replacement requires the existing authenticated download path for
verification. Native watcher delivery can be incomplete on some volumes, so
the documented quiet reconciliation remains necessary.

Tests cover watcher handoff, external deletion after history removal, alias
paths, event races, watcher loss, bounded retained changes, sleep-gap detection,
writer retirement and subscription delivery. UI tests use English at 900×600
and actual full-screen mode as required by the contributor rules.
