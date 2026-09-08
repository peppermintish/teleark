# Performance audit - September 2026

This audit uses isolated fake remotes, temporary databases and synthetic histories. Each verified fix receives a `perf/20260908-NN-*` annotated checkpoint tag. Tags point to complete source snapshots with a per-fix parent; the user's current branch, index and unrelated uncommitted work remain intact. These are source checkpoints, not release tags.

## 01 - Native download part-event overhead

**Cause.** Every part event shifted a bounded `Vec`, rebuilt a `BTreeMap` from up to 8,192 events, scanned it repeatedly and cloned the full task snapshot. Progress callbacks also cloned detailed history even when persistence was not due. This work shares locks with the interface's snapshot readers.

**Change.** `PartEventHistory` uses a ring plus incremental latest-state counters. Hot callbacks return only the fields their caller needs; progress copies a persistence snapshot only when the checkpoint is due. The replay remains bounded and shows its omitted event count. Total completed/missing parts use completed byte progress, so replay eviction cannot make total completion go backwards.

**Evidence.** Deterministic fixtures compare incremental retained-state counts against the previous reconstruction through three full ring lengths, including repeated IDs, retries and failures. Existing worker completion, pause/cancel/retry and persistence tests run against the changed path. A manual comparison preserves the old algorithm in test-only code:

```bash
cargo test -p teleark-runtime --locked perf_native_part_history -- --ignored --nocapture
```

On this host's unoptimized test build, a full 8,192-event replay followed by 1,000 updates took 4,702,649 microseconds in the reference path and 1,403 microseconds in the incremental path. These are local algorithm timings, not end-to-end transfer rates or UI latency; the manual timing has no flaky wall-time pass threshold. Logs are under `target/performance-audit/`.

## 02 - Transfer snapshot copies, idle polling and queue refill

**Cause.** The GUI polled every 250 ms, copied every native/Vault snapshot (including replay and telemetry), formatted debug strings to detect differences, then copied snapshots again while rendering lists, rates and details. Queue admission also copied unrelated histories; a full native queue depended on that GUI poll to admit later durable tasks.

**Change.** A frontend-neutral transfer store keeps immutable shared views and copies only changed records. Per-record lookup uses task IDs. A retained publisher sleeps while idle, coalesces byte/part samples over 100 ms, flushes the trailing sample and publishes phase/terminal changes immediately. GUI owners subscribe before their baseline, retrieve views in background tasks, and render cached data without runtime queries. The one-second duration clock runs only with active work. Native queue capacity is refilled by its retained worker and account activation; candidate selection is bounded to available slots. Admission checks build a path set instead of cloning telemetry and comparing every request with every task.

**Evidence.** A 10,000-record fixture performs 1,000 idle reads with zero additional record copies, then changes one record and observes exactly one copy; old views remain immutable. Signal tests cover idle waiting, burst coalescing, trailing samples, terminal publication and owner closure. A blocked fake backend holds the first item of a 67-task batch until the queue is full; all tasks subsequently complete without a frontend poll or subscription. GUI coverage checks cached rendering when the runtime is unavailable and filters another account's records.

```bash
cargo test -p teleark-runtime --locked perf_transfer_snapshot_views -- --ignored --nocapture
```

On the same unoptimized build, 128 tasks with 512 events each and 100 reads took 855,718 microseconds with full copies, 6 microseconds with an unchanged view, and 7,990 microseconds when one task changed before every read. These isolated costs exclude rendering, network and disk latency.

## 03 - Formatting invisible transfer rows

**Cause.** List widgets virtualized drawing, but the frontend formatted every task before supplying those widgets. Hidden batch members were also formatted, and Vault batch status aggregation formatted each member just to obtain its state. Mouse interaction and progress updates therefore repeated work proportional to all retained tasks.

**Change.** Cached lightweight identities drive grouping, counters, filters, selection and scrolling. Rows are formatted when the list requests a visible item, and batch inspectors use the same approach. Batch state is derived directly from typed state. Search preserves localized source/destination labels and uses a source lookup built once per query. Account and snapshot identity invalidate the lightweight cache; locale-dependent text is produced when shown.

