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

## Native interaction review

The macOS review uses an isolated native build with `--preview-ui` semantics,
English and light mode. Its temporary native entry point supplies synthetic
launch arguments directly; shell wrappers and re-executing the binary prevented
the desktop tool from attaching. The direct entry point resolved that issue.
The harness and its temporary Cargo target are outside the delivered source.
No installed account, personal credential, session or user document was used.

The minimum-size window is 900×600 logical points (the captured image includes
macOS titlebar chrome at 2× backing scale). Actual native fullscreen was entered
with Control–Command–F and verified by the native window state, the Exit full
screen control and 2880×1800 display capture. A larger ordinary window was not
used as a substitute.

| Surface | Interaction evidence |
| --- | --- |
| Transfers | Compact and fullscreen rows, tabs, Manage disclosure, batch/child selection, search/clear, details opening and independent scrolling. Escape closes details before leaving fullscreen. |
| Transfer failure | Specific timeout cause and retry guidance stay readable. Historical activity scrolls independently at both sizes. Missing native controller measurements do not claim encryption or CPU work. |
| Upload | Compact and fullscreen overlay, reachable footer, file-picker cancellation, selecting one synthetic 53 B temporary file, removing it, and Escape cancellation. No transfer was submitted. Removed the read-only “Upload options” disclosure and unsupported “Checked just now” claim. |
| Storage | Files/Raw Files navigation, no-match search feedback, search clearing, file details and Escape, then Channels navigation without retaining unrelated details. |
| Library | Local/Remote tabs and file-type filtering at compact and fullscreen sizes. Single-file results exposed count labels that now use count-neutral wording. |
| Channels and synchronization | File-type and batch disclosures, waiting/rate-limit explanation, event history and omission notice. Inspector scrolling leaves the file list in place; Escape works after its own controls receive input at both sizes. |
| Speed limits | Negative input is rejected; a synthetic 1,024 KiB/s upload limit displays as 1 MiB/s. Navigation shortcuts do not move the page behind the modal; Escape dismisses it in fullscreen. |
| Settings | General, Appearance and Downloads in compact/fullscreen layouts; retained Light selection, switch confirmation/cancellation, scrolling, concurrency controls and settings feedback. Keychain confirmation was cancelled without changing the credential backend. |
| Sign-in and network | QR/phone navigation, Enter submission retaining a synthetic invalid draft, return from network settings, invalid proxy port rejection, and compact scrolling to the validation message/actions. Fullscreen layout remains reachable. No network session was opened. |
| Locked screen | Navigation shortcuts stay blocked. Account-switch confirmation is cancelled without changing accounts; Escape retains fullscreen and returns focus to the visible lock/proxy view. |

Additional fixes from this review count retained Vault commands as active work
while account/proxy transitions drain, including when their bulk queue was
cancelled. Completion wakes the transition even when a result is stale. The
locked-screen Escape handler now follows the same confirmation-first order as
the workspace. About includes the canonical current changelog. Windows installer
help links now point to the configured repository.

## Evidence and boundaries

The final GUI run passed **214 tests**, with three existing manual comparative
benchmarks ignored. This is 25 more passing tests than the audit baseline. The
focused i18n run passed all 26 tests. Scoped strict Clippy, formatting and script
regressions passed; full release qualification is recorded separately when run.
The deterministic cases cover actual GPUI events/focus, compact geometry,
fullscreen state, blocked operations, independent completion, cancellation,
capacity limits, account generations, and shutdown barriers. Native previews
exercise presentation and local interactions; unavailable remote actions remain
disabled in this environment.

This is a macOS synthetic interaction review, not exhaustive platform,
accessibility or live-network qualification. Windows/Linux native interactions,
real Telegram sign-in/2FA, actual uploads/downloads, OS sleep/logout, credential
storage changes and recovery exports were not exercised. Their protected
qualification remains tracked in implementation status. No personal files were
opened or transferred; file-picker testing selected only the temporary fixture.
No persistent schema, encrypted format or dependency changed.
