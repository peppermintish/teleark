# GUI interaction audit — 2026-09-26

This review treats TeleArk as a desktop application: an action should acknowledge
input, preserve context, remain controllable while work runs, and lead to the
expected result. All fixtures are synthetic. Interface checks use English and
light mode; no personal Telegram credentials, sessions or files are required.

## Corrected interactions

- Settings appearance tiles and notification/download switches accept Enter and
  Space. Visible login forms accept Enter, preserve drafts while requests run,
  and do not submit hidden forms or composing text. Code/password verification
  describes the operation in progress.
- Escape cancels account-switch and credential-storage confirmations without
  applying them or leaving fullscreen. Open dialogs keep navigation shortcuts
  from moving the workspace or focusing a search input behind the dialog. Find
  focuses search only on pages that actually render it.
- Proxy failures remain visible even if a retained network snapshot still says
  the previous route was connected.
- Storage keeps Upload reachable while a channel key is unavailable. Search
  distinguishes no matches from an empty channel. Files/Raw Files and Channels
  navigation clear unrelated selection and details. Upload tooltips distinguish
  the full source path from the Remove action.
- A channel download callback verifies its account/channel scope and clears only
  the files it submitted. New selections survive earlier work completing.
- Download again leaves the Completed filter and selects the new transfer only
  if its account generation is still current. Individual transfer actions retain
  acknowledgment when Manage is collapsed.
- Vault bulk actions dispatch the selected Vault tasks through a retained queue,
  with at most eight actions running and the existing shared 64-job ceiling.
  Queued/running counts, elapsed time and cancellation stay visible during
  navigation. Cancel remaining actions preserves already admitted transfer
  owners and their pause/stop controls. Pause/Stop can replace queued resumes;
  revocation and Pause and Quit discard pending actions before draining owners.
- Managed files accept independent download requests instead of silently ignoring
  every click while one download runs. Sixteen retained requests bound admission;
  account/channel/file identity prevents duplicates. Capacity, elapsed time and
  a route to Transfers remain visible. Completion, failure and cancellation free
  capacity, and stale callbacks cannot remove a newer request.

## Evidence and boundaries

The final macOS source run passed all **210 GUI tests**, with three existing
manual comparative benchmarks ignored. This adds 21 GUI regressions to the prior
189 passing tests. The checks exercise actual GPUI events/focus, compact geometry,
fullscreen state, blocked operations, independent completion, cancellation,
capacity limits, account changes and shutdown barriers. Strict Clippy passed for
GUI and i18n with all targets and warnings denied. All 26 non-visual i18n tests
passed, including complete-catalog parity and English fallback. The final native
executable built successfully; formatting and whitespace checks passed. These are
the scoped GUI/message gates, not a new full release qualification. The existing
upstream `block 0.1.6` future-compatibility warning remains unchanged.

A GPUI test that toggles fullscreen exercises the framework's fullscreen state;
it does not substitute for a native desktop review. Desktop inspection repeatedly
timed out (`-10005: timeoutReached`) when attaching to isolated preview apps,
including a temporary native preview entry point. The installed application was
not used to test account operations. No native review acceptance screenshots or
900×600/fullscreen interaction passes are claimed. The temporary entry point and
Cargo target were removed; they are not product changes.

The native review remains blocked pending a working attachment to the synthetic
preview. Windows/Linux interaction and live Telegram transfers were not exercised.
The next release must wait for native English/light review at 900×600 and actual
fullscreen, including keyboard traversal, menu/shortcut behavior, file-picker
cancellation, inspector scrolling, transfer controls and failure recovery. A
passing synthetic interaction does not establish these platform or network results.

No persistent schema, encrypted format, dependency or application version changed.
The fixes are recorded under Unreleased; there is no new release tag or publication.