**Evidence.** A real GPUI 900x600 viewport with 10,000 synthetic tasks formats fewer than 200 rows across its layout passes instead of formatting the entire history. Tests compare lazy filters and bulk-action scope with the displayed-row reference in all three locales, including collapsed groups, Unicode searches, status filters and hidden selection. Existing compact layouts and batch activity tests pass.

```bash
cargo test -p teleark-gui --locked perf_transfer_virtualization -- --ignored --nocapture
```

The final manual run prepared 10,000 tasks for ten list updates: full formatting took 980,467 microseconds, while cached identities and twenty visible rows per update took 9,755 microseconds. This measures list preparation, not GPU frame rate. GUI tests (73), strict GUI Clippy, i18n tests and formatting checks pass.

## 04 - Eager channel sidebar menu allocation

**Cause.** GPUI Kit already virtualizes sidebar drawing, but the app created a title allocation, menu object and listener for every channel before handing the children to that widget. Every repaint repeated those allocations.

**Change.** Sidebar children retain lightweight source identities and a weak owner. The existing GPUI Kit sidebar creates menus only when it asks to render a visible item. Identity checks reject an item invalidated by a source refresh; clicks retain channel selection behavior. The standard header, footer, resizing and scrollbar remain in use.

**Evidence.** A 900x600 GPUI fixture with 10,000 sources creates fewer than 200 menus over layout passes, excludes the private storage channel, selects a channel through an actual click, scrolls while retaining selection and keeps the footer reachable. A stale source index is rejected. The full GUI suite and strict GUI Clippy pass. Existing compact sidebar checks cover three locales and both themes.

```bash
cargo test -p teleark-gui --locked perf_channel_sidebar_construction -- --ignored --nocapture
```

For 100 preparations of a 10,000-channel list, the previous eager construction took 1,115,108 microseconds and lightweight identities took 150,145 microseconds in the unoptimized test build. Visible menu rendering is excluded from both preparation timings; this is not an end-to-end frame-rate result.

## 05 - Repeated channel filter evaluation during repaint

**Cause.** Expanded batch controls independently filtered and allocated the complete file list three times per repaint, including filename classification. Select-all evaluated the time-sensitive filter again, which could disagree with the already displayed table near a time boundary. Selection itself already used a set; there was no quadratic vector-membership defect.

**Change.** Counters, action availability and select-all reuse the displayed table projection. Source updates and filter changes still rebuild that projection explicitly.

**Evidence.** A deterministic 5,000-file fixture checks all files, one file kind and no matches, collapsed/expanded controls and selection. An instrumented filter records zero evaluations for those render and select-all operations after projection refresh (the previous expanded controls evaluated 15,000 files per render). A boundary fixture verifies that selection follows the displayed IDs until the projection refreshes. GUI tests pass 75 cases and strict GUI Clippy passes. No new timing estimate is asserted for this small change.

## 06 - Unused filesystem scans, unchanged repainting and local-output lookup

**Cause.** A retained GUI task recursively measured up to one million application-directory entries every 15 seconds, although no screen read its result. Local-output probes notified the whole app every three seconds even when nothing changed, and each row searched all retained output records to find its local copy. Background loops also held a strong app reference across timers, preventing a closed window from releasing its owner.

**Change.** The unused recursive task and cache are removed; the runtime measurement API remains available for an explicit consumer. The cheap five-second free-space query notifies only on a changed result. Local observations update only changed records, with incremental source/candidate/age indexes bounded to 10,000 outputs. Candidate ranking preserves present-copy priority, latest completion and deterministic path ties. An evicted old observation cannot trigger a repaint when immediately discarded again. Visible output views and active transfers retain the three-second probe interval; idle Account/Settings pages use thirty seconds. Page checks are cheap and returning to a visible view resumes probing within the three-second tick. Initial history no longer queries the same page twice. Account/login generations reject stale results, and timers release strong app references before waiting.

**Evidence.** Deterministic tests compare indexed selection with the prior scan through source replacement, missing/present states and failure, validate all three retention indexes over 10,100 outputs, and cover hidden/visible/active cadence. A real temporary catalog plus controlled GPUI clock verifies that unchanged probes cause no notification and a closed window releases its app while both background owners are waiting. The full GUI suite passes 79 tests and strict GUI Clippy passes.

