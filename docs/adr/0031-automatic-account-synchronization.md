# ADR 0031: Automatic account synchronization and viewport history

Status: Accepted, 2026-09-12. Supersedes the GUI-owned initial discovery, single synchronous execution lane, explicit history buttons and duplicated sync controls described in ADR 0018 and ADR 0019. Their cursor, authenticated private-file projection and loss-recovery contracts remain in force.

[ADR 0033](0033-static-sync-inspector-and-navigation-independent-updates.md) supersedes the navigation interests, counter presentation and unconditional reconciliation described below.

## Decision

Synchronization begins when a restored or newly authorized account is ready, including while the workspace is locked. Runtime owns initial remote directory discovery as well as subsequent reconciliation. The GUI preparation job restores local upload history and activates transfers; its success is not a prerequisite for channel synchronization. Initial session connection and private-storage discovery retry transient failures automatically with capped delays. Authorization, local persistence and permission problems remain visible and actionable.

Telegram publishes account readiness and connection revisions through a retained lifecycle object outside the reactor. Route replacement invalidates the old generation before retiring the reactor. Subscribers register before reading their baseline. A lifecycle change cancels active requests and wakes the coordinator; old reactor publications cannot restore an older generation. The coordinator replaces transport subscriptions, repopulates the new reactor's peer directory and recovers from committed cursors. A retiring transport consumer clears only the callback it installed, so it cannot disconnect a replacement consumer's notifications.

A transport channel-metadata update triggers directory reconciliation, coalesced with work already in flight. Missing-channel hints and coverage gaps also invalidate the directory. A one-second coalescing deadline and the existing 15-minute healthy reconciliation fallback are service deadlines, not GUI refresh loops. Actual transport data wakes network backoff early; server FloodWait is account-wide and remains authoritative. Channel/server retries have no attempt cutoff and use equal jitter with a 60-second ceiling. Unsupported access, persistence and authorization failures do not discard data.

The coordinator owns at most four channel operations and one independent directory operation. Each channel has one operation in flight, retaining its job state and cancellation handle; incoming pushes and user commands are merged with the returned state. Blocking a remote channel or a later directory reconciliation does not occupy the other channel slots. A connection-revision change rejects late responses. Source-directory persistence also checks cancellation on the Storage owner before writing. Channel writes retain their existing compare-and-swap revision guard. A worker panic produces a terminal failure rather than leaving an invisible occupied slot. Shutdown cancels and joins the retained work on the coordinator/reaper, never on the GUI or Telegram reactor.

## Presentation and demand

A committed directory revision publishes an immutable latest snapshot. The sidebar subscribes before reading its baseline and replaces membership/titles only on a metadata revision; coalesced notifications cannot lose the latest rename or departure. Cached metadata is a baseline, not evidence of current remote access. Initial source availability still depends on account/session readiness.

Channel and private Raw Files use their existing coherent local rows/revision baseline and material deltas. Material channel commits invalidate the Remote Library projection; its background replacement preserves matching selections and the visible rows while reading. Repeated invalidations coalesce, unchanged/phase-only events do not query the library, and pagination waits for a conflicting replacement. Private metadata changes trigger a storage-state recheck; authenticated manifest updates continue through the existing managed-file event feed.

The account owner retains one viewport history demand with a unique request ID and explicit pending/completed/cancelled/failed state. Scrolling near the end of Channel or private Raw Files requests one 200-message page. The same page owner is not requested again while pending. Navigation cancels that demand without pausing live synchronization. Old completion/cancellation cannot change a newer demand or overwrite a terminal result. Live pushes may cancel an attached history read while preserving the demand, allowing live work to run first; history resumes after it. Empty/exhausted history finishes the demand without fabricating progress. The 5,000-row display cap remains. Legacy Saved Messages recovery retains its separate explicit reader.

Routine refresh buttons are removed from the channel sidebar, file header and managed-storage header. The per-view sync footer and Cmd-R refresh action are removed. The bottom status entry opens one independently scrollable inspector without navigating. It shows actual phases, active/queued counts, phase duration, last activity, retry deadline, structured failures and the bounded event timeline. Multiple active operations remain visible when one completes. Recovery controls remain in the inspector and failure-specific views; a completed local preparation job has no refresh action. Initial connection retry and Remote Library replacement have visible status too.

## Bounds and compatibility

There are at most 10,000 directory entries, 64 queued commands, four channel workers and one directory worker. Each retained channel push batch and the coordinator's incoming push budget are bounded at 512 records/4 MiB. A merged batch exceeding that bound is discarded with an overflow counter and difference recovery; moving a batch to a worker does not clone its payload. The bounded workers plus pending budget retain at most five such batches. The transport and committed-delta journals retain their existing independent 4 MiB limits. Five active phase records and 128 timeline events are retained, with explicit event truncation. History retains one request record.

SQLite remains read/write schema 16 with its existing automatic upgrade paths. No crypto, manifest, recovery, transfer or preference field changes meaning. The new directory cache uses independent codec version 1 in the existing settings table:

- Header key: `channel-directory.<account_id>`; JSON object `{"version":1,"pages":N}`.
- Page keys: `channel-directory.<account_id>.<zero_based_page>`.
- Each page is a JSON array of at most 128 entries. An entry is exactly `[id, name, username_or_null, kind]`, where `kind` is `user`, `group` or `channel`.
- At most 79 pages and 10,000 total entries; each setting retains Storage's 1 MiB bound. Names and usernames are verbatim. PTS is never restored from this cache.
- Header and pages are saved in one settings transaction. Fewer pages or an empty directory changes membership atomically; older unused bounded page slots are ignored. File data, cursor rows and original chat records are not deleted when a source departs.
- With no header, installations from any supported prior release read the account's existing channel-cursor/chat join as a conservative baseline. The next successful remote directory creates version 1 automatically. No export/reset or manual script is required.
- Malformed and unsupported/newer headers fail intact, before cache replacement. Existing directory bytes remain available for a compatible reader. A rejected oversized page leaves the previous cache intact.

## Evidence and remaining qualification

Deterministic tests exercise the actual coordinator starting without GUI discovery, blocked channel and directory calls, independent completion, connection replacement and late-page rejection. Other tests cover immutable/coalesced metadata baselines, retry limits/classification, FloodWait, direct pushes and gaps, viewport request replacement/cancellation, local cache preservation, source account scope and unsupported cache versions. English-only compact UI tests verify the global status on every page and absence of routine refresh controls.

Protected real-account qualification remains separate: cross-device delivery latency, extended outages, real Telegram rate limits and very old server history gaps are not simulated performance guarantees. The pre-existing historical-discovery and exact-ID reconstruction limitations in ADR 0019 remain; automatic synchronization does not assert exhaustive reconstruction of Telegram history unavailable to the server APIs.
