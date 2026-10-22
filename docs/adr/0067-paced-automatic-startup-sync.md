# ADR 0067: Paced automatic startup synchronization

Status: Accepted, 2026-10-17. Supersedes ADR 0031's unconditional connection-wide difference demand, ADR 0033's prohibition on inspector presentation clocks, and the manual review/recovery controls in the synchronization inspector. The durable cursor, independent channel silence recovery, account isolation and retained worker guarantees of ADRs 0064–0065 remain.

The channel silence fallback and subscription scheduling are superseded by [ADR 0068](0068-targeted-channel-sync.md); automatic ownership, durable recovery and admission remain in force.

## Problem

Each connection revision discarded transient channel state and forced every cached channel to fetch a difference. The initial directory had already supplied remote PTS, so equal durable cursors still produced redundant requests. An overdue observation could independently force the same work after a restart. Four ordinary channel workers and the reserved managed worker could rapidly drain successive pages; directory pagination also ran without spacing. Server FloodWait existed only in process memory, so reopening lost its deadline.

The inspector mixed an automatic-sync message with catalog cancel/retry and change-review buttons. Its elapsed values only changed on unrelated renders, and a relative retry countdown could become stale.

## Decision

### Automatic synchronization contract

Synchronization is a product responsibility owned by Runtime. Once the account
is authorized and local storage is ready, the account owner starts discovery and
catch-up automatically and continues across navigation, inspector closure and
connection replacement. Opening a page or the inspector must neither start nor
restart synchronization.

- Runtime owns initial discovery, incremental updates, gap recovery, request
  pacing, transient retries and server cooldowns. Routine operation must not
  require a user to start, refresh, retry, cancel or tune synchronization.
- Correctness takes precedence over appearing current: retain local data during
  waits, reconcile from committed cursors, validate and persist changes before
  advancing durable coverage, and reject stale account/connection results.
  Restart must preserve recoverable state and known server deadlines.
- The Channel Synchronization inspector provides observability only. Close is
  its sole control, and closing it does not stop background work. Phases, reasons
  for waiting, timing, completion and bounded history remain inspectable without
  requiring acknowledgment or other sync-management actions.
- Request input only for a genuine prerequisite that cannot be resolved
  automatically, such as authentication, key access or storage permissions.
  Explain that prerequisite through its owning application flow while preserving
  existing data; do not add a routine sync-recovery button to the inspector.
- Explicit requests for earlier history and file-transfer controls retain their
  separate meanings. They do not make routine account synchronization dependent
  on manual intervention.

These requirements apply to future synchronization changes as well as the
startup fix below. Verification must cover automatic start and recovery without
GUI commands, navigation/inspector independence, restart and cooldown handling,
stale-result rejection, and the absence of sync-management controls.

### Scheduling and progress

Startup still loads local data and refreshes the account directory to populate the new Telegram connection's peer cache and observe membership/head changes. A connection replacement no longer creates blanket difference demand. When a channel's durable cursor covers the freshly discovered PTS, restoring its local state postpones the channel silence fallback without issuing a redundant difference or rewriting its cursor. Genuine gap/overflow/repair/history demand remains. Missing PTS still uses the durable observation deadline; changed heads recover from committed PTS. Bootstrap and authenticated managed-catalog reconciliation remain automatic.

One account-owned budget initially spaces remote synchronization page starts by one second. A server cooldown doubles the interval once per active cooldown window, up to 30 seconds; overlapping failures cannot multiply the same window. Sixty successful pages outside an active cooldown reduce the interval by one second, down to the one-second floor. The learned interval survives restart, so reopening during a large catch-up does not discard server feedback. Local reads, contiguous pushes and empty local verification bypass admission. Waiting uses retained bounded workers, holds no budget lock across a wait, observes cancellation, and emits a distinct scheduled-check phase before a remote request. Known FloodWait immediately extends this budget before completion reaches the coordinator, preventing already queued workers from ignoring a reported cooldown. Overlapping waits retain the later deadline. Directory iteration separately yields one second per 100 returned dialogs, matching the pinned grammers 0.11 iterator's page geometry; the bounded 10,000-dialog scan has a 180-second deadline rather than 60 seconds to accommodate this spacing. These are conservative local policies, not a claim about Telegram's unpublished request quotas.

Account-scoped cooldown metadata persists separately from payloads and cursors. Reopening restores the remaining deadline before any sync network call. The account coordinator retains completion readiness independently of thread wake tokens and checks it immediately before parking: an intervening acknowledged storage call may consume an unpark token, but must not leave a queued completion asleep behind a long server deadline.

Channel Synchronization is informational. Its only control is Close; it offers no sync start, retry, cancel, review acknowledgment or tuning action. Routine transient recovery remains Runtime-owned and automatic. History continues to show channel edits/deletions and typed failures, while access/unlock/storage problems retain actionable guidance in their relevant application flows. Historical change text says changes were recorded rather than demanding acknowledgment. Explicit earlier-history demand and independent file-transfer controls retain their meanings.

The inspector shows scheduled checks separately from server waits, active/queued work, fixed next-check timestamps, elapsed phase/last-activity values and bounded history. A retained one-second presentation task invalidates only the open active/waiting inspector, preserving the immutable history projection and avoiding application/workspace notifications or service/database polling. Idle/terminal inspectors stop ticking. No percentage, throughput or ETA is invented. Compact and native full-screen inspectors retain independent scrolling.

## Persistent compatibility

SQLite schema remains 26; preferences remain v1 and payload/recovery codecs are unchanged. New optional setting `channel-sync-cooldown.<account-id>` uses independent JSON codec v1: `{"version":1,"until_unix_ms":<signed UTC Unix milliseconds>,"interval_ms":<1000..30000>}`. A missing record, including upgrades that skip any prior release, means no saved server cooldown; existing data needs no conversion or reset. Runtime reads/writes v1 only, and stores the maximum existing/reported deadline in an atomic settings write. Expired deadlines allow sync while retaining the learned interval. The additive optional `interval_ms` field does not change existing deadline semantics: older v1 readers ignore it; a legacy v1 record without it automatically adopts a two-second interval because it records a prior server wait. Successful recovery may lower the saved interval while never shortening a live deadline. Unsupported versions or malformed records remain intact and stop this sync owner with storage/compatible-version guidance. This metadata contains no credentials, channel contents or payload bytes.

## Evidence and limitations

Deterministic coverage exercises old observations across repeated launches and connection replacement (32 unchanged channels: zero difference requests; one changed head: one difference), restart cooldown admission, account-scoped/monotonic codec persistence and future-version preservation, inter-page spacing, cancellation/feedback before requests, released locks, overlapping FloodWait and retained completion readiness. GUI checks exercise both 900×600 and native full-screen flags, distinct scheduled/server-wait presentation, absence of manual sync controls, immutable history and quiet parent notifications. Native synthetic English/light review covers 900×600 and actual macOS full-screen, the normal Network settings entry point, the Close control and independent inspector scrolling.

The deterministic and synthetic UI checks use no real account or local credentials. Real-account release qualification is recorded separately in the validation report. Telegram may still return rate limits, especially for large initial imports or shared account activity; this change removes demonstrated redundant work, paces necessary catch-up and honors known waits. It does not certify an account-specific rate-limit frequency or end-to-end speed improvement.

Protocol references: [Telegram update reconciliation](https://core.telegram.org/api/updates) and [server error handling](https://core.telegram.org/api/errors). The inspected grammers 0.11 public iterator is MIT OR Apache-2.0; no incompatible implementation or new dependency is used.