```bash
cargo test -p teleark-gui --locked perf_local_source_lookup -- --ignored --nocapture
```

One thousand source lookups over 10,000 output records took 545,779 microseconds with full scans and 1,860 microseconds with the index in the unoptimized test build. This excludes filesystem latency. External filesystem changes still require bounded observations: older inventory rotates in pages, and the presentation cache is not a complete durable inventory.

## Audit boundaries

Static inspection and controlled workloads can establish specific regressions and their fixes; they cannot prove that no performance problem exists on every device or network. Credentialed cross-device Telegram timing is not part of these isolated tests. The closing review and final validation are recorded below.

## 07 - Telegram request head-of-line blocking

**Cause.** One command loop awaited whole native downloads, encrypted-part transfers and long scans before accepting the next command. A healthy network connection could therefore still leave unrelated channel reads or account images waiting for a large file to finish.

**Change.** The retained worker now has bounded independent read (8), transfer (4) and mutable control (1) lanes, plus 32 pending requests. Immutable connection/chat snapshots avoid copying the entire roster. Reads continue while a transfer or metadata discovery waits. Login/logout cancels and drains older work before publishing new session state; the existing storage creation guard survives cancellation inside its sole control owner. Full queues return a typed capacity error while keeping session-control admission available. [ADR 0021](adr/0021-bounded-telegram-request-lanes.md) documents ownership and limits.

**Evidence.** Tests run the actual generic dispatcher with controlled futures: four blocked transfers and one blocked metadata command do not block a read; saturated reads reject excess requests but admit a login barrier; old replies close, new reads wait for the replacement state, and closing the owner drains all lanes. No live Telegram throughput or latency improvement is inferred from these scheduling tests. The full workspace gates pass for this ownership change (396 tests, five manual benchmarks ignored by normal tests; formatting, all-target check/Clippy, Core/i18n, rustdoc and dependency policy also pass); results are retained in `target/performance-audit/07-full-*.log`.

## 08 - Repeated account identity RPCs

**Cause.** Every scan/page, channel read and transfer object requested `current_account()` before its useful RPC. That adapter operation performs authorization and `get_me` calls, adding repeated network dependency to one unchanged authenticated session.

**Change.** Runtime routing checks use the account ID established by a successful connect, QR, code or password result. A session barrier clears that ID before any authentication change, and pending/failed login results retain no authorized ID. Shared read snapshots carry only that session's identity. Actual Telegram RPC authorization and the adapter's fresh private-storage metadata/account-bound identity checks remain intact; this cache cannot authorize a remote write.

**Evidence.** A deterministic fixture performs 10,000 account checks without a connection or async runtime, rejects a different account, checks snapshot identity and clears the old identity for every pending/failed auth state before accepting a replacement account. Runtime tests (99) and strict Clippy pass in `08-final-tests.log` and `08-final-clippy.log`. The first full run exposed an existing test-fixture race: terminal presentation preceded the worker’s final database write, so a restore fixture could be overwritten. The fixture now waits for the worker to release task ownership before replacing stored data. This removes a specific preliminary lookup, with no guessed network latency or bandwidth figure.

## 09 - Repeated download bitmap scans

**Cause.** After each 1 MiB completion, the adapter traversed a byte-per-flag vector to compute total bytes and completion, then traversed it again to pack the persistent bitmap. This made one file's repeated accounting grow quadratically with its part count.

**Change.** The in-memory map now stores the same packed bits as the existing `TARKDPM1` version-1 codec, with incremental byte and part counters. Duplicate completion is idempotent. Encoding copies the packed payload; startup decoding derives the counters once. The existing per-part persistence order is unchanged by this optimization.

**Evidence.** A fixed binary v1 fixture and an independent legacy encoder compare bytes and counters for empty, exact-part, short-tail and 4,097-part files, out-of-order completion, duplicate callbacks and ignored unused bits. Telegram tests pass 37 cases, with one manual benchmark ignored; strict Clippy passes. For 10,000 parts and 1,000 completions, the unoptimized comparison took 119,607 microseconds for the legacy reference and 272 microseconds for packed accounting/encoding. Flag storage fell from 10,000 to 1,250 bytes. This excludes filesystem and network time. Command: `cargo test -p teleark-telegram --locked perf_download_part_map -- --ignored --nocapture`.

