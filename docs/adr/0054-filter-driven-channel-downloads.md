# ADR 0054: Download channel filters into isolated batch folders

- Status: Accepted
- Date: 2026-09-22

## Decision

The channel toolbar exposes a Batch action independently of row selection. It
freezes the current file-kind and relative-date filters at admission, then reads
all matching indexed source files, including files outside the browser's retained
5,000-row page. The UI states this indexed-only scope and the 5,000-task limit.
No claim is made that unindexed remote history has been downloaded. Normal
channel indexing continues automatically; the manual row selection action keeps
its existing behavior.

Storage reads candidates in pages of 256 using the account, channel and strictly
descending message identity. The source identity index serves the range and
ordering at deep cursors. Tombstones exclude removed messages. Display timestamp
nulls and ties cannot skip or duplicate rows. Runtime applies the same filename
classification and frozen date bounds as the table. Discovery retains at most
5,000 matches; an oversized result fails before creating any folder or transfer.
The cached projection can advance during discovery: newer message IDs above the
initial page do not enter the batch, and each candidate is evaluated as read.
This is not a database-wide snapshot or a remote completeness guarantee.

Each successful filter admission creates a separate `batch-<timestamp>-<suffix>`
folder under managed Downloads through exclusive directory creation. Unix folder
permissions are 0700. Remote basenames cannot escape that folder, and colliding
names receive distinct suffixes. A bounded private directory of empty reservation
names asks the destination filesystem to arbitrate case and Unicode aliases for
final files, partials and maps; it is removed before queue admission and never
contains payload data. Persisted task destinations preserve the folder
across restart, retry and history inspection. Failed preparation only attempts to
remove its empty folder; it never recursively removes an accepted output.

Discovery and folder work run off the frontend thread in a retained task.
Acknowledgment precedes this work. A bounded snapshot retains every preparation
phase transition and its time; only current counters are sampled. Channel and
global status present the current phase, duration, last activity, cancellation
and terminal outcome. A presentation timer observes progress and never drives
Storage or service queues. Navigation does not cancel or hide preparation.
Account reset or session loss cancels pending preparation, while runtime rechecks
account ownership between pages and before admission. The cancelled owner remains
retained until blocked work drains, so a replacement account cannot overwrite it.

Cancellation is serialized against the start of durable queue admission. Before
that boundary it prevents task creation. After that boundary, the existing batch
Stop operation owns cancellation, writer draining and partial-file cleanup.
Payload verification, private partial files and final publication are unchanged.

## Compatibility and evidence

No persistent schema, native transfer codec or encrypted payload format change is
required by this feature. Existing batch histories and destinations remain valid.
The accompanying credential work independently versions its own persistence.

Deterministic tests cover filters finding older rows outside the browser page,
duplicate names, separate private folders, restart, empty and oversized results,
account isolation, cancellation while a dependency is blocked and the admission
boundary. Storage tests verify tombstones, null/tied timestamps and real query
plans at initial and deep cursors. English GUI tests cover an unselected Batch
button, progress after navigation, account reset and 900×600/full-screen layouts.
These tests use synthetic data and do not measure live Telegram throughput.
