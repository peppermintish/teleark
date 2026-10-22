# Startup and synchronization latency diagnosis — 2026-10-23

## Scope and evidence limits

This is a diagnostic investigation, not a behavior change or release acceptance.
Source inspection used repository revision `7c145f0` (application 0.5.19).
The installed macOS application reports 0.5.7. No TeleArk process was running.
The latest available local process diagnostics end on 2026-10-17; they are
historical evidence and do not measure the user's newly reported launch or
establish its executable version. No application was launched, remote account
was contacted, credentials were loaded, or local application state was changed.
Local SQLite inspection used a read-only connection and selected only relevant
settings and aggregate synchronization counts. No account identifiers, channel
names, filenames, message content, keys or raw diagnostics are included here.

## Current source behavior

`channel_sync::run` first publishes `ReadingLocal`, reads the persisted directory,
restores the managed watch and saved account admission policy, then binds the
current connection. Initial and replacement connections refresh the remote
dialog directory. Channel jobs cannot start until `metadata_ready` is true.
`list_sync_dialogs` inserts a one-second delay per 100 returned dialogs, with
a 10,000-dialog cap and an enclosing 180-second timeout. The timeout is an upper
bound for that attempt, not an expected startup duration.

After discovery, jobs load durable per-channel state. Equal known remote/local
PTS with no independent work does not require a difference. New channels seed
one recent history page and then fetch a difference. Newer heads, missing push
sequences, overflow, unfinished gap filling and durable repair require targeted
work. Repair verifies cached message IDs; gaps can require multiple history
pages. Explicit earlier-history demand remains separate.

Contiguous pushes apply directly without admission/network reads. The visible
ordinary channel and the managed storage channel also renew subscriptions after
final differences using the returned timeout, defaulting/clamping to one second.
Closing/locking an ordinary view retires that view's subscription. Ordinary
background channels have no recurring age-based difference sweep in 0.5.19.
These semantics are documented in ADR 0068 and differ from older releases.

`CheckingPts` is published before selecting the next operation, including a
direct push, history, difference, manifest discovery or verification. It is not
a dedicated PTS-check RPC. `PollingDifferences` is the remote difference phase.
An unchanged successful difference still records a durable observation timestamp,
but does not advance the file projection revision.

`ReadingLocal` describes several operations: account directory restoration,
channel cursor reads, managed-watch restoration, cached-ID reads for repair/gap
boundaries, and preserved manifest inventory reads. Storage requests acknowledge
through a shared worker; their elapsed time includes admission/queueing and
execution. A displayed phase duration alone cannot identify SQLite query time,
and a historical event is not necessarily ongoing work.

Managed Files have additional readiness work: finding manifests, downloading
manifest documents on an in-memory cache miss, authenticating/decrypting metadata, updating the
inventory and producing the displayed file projection. This is distinct from
ordinary message synchronization. It does not imply downloading/decrypting all
stored file payloads. Its active phase can take priority in the inspector label.

## Admission and presentation findings

`channel_sync/pacing.rs` spaces remote synchronization page starts across the
account. Production admission starts at one second. Server waits double the
interval once per cooldown window to a maximum of 30 seconds. Sixty successful
pages outside an active server wait reduce it by only one second. Both server
deadlines and the learned interval survive restart. An expired server deadline
does not reset the interval to one second.

At a 30-second interval, ten eligible remote page starts span approximately
270 seconds before accounting for RPC latency, additional server waits or
retries. This is a scheduling example, not a measurement of the reported launch.
Even with uninterrupted successes, reaching one second from 30 seconds requires
1,740 successful pages under the current recovery policy.

The footer maps `Pacing`, `Waiting` and `RateLimited` to `Waiting to sync`.
Queued admission work can also keep that label visible after another channel
successfully completes. A scheduled future subscription deadline alone does not
mark a worker active/waiting; once renewal becomes due it can queue behind the
shared budget. Consequently a frequently renewing subscription, especially
alongside another watched channel or repair, can repeatedly present as sync
activity despite already-current content.

The product currently exposes low-level execution stages through a single
overall status without a distinct durable indication that startup catch-up has
finished while live subscriptions continue. The inspector label can prioritize
managed scanning, the runtime snapshot prioritizes active/waiting work, and the
timeline retains completed transitions. These different scopes make the status
hard to interpret. Continuous waiting is not proof of a deadlock, nor is it proof
that startup catch-up has succeeded.