## 10 - Library keyset pages repeatedly sort the catalog

**Cause.** Search orders by modified time with a created-time/null fallback, while the original indexes contain raw modified time. SQLite therefore scanned rows and built a temporary sort tree for each page. Adding an expression index alone still scanned all preceding keys with a tuple cursor; an explicit scalar upper bound is also needed to seek a deep page.

**Change.** Schema 13 automatically adds matching global/account/channel/kind indexes. The cursor predicate retains the same ordering and codec, with a redundant scalar bound that gives SQLite an index range seek. Earlier raw-time indexes remain available for explicit timestamp facets. Index construction uses the existing visible background migration transaction, validates integrity and preserves original data on failure.

**Evidence.** Actual SQLite query-plan assertions require an index `SEARCH` without a temporary sort for all four scopes. Pagination compares all 1,003 synthetic records against the reference order, including timestamp ties and null fallback. Upgrade tests cover every schema 1–12, failed index DDL, disk-full rollback, wrapped-key/generation and setting preservation, restart and reopen. The full workspace gates pass for this persistent-schema change: 400 normal tests, seven manual benchmarks ignored, formatting, all-target check/Clippy, Core/i18n, rustdoc and dependency policy.

The production `search_files` API on bundled SQLite 3.53.2 read 100 deep pages in a 100,000-row temporary catalog: 8,901,904 microseconds without the matching indexes and 198,826 microseconds with them, in the unoptimized test build. Each call still computes an exact count; this comparison does not claim that count cost is fixed. Command: `cargo test -p teleark-storage --locked perf_library_search_indexes -- --ignored --nocapture`. A separate query-plan probe under `target/performance-audit/10-profile.log` isolates the sort/seek cost; use the production-API numbers above for the audit comparison.

### 11 — unfiltered search counting (2026-09-08)

- Root cause: `COUNT(*) FROM logical_files f WHERE 1 = 1` disables SQLite's dedicated count operation even with no user predicates. The search now removes only that trailing constant clause; filtered and FTS counts remain exact queries. No count cache or schema/codec changes were introduced.
- Manual debug-build benchmark, bundled SQLite 3.53.2, 100 counts over 100,000 temporary rows: constant-WHERE reference **164,476 µs**, bare exact COUNT **6,365 µs**. This isolates counting, not the full search API or UI latency. SQLite still reads B-tree page metadata; this is not a constant-time cached total.
- Deterministic coverage verifies empty/nonempty totals, kind filters, same-connection deletes, external committed inserts and external rollback. Storage tests **38 passed, 2 manual benchmarks ignored**; strict package Clippy passed. Logs: `target/performance-audit/11-final-{tests,clippy,benchmark}.log`. An initial benchmark fixture used unsupported `u64` SQL decoding; corrected to `i64` before final checks.
- Tag: `perf/20260908-11-exact-count-fast-path`.

### 12 — nonblocking sampled download checkpoints (2026-09-08)

- Root cause: the native download's synchronous observer runs inside the Telegram async reactor. Every 750 ms it called the Storage actor and waited for an acknowledged SQLite write. A slow/busy database therefore blocked unrelated network futures on that reactor.
- Change: sampled progress uses `try_send` into the existing bounded Storage actor. A full/disconnected queue records a sanitized deferred-checkpoint diagnostic; current UI progress and the part-map recovery file remain available, and a later sample can persist. Lifecycle transitions and the final result still use acknowledged writes. SQL updates only bytes/duration/time for the matching running account and attempt, with monotonic byte/time guards: a late sample cannot resurrect cancellation, completion, pause or an older retry. No schema or codec changes.
- Evidence: a controlled receiver remains blocked while 1,000 submissions return (one accepted at capacity one); SQL tests cover older progress, wrong account, all retired states and a replacement attempt. Runtime **100** and Storage **39** normal tests pass. Full workspace gates pass (**403 normal tests**, eight manual benchmarks ignored): fmt, all-target check/strict Clippy/tests, Core/i18n, rustdoc and dependency policy. Logs: `target/performance-audit/12-full-*.log`. No synthetic network speed multiplier is claimed.
- Tag: `perf/20260908-12-nonblocking-checkpoints`.

