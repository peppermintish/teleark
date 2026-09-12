# ADR 0032: Event-driven status presentation

Status: Accepted, 2026-09-13. Refines the presentation described in ADR 0019; transport scheduling, persistence and synchronization protocols do not change.

## Decision

Business notifications update the synchronization inspector. Its presentation clock never notifies the application entity or reads Runtime/Storage. The clock exists only while details are open and the native window is active; closing details releases its entity, task and subscriptions. Window deactivation cancels the timer. Reactivation immediately recalculates visible values and resumes timing only for active work. Completion stops timing on its event. The asynchronous loop retains weak ownership across waits.

Only the bounded live summary is reformatted once per second. Elapsed values use whole seconds; activity ages older than a minute use minute buckets; retry deadlines use ceiling seconds and saturate at zero. A notification occurs only when displayed strings change. Terminal phase/scan durations are frozen at the last recorded activity. History uses fixed local dates/times derived from a stable monotonic-to-wall-clock anchor; elapsed wall time cannot change an existing row.

History formatting is event-driven and uses a channel-name map, avoiding a full channel search for each retained event. It retains the existing bounded event history and materializes only visible rows in its own scroll viewport. Long source/error text is available through the row tooltip. The event truncation/overflow summary remains visible outside that viewport.

The main page and channel-source list have separate retained GPUI cache boundaries from the header and inspector. Search-input presentation can invalidate itself after paint; its invalidation must not reconstruct file/task/source collections. Business notifications, layout changes and theme changes invalidate the relevant presentation. Palette helpers are GUI-thread-local so independent application/test threads cannot invalidate each other's styles.

The locked GPUI version does not replay accessibility nodes when reusing a cached subtree. When accessibility is active, the frontend uses normal element rendering instead of those visual caches, preserving discoverable controls. In that compatibility path the shell/page may be traversed on a visible clock tick, but history strings are still event-driven, history rows remain virtualized and there is no app-level timer notification. This is an explicit limitation of the cache optimization, not a claim of zero full-window paint work in every environment.

## Evidence and scope

Deterministic tests assert that a visible timing change sends no app notification and reuses the main page and history when visual caches are available; 128 history entries materialize only visible rows. They cover inactive-window timer cancellation, immediate resume, completion/new-work transitions, clock release after closing, second quantization and frozen terminal durations. Existing GUI geometry and interaction tests cover cached content resizing and primary actions.

Native synthetic English-only checks cover 900×600 and actual full-screen mode in light and dark themes, independently scrolling history, fixed timestamps, reopening, and accessibility controls retained across updates. A short process-CPU sample with accessibility enabled is qualification of that synthetic fixture only, not a real-account benchmark or a measured before/after speedup. No schema, encrypted format, persistent setting, dependency or license changes are introduced.
