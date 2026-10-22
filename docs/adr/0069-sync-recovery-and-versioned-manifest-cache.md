# ADR 0069: Time-based synchronization recovery and version-bound manifest reuse

Status: Accepted, 2026-10-23. Supersedes ADR 0067's 60-successful-page interval
recovery rule and same-process-only manifest reuse described by ADR 0064.
ADR 0068's targeted differences, subscription timeouts, retained owners and
server-deadline guarantees remain in force.

## Context

Live diagnostics showed 60 sync requests spanning 29.5 minutes at 30-second
spacing, while their accumulated remote-page time was about 86 seconds. A
historical server cooldown had expired, but restoring its interval and requiring
60 successful pages per one-second reduction made recovery depend on the very
throughput the policy suppressed. Ordinary network errors reset that progress.

The derived manifest cache was memory-only. A normal catalog load after restart
redownloaded candidates even when encrypted recovery inventory existed. That
inventory deliberately retains the first authenticated envelope; it cannot
safely act as a current-message cache after edits without a version binding.

## Decisions

The account budget retains the authoritative server deadline separately from
its adaptive spacing. A new server cooldown still doubles spacing once per
active window, up to 30 seconds, and overlapping waits retain the later deadline.
After each complete 60-second window without a new server wait, local spacing
halves to a one-second floor: 30, 15, 7.5, 3.75, 1.875, 1 seconds. Recovery does
not require page counts or a new periodic service timer. Admission and completed
requests evaluate the existing monotonic clock; ordinary network errors do not
reset the anchor. All starts remain serialized by the same budget. A recovered
interval recomputes eligibility from the last actual start so waiters cannot
burst. Persisted v1 cooldown fields retain their meanings; restart derives
elapsed time from the later of the server deadline and setting update timestamp.
A backward wall-clock change cannot shorten a known future server deadline.

SQLite schema 27 adds `vault_manifest_cache`, holding a current encrypted
envelope per account/channel/message, with source revision and remote name.
Lookups and atomic writes join the actual channel version, reject tombstones,
and bypass reuse during unfinished gap/repair work. Runtime authenticates cached
bytes and validates manifest identity/locators before deriving metadata. Missing,
changed and damaged entries are downloaded; corruption is repairable rather than
silently trusted. Wrong/unavailable keys cannot yield plaintext. Original
`vault_inventory` envelopes remain independent and unchanged. Cache publication
follows recovery-inventory persistence. Existing inventory cannot be backfilled
without proof of its source version, so supported upgrades warm the new cache
once. All subsequent unchanged cold starts can reuse authenticated local bytes.

Runtime snapshots expose actual catch-up demand and directory readiness
separately from subscription count. Ordinary subscription renewals do not make
already-current content appear to be a new startup catch-up. Directory work,
real gaps/repairs, catalog preparation, failures and server cooldowns remain
visible. The shell distinguishes local pacing, server cooldown and retry waits;
the inspector retains detailed live phases and its bounded timeline. GUI does
not infer protocol readiness from error prose or its presentation clock.

## Compatibility and validation

Release 0.5.20 reads/writes SQLite schema 27 and automatically upgrades 0–26,
including skipped releases, using verified restartable transactions. Existing
files, keys, original sealed recovery inventory, downloaded-output inventory and
recoverable transfers retain their meanings. Payload/manifest/key/recovery and
transfer codecs are unchanged. No new dependencies or license changes.

Controlled-clock tests cover elapsed and restart recovery, overlapping server
waits, ordinary network errors, cancellation and no request bursts. Operation
counts retain unchanged-channel restart/reconnect and direct-push guarantees.
Manifest tests cover unchanged cold restart without downloading, authenticated
cache use, corruption repair, missing keys, account/channel isolation,
same-metadata edits, gaps, deletes and stale writes. Storage validates the actual
point-lookup query plan and schema-27 rollback/restart. GUI tests cover live
readiness and distinct waits at 900×600 and native full-screen flags; native
English/light preview checks and release gates are recorded separately.