### 13 — bounded background session logging (2026-09-08)

- Root cause: session logs flushed a `BufWriter<File>` synchronously for every part/controller event. Native callbacks execute on the Telegram async reactor, so slow log storage could stall otherwise independent requests. Initial log creation failure also incorrectly failed file transfer.
- Change: one retained process-owned writer handles complete JSONL records; callbacks only stage/submit bounded data. Limits are 256 queued records, 1 MiB queued bytes, 64 KiB per record. A single writer preserves retry order; opened append handles prevent queued writes from recreating deleted log paths. Logging failures remain diagnostic. Explicit aggregate `all_transfers` gap records, plus localized omission counts in Settings/transfer details, disclose loss. Live task states and recovery persistence remain independent. Abrupt exit can lose the best-effort diagnostic tail.
- Evidence: controlled blocking-disk tests submit/flush/drop 1,001 later records without waiting, assert both queue bounds, parse whole JSONL records/gaps/final events, discard oversized records, and continue on another log after a storage-full failure. A fake-remote download completes with correct output despite a colliding regular file blocking the log directory. Existing privacy/mode/format tests pass. Runtime **104** tests, GUI/i18n tests and strict Clippy pass; full workspace gates pass (**407 normal tests**, eight manual benchmarks ignored), including dependency policy and rustdoc. Logs: `target/performance-audit/13-full-*.log`. Initial test builds exposed a missing direct test-only JSON parser dependency and an unnamed lock binding; corrected before final gates. The existing MIT/Apache-2.0 `serde_json` version is reused only by Runtime tests and notices/lockfile are synchronized.
- Prevention: ADR 0022 explicitly supersedes ADR 0009's synchronous required log writes; unchanged schema-1 records retain their meanings and the new gap event is additive. Contributor rules now capture recurring projection/owner/memory/query-plan mistakes; local count/bitmap/index details remain protected by their specific tests and this evidence record.
- Tag: `perf/20260908-13-background-session-logs`.

### 14 — nonblocking owner destruction (2026-09-08)

- Root cause: dropping the Storage/Telegram facades synchronously waited for queue capacity and then joined their worker threads. Dropping native transfers additionally copied a full replay snapshot and waited for SQLite. A window/account resource release could therefore wait for a busy disk or blocked network owner.
- Change: destructors send only nonblocking shutdown hints and join only already-finished threads. Closing the final sender lets the owned worker drain/retire; its connection, requests and network state outlive the facade safely. Native shutdown submits a minimal guarded progress sample without copying replay history or waiting for persistence; the previous checkpoint/part map remains available on saturation, and the transfer owner performs acknowledged retirement when control returns. Vault already used the nonblocking pattern and was retained.
- Evidence: controlled tests keep Storage and Telegram queues full and owners blocked while the dropping thread returns, then release and verify worker retirement. The native test blocks both backend and Drop-path storage. Existing restart coverage still restores the latest 7-byte checkpoint and completes the retry. Runtime **106** normal tests and strict Clippy pass. Full workspace gates pass (**409 normal tests**, eight manual benchmarks ignored), with logs in `target/performance-audit/14-full-*.log`. These tests establish absence of cross-owner waits, not an operating-system shutdown-time promise.
- Prevention: the latency-sensitive destructor rule is in `AGENTS.md`; the underlying request ownership remains covered by ADR 0021. No new durable format or independent architecture is introduced.
- Tag: `perf/20260908-14-nonblocking-owner-drop`.

### 15 — filesystem waits outside shared service actors (2026-09-08)

