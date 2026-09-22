# ADR 0050: Expire local availability and observe retained paths independently

- Status: Accepted
- Date: 2026-09-22
- Supersedes: ADR 0013's GUI-owned periodic inventory observation mechanism

## Problem

Channel badges used a cached filesystem observation without an age limit. The
GUI repeatedly paged transfer history to choose paths to check. Removing native
history could leave an already cached positive observation permanently outside
those pages. A slow filesystem operation held up every later observation in the
same refresh. Older inventory pages could also reassign a reused path to an
older source. Historical completion was correctly retained, but the displayed
local availability could cease to reflect the filesystem.

## Decision

Runtime owns a retained local-download observer with a separate inventory reader,
four filesystem workers, a 32-request probe queue and a controller that never
waits for SQL or filesystem work. GUI observes coalesced path changes and only
provides account/generation and current-channel context. No GUI timer reads the
inventory or drives the probe queue.

New paths publish Checking before probing. Regular files of the recorded size
become Present; missing, size-changed and unavailable paths retain their distinct
states. All observations expire after 15 seconds and return to Checking even if
a worker remains blocked. An individual check taking 15 seconds or longer yields
Unavailable rather than publishing a potentially old positive observation.
Known paths in the selected channel are rechecked after three seconds; other
retained paths are rechecked after 30 seconds and are prioritized when their
channel is selected. These are scheduling intervals, not an end-to-end timing
guarantee on an unavailable filesystem.

Filesystem refresh uses retained path identities independently of inventory
discovery, so deleting transfer history cannot freeze an availability badge.
Initial discovery pages existing successful downloads, including old records.
Successful native/Vault completion and legacy account resolution advance an
ephemeral inventory revision; unchanged revisions cause no repeated SQL reads.
Inventory errors retry separately and cannot stop filesystem observations.

The observer and presentation index retain at most 10,000 destinations and keep
incremental age/due/expiration indexes. A localized channel footer discloses when
older destinations are omitted. Older source metadata cannot overwrite a
newer association for the same path. Each probe has a scope epoch and request
token: account changes and path replacement reject late results. A stalled
frontend receives a bounded replacement if its pending changes exceed the limit.
Dropping the observer requests shutdown without joining filesystem threads on a
UI or network reactor. The fixed worker count bounds blocked filesystem calls.

## Compatibility and limits

The observations, revisions, epochs and Checking state are ephemeral. SQLite
remains read/write schema 22 with automatic upgrades from 0–21. Payload, recovery
and cleanup codecs are unchanged; no migration or manual reset is needed.
Neither a missing file nor observation failure deletes history, inventory or
remote files, and it never starts a download. Vault inventory survives transfer
history deletion. As in ADR 0013, matching size is a metadata observation, not
cryptographic verification; same-size replacement is outside its detection.

Tests use temporary files, fake blocked probes and controlled timestamps to cover
deleted historical outputs, pagination, history-independent rechecks, expiry,
changed sizes, new completions, stale results, account changes and nonblocking
shutdown. GUI checks use English and light-mode previews at 900×600 and actual
full-screen.
