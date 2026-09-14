# ADR 0040: Automatic device keys and an optional application PIN

Status: Accepted — 2026-09-15

## Decision

The desktop application has one optional access gate: its application PIN. Users
may leave the PIN unset. Setting, changing or removing it never generates,
rewraps or replaces file encryption keys. After application access is granted,
Files, uploads, transfers, channels and settings open directly. There is no
Vault password form or second unlock dialog. This supersedes the desktop
password/unlock flow and disabled device-key adapter in ADR 0026 and the earlier
Vault UI decisions; ADR 0037's PIN verifier and background-operation policy remain.

When account-scoped private-channel management completes, the frontend admits a
key-preparation command and waits on its retained background owner. Master keys,
recovery keys and file keys continue to use the Crypto crate's OS-backed CSPRNG.
The macOS adapter uses `security-framework 3.7.0` (MIT OR Apache-2.0, already in the
locked graph) to store an authenticated recovery bundle in the system Keychain.
Only the key owner performs Keychain I/O. There is no plaintext-file fallback.
Other desktop platforms remain unqualified and return an access error until their
secure-store adapter is implemented.

## Persistence and failures

The Keychain service namespace is `app.teleark.encryption-key.v1`. Each account
identifier is `<lowercase vault-id hex>/<recovery generation>/<BLAKE3 recovery-wrap
hex>`. These values are non-secret, immutable identifiers. The digest isolates
competing rotations that happen to choose the same generation. The item payload
is the existing exact recovery-bundle v1 ASCII codec; it is authenticated against
the exact durable recovery wrap before use. Keys are scoped by random vault IDs,
not by a PIN, account name, path or user-entered text.

Creation, restore and rotation write and read back the Keychain item before
committing the SQLite metadata transaction. Failed validation, permission denial,
cancellation or stale logout generation cannot publish a usable session. A failed
SQLite CAS may leave an unreferenced Keychain item; it never overwrites the valid
item referenced by the committed record. Retry rereads database authority. A valid
explicit recovery import can repair a missing or corrupted Keychain item; a
missing item never means the existing database should be reset.

SQLite schema 19, password-wrap v1, recovery-wrap v1, recovery-bundle v1, manifest
v1 and part-container 2.0 read/write contracts remain unchanged. The record's
password wrap retains its original byte meaning, wrapping the random master key
under an independently generated random 256-bit password that is then discarded.
This field is no longer a desktop unlock path. No file or recovery key is derived
from that value. Legacy codec readers and compatibility fixtures remain.

This change is an explicitly requested incompatible desktop access-model update
for an installation with no legacy data. It does not add a legacy-password
migration wizard. Unexpected old records are preserved and require their recovery
bundle if they have no matching device-store entry; no silent replacement occurs.

## Recovery and ownership

Recovery bundle viewing/export and rotation remain explicit settings actions.
Secret-bearing job results have redacted Debug and zeroize on drop. Stale account
or PIN-locked callbacks cannot reveal a bundle. Exports remain opt-in and use the
existing restrictive, non-overwriting file export flow. Rotating recovery cannot
revoke a previously exported bundle that still authenticates the same master key.

A new upload epoch requires explicit confirmation and preserves older wrapped
records and ciphertext. Import of a known historical bundle verifies the exact
stored wrapper. A valid bundle from another installation can also add a historical
record using an insert-only Storage operation, without switching the current
upload key. This keeps disaster recovery available after automatic first setup.
The existing active-plus-one-historical session-key bound remains in effect.

Admission and completion retain their account/session generation fences. PIN
locking does not retire runtime keys or cancel background transfers. Account
sign-out retires key admission; delayed preparation cannot reopen the session.
Transfers and storage owners remain responsive while Keychain work waits.
Preparation phases, duration, last activity, cancellation and terminal results are
visible across navigation, with a bounded sixteen-transition timeline. Timers only
present elapsed time; service startup is triggered by management events, not UI
polling. Missing keys produce a retry/recovery reason without gating pages.

## Evidence

Deterministic tests use temporary databases, fake Keychain adapters and controlled
blocking. They cover initialization with no PIN, repeated admission, restart and
file-key preservation through PIN changes/removal; rejected and unverified saves;
missing/corrupt entries; rotation rollback and recovery authenticity; concurrent
SQLite commits; cancellation/logout while saving; and recovery import after
another installation has already generated its own active key. GUI regressions
cover direct navigation/upload selection despite key preparation, visible transfer
metadata and search, status layout and stale completion. Full source gates and
English/light native 900×600 plus actual full-screen reviews accompany delivery.
No live Keychain mutation or credentialed Telegram qualification is part of these
synthetic checks.