- Root cause: the shared Storage actor created managed directories for preference updates/layout queries and tested candidate download paths. An unavailable mount therefore blocked unrelated SQL work. Native Telegram destination validation also called synchronous filesystem metadata on its async reactor.
- Change: existing background Runtime callers prepare directories and inspect candidate paths; the Storage actor handles only preference persistence and reservation SQL. Settings persist only after successful filesystem preparation. Name validation, bounded collision search and historical reservations remain unchanged. Telegram destination validation now awaits Tokio's filesystem API.
- Evidence: a controlled filesystem-preparation closure stays blocked while the real Storage owner completes an unrelated settings write/read and Library search. Failed directory preparation preserves settings and the colliding original file. Existing no-overwrite, deleted-output/history-reservation, private-partial and publication tests pass. Runtime **108** normal tests and strict Clippy pass; full workspace gates pass (**411 normal tests**, eight manual benchmarks ignored) in `target/performance-audit/15-full-*.log`.
- Review result: local-file metadata inspection was already outside the SQL actor; its import path did not need this change. Filesystem access still takes real device time on the requesting background owner; no network or disk throughput improvement is invented.
- Prevention: this applies the service-ownership rule in `AGENTS.md` and Architecture; no new persistent format or separate ADR is needed.
- Tag: `perf/20260908-15-filesystem-owner-isolation`.

### 16 — session-wide history and recovery budgets (2026-09-08)

- Root causes: per-task replay limits multiplied across an unbounded resident task history, while startup's newest-10,000 query could silently omit older queued/paused/failed work. Each task's lifecycle event vector also grew with repeated control actions.
- Change: a shared atomic admission budget normally retains at most 10,000 native records, evicting only old completed/cancelled resident groups and protecting recoverable/scheduled batches. Database restoration first retains every recoverable task and its batch members, including an oversized legacy backlog, and fills remaining capacity with recent history. Database records, logs and files survive in-memory eviction; cold historical redownload still loads one record and validates its account. Full replay remains for bounded scheduled/running work plus 16 recent idle tasks; older idle tasks keep 20 part events/32 decisions. Lifecycle history keeps the latest 256 events. The UI discloses omitted tasks, controller decisions, part events and lifecycle events.
- Evidence: tests preserve 10,020 legacy recoverable rows and old recovery/batch membership; 16 simultaneous admissions cannot exceed a five-slot test budget; failed inserts release reservations; protected/retired groups behave correctly. A real fake-backend flow verifies database/file preservation, cold redownload and foreign-account rejection. A 64-task replay fixture retains **148,376** part events after compaction versus **524,288** before, including two protected tasks; these are event counts, not process RSS. Compact histories can resume appending with correct incremental counters, and the latest lifecycle outcome survives 1,000 transitions.
- Runtime **113** and Storage **40** normal tests pass; full workspace gates pass (**417 normal tests**, eight manual benchmarks ignored) in `target/performance-audit/16-full-*.log`. Initial test compilation used iterators where the store constructor requires `Vec`; corrected before verification. No schema/codec change. Legacy over-budget recoverable work is a deliberate compatibility exception; broader log-file disk retention and complete cold replay remain documented limits.
- Prevention: ADR 0023 and the global-memory rule in `AGENTS.md` capture the recurring ownership/retention mistake.
- Tag: `perf/20260908-16-history-recovery-budgets`.


### 17 — retirement without a global scheduling wait (2026-09-08)

- Root cause: native retirement and retry held the shared scheduled-task mutex across acknowledged SQLite writes and failed-partial cleanup. A slow owner therefore blocked unrelated admission and scheduling inspection.
- Change: serialize only the in-memory publication and retry decision. Retain the task identity in the scheduled set while persistence/cleanup runs outside the lock. A retry requested during old ownership becomes visible as queued immediately; the retained worker persists and starts it after old writes/cleanup finish. An unscheduled retry reserves its identity while acknowledging its queued checkpoint, preventing an automatic refill from starting it early. No second owner can open the same partial. No schema or codec changes.
- Evidence: the production failure path is held inside a controlled cleanup callback while retry and another task's admission return, with exactly one backend attempt before release. Afterwards both tasks complete and the stored original task is completed. A separately withheld Storage reply blocks terminal persistence while the global scheduling lock remains available. Existing immediate cancel/retry, pause/resume and restore tests pass. Full final gates pass: **419 normal tests**, ten manual probes ignored by normal runs, fmt, all-target check/strict Clippy, Core/i18n, rustdoc and dependency policy; logs: `target/performance-audit/final-full-*.log`.
- Recovery boundary: requesting a retry while its old owner is still retiring does not promise a new durable queued state until that owner finishes. Abrupt process loss in that interval retains the previous durable recoverable/terminal state and its existing recovery data; an interrupted cancelled retry may require retry again after restart. No partial is opened by two attempts. This is recorded explicitly rather than representing a queued request as successful transfer completion.
- Prevention: existing contributor ownership rules apply; the native retirement corollary is documented with ADR 0021 rather than creating another architecture decision for the same scheduling principle.
- Tag: `perf/20260908-17-nonblocking-retirement`.