## Historical measurements

Times below are from existing process diagnostics. Network-page elapsed values
exclude the preceding Runtime admission wait.

| Observation | 2026-10-17 result |
| --- | --- |
| Local directory cache load | 1 operation, 5 ms |
| Channel state loads | 36 operations, 0 ms at logged resolution |
| Managed-watch load | 1 operation, 0 ms at logged resolution |
| Remote dialog discovery | 1 operation, 3,775 ms |
| Channel differences | 60 operations, median 235.5 ms, maximum 1,164 ms |
| Gap history | 9 operations, median 1,124 ms, maximum 1,716 ms |
| Cached-message verification | 5 operations, median 289 ms, maximum 378 ms |

The same log records server retry delays of 18, 3, 3 and 5 seconds during cached
message verification, and an admission interval increase to 30,000 ms. Earlier
2026-10-16 diagnostics record interval increases to 4,000, 8,000 and 16,000 ms.
The inspected local cooldown record retains a 30,000 ms interval and an expired
historical server deadline. Aggregate local cursor state contains 38 channels,
three with unfinished repair, zero with a pending gap and zero at revision zero.
These observations explain a real mechanism for long waits and resumed repair;
they do not establish current remote heads or the user's latest bottleneck.

## Official Telegram comparison

Only official protocol documentation was consulted; no external client
implementation was inspected. Telegram documents pushed updates, independently
tracked channel PTS and targeted gap recovery in
[Working with Updates](https://core.telegram.org/api/updates). Visible channel
subscriptions use server-timed difference renewal; see
[channelDifferenceEmpty](https://core.telegram.org/constructor/updates.channelDifferenceEmpty)
and [getChannelDifference](https://core.telegram.org/method/updates.getChannelDifference).
The protocol supports prompt live updates without repeated full history scans.
This does not constitute a benchmark or implementation analysis of an official
Telegram application. TeleArk's global admission policy and extra authenticated
file catalog work must be assessed separately from the underlying protocol.

## Follow-up priorities and validation

1. Capture the actual running version and one complete launch with separate
   queue, storage execution, RPC, retry and catalog-projection timings.
2. Distinguish caught-up content/live subscription from catch-up backlog and
   name local pacing, server cooldown and failure retry separately.
3. Assess recovery of a persisted high admission interval and competition
   between subscription renewal and real repair, while respecting server waits.
4. Identify the precise operation behind multi-second `ReadingLocal`; existing
   historical cache measurements do not reproduce that symptom.

Validation consisted of source-path tracing, official protocol verification and
read-only aggregation of existing local diagnostics/state. No behavioral tests
or UI previews were run because application behavior and layout were unchanged.
Existing release test results are not treated as new performance measurements.

## Follow-up: incrementality and avoidable work

The current implementation cannot be described as entirely incremental or
verified maximally efficient. Channel message catch-up uses durable cursors and
targeted differences, but that property does not extend to every startup/catalog
operation. In particular:

- Remote dialog discovery enumerates a fresh directory on each new connection;
  it is not a delta from the persisted directory. It supplies current peer
  references and channel heads, so removing it without a replacement would not
  establish correctness. Its full completion gates channel jobs today.
- `VaultOwner` initializes `ManifestCache::default()`. `scan_cached` obtains
  candidates from SQLite and passes uncached `.tarkm` candidates to
  `health::load`, which unconditionally calls `store.download` before opening
  and persisting the envelope. That loader does not first reuse the persisted
  `VaultInventoryRecord.sealed_manifest`. The later `retain_missing` path skips
  files already present in the report, so it does not prevent those downloads.
  A new process with an empty memory cache can therefore download unchanged
  manifest documents again as it projects its catalog (within existing caps).
  These are metadata documents, not whole encrypted file payloads. Safe reuse
  needs account/channel/message-version validation and authenticated opening;
  no change should blindly trust an obsolete envelope after an edit or gap.
- Within one process, unchanged manifest candidates reuse cached projections;
  changed message candidates invalidate their own entries. However each
  projection still enumerates candidates, retains entries, clones results,
  sorts the report and checks local file health. This is not an entirely
  change-proportional pipeline. Cache clearing/eviction also creates misses.
- Unchanged channel difference responses avoid file-row revision churn, but
  still update the durable successful-observation timestamp. Necessary live
  subscription traffic must be distinguished from actual catch-up and from
  avoidable repeated work.
- Persistent adaptive admission can leave necessary requests delayed even when
  the payload work is incremental. Incrementality alone does not demonstrate
  good latency or justify the current recovery slope.

The relevant source is `channel_sync.rs`, `channel_sync/pacing.rs`,
`vault.rs::scan_cached`, `vault/catalog.rs::ManifestCache::project_observed`, and
`vault/health.rs::load`. Existing catalog tests demonstrate unchanged-candidate
reuse in the same cache instance; they do not prove zero downloads across a
process restart with a populated persistent inventory. This follow-up used
source inspection, not a new live performance measurement or test execution.
Required evidence before claiming efficient restart includes operation-count
regressions for unchanged cold/warm starts, one changed manifest, sequence gaps,
edits/deletes, account/key isolation and paced request recovery, together with
separate queue/RPC/storage timings. No runtime implementation was changed.

## Running-instance inspection after user launch

The user subsequently opened an instance and requested another inspection.
The process executable resolves to this workspace's `target/release/teleark`,
not the separately installed 0.5.7 application. Its open database and process
diagnostic file were identified from its file handles. The executable was last
modified at 2026-10-22 18:42:24 UTC. The current process diagnostic stream does
not contain `application.starting` or an explicit version record; the exact
embedded executable version was not independently confirmed. Current source
metadata remains 0.5.19. Inspection did not restart/rebuild the executable or
issue a synchronization/transfer command.

The latest initialization boundary in the open daily log starts at 2026-10-22
18:42:26 UTC. The following snapshot ends at 19:12:00 UTC, corresponding to
2026-10-23 05:12:26–05:42:00 in Australia/Adelaide. Earlier initialization
records in the same daily file are excluded.

| Observation | Current-session result at snapshot |
| --- | --- |
| Synchronization remote pages | 60 |
| Differences | 32; median 332 ms; maximum 20,002 ms |
| Gap history | 22; median 952.5 ms; maximum 5,206 ms |
| Cached-message verification | 4; median 2,251 ms; maximum 12,289 ms |
| Directory discovery | 2; median 3,382 ms; maximum 4,124 ms |
| Sum of logged remote-page durations | 86,229 ms |
| First-to-last remote request start span | 1,770.909 seconds |
| Consecutive request start intervals | 59; median 30.006 seconds; range 29.997–30.089 seconds |
| Remote failures | 1 network failure, with a 20,002 ms duration |
| Remote responses carrying a server retry delay | 0 |
| Storage operations taking at least one second | 0 |

The local directory-cache read and 36 channel-state reads log 0 ms at the
logger's integer-millisecond resolution; the initial local channel view logs
1 ms. These observations do not reproduce a multi-second database cache read.
The saved cooldown record still contains 30,000 ms spacing with the old server
deadline already expired. There is no interval-change event in this session at
the snapshot. The regular 30-second request spacing and saved policy support
local admission as the dominant delay; the single failed RPC and some slower
requests are additional, smaller costs. The accumulated remote-page time is
not a full startup CPU/I/O profile, and the remaining wall time must not be
treated as an exact measurement of one phase.

A separate 38-second read-only observation, 19:10:59–19:11:37 UTC, recorded two
successful gap-history pages taking 884 and 790 ms. Comparing SQLite cursor
snapshots found one channel's gap cursor and revision advancing. At the end,
38 channel states were retained, 10 had pending repair and five had pending
gap filling (the two sets may overlap). Thus recovery was making durable
progress; it was not wholly stalled. There is no established remaining page
count or defensible completion ETA. These counts do not identify which channel
the user was viewing or prove the necessity of every retained repair.

The live evidence confirms significant work delayed by persisted admission.
It does not establish that manifest re-downloads are this session's dominant
bottleneck, nor that a once-per-second inspector transition is a once-per-second
network request. The native UI inventory did not expose this unbundled process,
so the inspector's actual current label/phase timing was not directly observed.
No UI layout changes or visual acceptance are claimed. Validation used existing
diagnostic events and read-only SQLite snapshots; no new behavioral tests ran.
