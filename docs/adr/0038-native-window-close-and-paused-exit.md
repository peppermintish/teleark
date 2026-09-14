# ADR 0038: Native window close and paused exit

Date: 2026-09-14. Status: accepted.

## Decision

This supersedes the wait-for-completion policy for Quit/Close in ADR 0037.
Closing the main window or using the application Quit command requests exit.
Active transfers require **Pause and Quit** confirmation; cancelling the request
leaves all work running. Idle exit uses the same checkpoint barrier without an
extra confirmation. Preview always closes immediately, regardless of synthetic
transfers or preview dialog state. Closing an auxiliary window leaves the main
window and its tasks alive.

After confirmation, the UI paints the pause acknowledgment before dispatching
retained background work. Native and encrypted owners receive pause independently.
Native admission closes, pause intent is persisted, and retained workers close
partial files. Vault admission closes, an independent control owner pauses queued
and running jobs plus pending uploads through the existing indexed keyset queries,
and retained transfer owners settle their journals. The pass includes saved rows
that are not materialized in the UI. Active transport cancellation follows durable
pause intent. Manifest discovery without a saved transfer is cancelled; accepted
upload selections save their queue and leave the remaining selection paused.
A final pass after all admitted transfers retire catches late saved selections.
No partial files, keys, receipts or saved task contexts are deleted.

The exit owner waits for local checkpoint/writer settlement, not full transfer
completion. It remains off the UI thread. Bounded 100 ms runtime settlement checks
exist only during exit, never as a recurring presentation or service queue timer.
Writer settlement times out after 30 seconds; a blocked persistence call must
return before its result can be assessed. Failures keep the application open and
offer retry or cancellation. Admission can reopen after failure; already-paused
work stays paused. While checkpoint owners are running, the dialog cannot dismiss
their ownership. Stale completion and deferred account/proxy actions cannot exit a
replacement session. Account switching and proxy changes retain ADR 0037's
cancellable wait-for-completion policy.

Normal application locking still changes only access presentation. Synchronization
and transfer ownership continue while locked. Exit stops the process after the
transfer barrier; transactional sync cursors and caches remain restartable.
Operating-system force termination is outside this cooperative protocol.

## Window commands

One application-level dispatcher is installed before startup completes. It keeps a
weak main-window target and routes Close Window to the focused window. Application
Quit always targets the main lifecycle even when an auxiliary window is focused.

| Action | macOS | Windows / Linux |
| --- | --- | --- |
| Close focused window | Command-W | Control-W; native Alt-F4 |
| Quit application | Command-Q | Control-Q |
| Settings | Command-comma | Control-comma |
| Toggle full screen | Control-Command-F | F11 |
| Minimize | Command-M | Native OS shortcut |
| Search / upload | Command-F / Command-U | Control-F / Control-U |
| Transfers / storage | Command-1 / Command-2 | Control-1 / Control-2 |
| Dismiss current dialog; leave full screen when no dialog | Escape | Escape |

Input editing stays with GPUI Kit. Locked presentation swallows workspace actions,
while native close, quit, minimize and full-screen controls remain available.

## Persistence and verification

No schema/codec version changes: the existing native task state and Vault v1
job/pending-upload contexts already encode explicit pause. Existing automatic
recovery converts interrupted `pausing` jobs to `paused`; explicit pause never
becomes eligible for automatic restart.

Deterministic tests cover a locked blocked writer, 260 non-materialized upload and
download jobs, pending selections, pause-before-cancellation ordering, admission
fences, restart preservation, persistence failure/retry, native partial-file
preservation, stale UI completion, focused PIN shortcuts and auxiliary close.
Native layout checks use English/light previews at 900×600 and actual full-screen.
Windows/Linux bindings are checked deterministically; native execution on those
systems remains a platform qualification step.