## Closing probes and scope

Two remaining source suspects were measured without changing their algorithms. On this host's unoptimized test build, 10,000 controller snapshots with the full bounded 2,048 decisions took **311,885 µs** (about 31 µs per copy). A 64 MiB temporary-file write with a flush per 1 MiB and final `sync_all` took **77,327 µs** without per-part maps and **77,616 µs** with the production atomic map replacements. This single ordered local-disk probe is affected by cache/device noise and does not establish a repeatable disk bottleneck or a cross-device rate. The existing checkpoint order remains intact; this audit does not introduce checkpoint batching or redefine old resume bytes.

Reproduce with `cargo test -p teleark-transfer --locked perf_bounded_controller_snapshot -- --ignored --nocapture` and `cargo test -p teleark-telegram --locked perf_download_checkpoint_disk -- --ignored --nocapture`. These probes have no timing pass threshold. All eight earlier comparisons and their logs remain available. This audit closes the 17 reproduced findings; it does not claim that every possible device/network workload has been profiled. Actual credentialed Telegram end-to-end timing, million-record catalogs, long-term diagnostic log disk retention and full cold historical replay remain outside this evidence.

## 18. Catalog RPC failure incorrectly invalidated the whole workspace

**Observed incident (2026-09-08):** entering the workspace after successful login displayed the English network error across unrelated views. Sanitized read-only diagnosis showed that connection/authentication and avatar reads succeeded, while `messages.getDialogs` returned RPC 500 `RPC_CALL_FAIL` (constructor `0xa0f4cb4f`). A direct public grammers client reproduced the same response outside the runtime dispatcher. Changing page sizes did not produce a complete roster: one-item pagination returned 14 unique peers before failing, against a reported total of 116. Partial results are not a successful discovery and must never authorize private-channel creation.

**Changes:** Telegram RPC codes >=500 map to the transient `Server` category, separate from transport and authentication failures. GUI catalog loading has its own retained task, cancellation token, monotonic request generation, phases and bounded current-run timeline; it no longer writes the global authentication activity. The shell status opens a scrollable inspector from any route, with phase/last-activity duration, sanitized failure reason, current attempt and cancel/retry controls. Catalog reads have a 20-second execution deadline and at most three attempts with 2/4-second waits. Cancellation, account switching and old callbacks cannot publish stale results. Initial storage management waits for successful catalog loading; navigation after a failed/cancelled run does not restart an automatic retry cycle. Failed refreshes keep the previous in-memory roster and existing local library bytes; there is no new cross-launch roster restoration or claim of fresh remote coverage. Transfers are activated only after complete results have been saved.

**Compatibility:** no database, session, encryption, recovery or native-download codec version changes. The new category is transient; native-download persistence still writes server failures through the historical `network` category so old readers retain their established behavior. Download retry eligibility, upload ambiguous-success handling and batch stop policy continue to include server failures. No new dependencies.

**Evidence:** deterministic adapter tests cover RPC 500/503 versus 401 and actual I/O timeout; runtime tests cover cancelled catalog requests without a connection or roster replacement; GPUI tests cover preserved login/data, wrong-account and cancelled callbacks, same-account relogin generations, and actual inspector/cancel clicks at 900×600 in all three locales. The post-fix live runtime probe recorded authorized connection (2,779 ms), catalog failure `Server` (869 ms), then successful independent read (669 ms). It made no remote writes and did not call storage creation, sign-out or account reset. This is one read-only incident reproduction, not protected end-to-end account qualification.

**Remaining external failure:** the live Telegram server still rejected full catalog reads at the final probe. The app can accurately show and retry this failure while local pages remain usable, but live channel enumeration/private storage cannot be declared restored. Sanitized evidence: `target/performance-audit/18-live-catalog.log` and `18-*-probe.log`; final source gates and release build have separate `18-final-*` and `18-release-build.log` records.
