# ADR 0018: Local-first Channel synchronization

Status: Accepted, 2026-09-08. Selection-only priority, polling and the private-projection exclusion are superseded by [ADR 0019](0019-event-driven-channel-projections.md).

## Context

Selecting a Channel previously cleared its view, read a cache and then scanned from the newest remote message. Navigation therefore owned network freshness, repeated reads and loading feedback. Cached history could not independently follow edits/deletions or recover an interrupted incremental update.

## Decision

Selection only changes synchronization priority. A retained Runtime owner per authorized account compares committed local PTS with source metadata and consumes coalesced Telegram push hints. GUI always queries the local projection and retains warm rows; it never owns a Channel network scan. Cold initialization, an observed mismatch/gap, explicit refresh/retry, and an explicit older-history request are the data-sync triggers. A periodic dialog metadata check detects silent passive subscriptions without rescanning clean history.

Telegram owns grammers/session access and official `updates.getChannelDifference`, history and exact-ID verification. Runtime owns scheduling, cancellation, retry/fairness and typed phase events. Storage owns SQL and an atomic compare-and-swap commit for files, deletions and independently versioned synchronization state. Core, Index, Crypto, Transfer and all adapters remain GPUI-free. The existing generic policy/range Index coordinator is not rebranded as this production path.

Schema 11 adds account/chat PTS, exclusive older-history cursor, independent gap-history and known-ID repair cursors, and deletion tombstones. No existing cursor or encrypted bytes acquire a new meaning. Initial seeding reads one bounded recent page using a previously captured dialog PTS, then catches up differences and validates legacy cached IDs. `TooLong` retains the local projection, backfills the new-message interval and verifies known IDs in bounded restartable pages. A missing integer message ID is not itself evidence of a gap. Deletions preserve local outputs and transfer history; stale history cannot resurrect tombstoned documents. Authoritative PTS updates handle edits within one timestamp second.

The owner bounds sources at 10,000, commands at 64, difference pages at 100, history pages at 200 and exact-ID verification at 100. Selected-source priority yields to other ready work after three turns. Network/persistence failures retry at 1/2/4 seconds before pausing. Structured account-wide FloodWait deadlines survive manual retries. Cancellation and account changes reject uncommitted responses; the worker handle is retained/reaped without a UI-thread join. GUI limits its warm cache to eight views/16 MiB and displays at most 5,000 virtualized rows.

Feedback is independent of diagnostic logging and navigation: preparation/local read, receiving/history, verification, persistence, retry/rate wait and terminal outcomes carry a phase duration and last activity. A global status opens an independent 128-event inspector; dropped events and hint overflow are disclosed. Unknown rates, percentages and ETA remain unknown. Startup paints before database work, runs automatic schema upgrades in background workers and shows a bounded migration timeline with retry guidance. The additive schema-11 transaction preserves recoverable originals and checks structure/foreign keys before its version marker commits.

## References and license provenance

- [Telegram update synchronization](https://core.telegram.org/api/updates): PTS ordering, channel differences and server-declared history gaps. This is a protocol reference, not implementation code.
- Permissively licensed SDK documentation informed the separation of account-scoped caches and synchronization lifetimes from the visible timeline. Its license was verified before review; no implementation was copied.
- The installed public grammers 0.10.0 APIs are MIT OR Apache-2.0, verified in package manifests before inspection. No dependency was added. No license-incompatible source, tests, architecture or summaries were consulted.

## Consequences and evidence

Repeated clean selection and table scrolling require no network history request; a warm view is immediately reusable. Durable state supports replay after failed commits, cancellation and restart, while callbacks stay scoped to their account and source. This covers the document projection for channels/supergroups, not private/common-chat synchronization or generic full-history range evidence.

Deterministic fixtures exercise selection/duplicate hints, blocked remote work with concurrent local reads, cancellation, rate deadlines, atomic/CAS rollback, same-second edits, tombstones, scoped data, initial/older-history separation and `TooLong` backfill restart. Migration fixtures exercise all supported source schemas, retained data/key-setting bytes, failed DDL and disk-full rollback/retry. GPUI tests exercise local-only selection and stale callbacks, compact localized controls and timeline scrolling. Real-account delivery/latency, long server outages and full native accessibility/backing-scale qualification remain separate evidence requirements.
