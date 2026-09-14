# ADR 0037: Application PIN and transfer drain

Date: 2026-09-14. Status: accepted.

## Decision

An optional application PIN gates the entire main window. With a PIN configured,
startup presents the account sign-in/PIN surface; manual Lock returns to that
surface. Without a PIN there is no application lock. The PIN is global to the
installation and survives account switches, so switching Telegram accounts never
bypasses the gate. The locked surface permits sign-in, account switching and proxy
configuration. Workspace menus/shortcuts and auxiliary batch windows are removed
from the locked presentation. Native window controls and Quit remain available.

This explicitly supersedes the application-facing locking decision in ADR 0026
and the lock indicator in ADR 0034. Vault password/recovery operations still
control cryptographic access to encrypted files; they are not application PINs.
Application Lock never invokes `DesktopVault::lock`, changes a service generation,
cancels a scan, drops transfer handles, or clears synchronized projections. The
runtime keys and operations needed by synchronization and queued/running transfers
remain available. Secret input widgets and displayed recovery secrets are cleared.
No immediate key-erasure or protection against a compromised OS is claimed.
The obsolete `lock_vault_when_hidden` desktop preference is removed from the
runtime contract and ignored if it remains in an older settings table.

## PIN record and execution

`application.pin` in the existing protected settings table contains independent
JSON envelope version 1: `{"version":1,"verifier":[…]}`. `verifier` is the byte array
encoding of the existing version-1 authenticated password wrap of a random,
disposable 32-byte key with a random domain and salt. It is never a Vault key or a
Telegram credential. PINs contain 6–12 ASCII digits. Readers/writers support v1;
absence disables the gate, while malformed or newer records fail closed without
changing the stored value. The frozen v1 fixture uses a synthetic test PIN.

Verification, generation and persistence run on retained background tasks, after
an acknowledgment frame. Plain inputs are cleared at submission; runtime PIN
strings use zeroizing storage. Five wrong attempts introduce a 30-second session
cooldown without a countdown/render timer. Unlock callbacks are fenced by the
application-lock generation. A current PIN is required to replace or disable an
existing PIN. Serialized compare-and-replace persistence rejects stale settings
writers. A failed save preserves the existing verifier and access policy.

## Disruptive actions

Application-menu/keyboard Quit and main-window Close, account switching and
Apply Proxy share one transfer gate. It covers upload admission/preparation,
Vault downloads, queued/running transfers, pending pause/cancel acknowledgments
and native partial-file cleanup. With active work, a modal offers Cancel or
Wait, then continue. Cancel affects only the requested operation. Waiting keeps
background work running and blocks conflicting new UI actions. Failed or paused
work remains durably recoverable; it is not silently cancelled or deleted.

Transfer revisions and preparation/completion callbacks advance the gate. A final
recheck precedes one deferred action; repeated notifications cannot run it twice.
There is no service-polling timer or repeated storage query. Proxy application
retains the existing fail-closed transport route and account switching retains
runtime account isolation. Waiting can be cancelled while a remote is slow or
blocked. Forced process termination and OS shutdown are outside this user-action
gate; existing durable recovery remains responsible for those interruptions.

## Synchronization presentation

The bottom synchronization status and private-channel warning are display-only.
Settings → Network proxy → Sync activity and logs opens the inspector. Its bounded
merged timeline includes directory loading, channel/manifest activity and private
edit/delete/gap observations, regardless of application lock state or private
section expansion. Stored timestamps and an application monotonic-to-wall-clock
anchor produce fixed event times. Proxy history uses the same fixed timestamps;
no history row shows an advancing age. A revision key prevents unrelated owner
notifications from rebuilding the timeline; the list materializes visible rows.
Retention omissions remain visible. Active catalog/library work cannot suppress
private/channel failures in the shell summary.

## Verification

Deterministic coverage includes disabled PIN, verifier persistence/frozen v1,
wrong/malformed PINs, stale settings writes, acknowledgment before crypto, stale
unlock rejection, background handle/key/projection preservation, locked shortcuts,
all three disruptive actions in both access states, cancellation, native cleanup,
and completion-triggered single execution. Visual review uses synthetic English,
light-mode previews at 900×600 and actual native full-screen.
