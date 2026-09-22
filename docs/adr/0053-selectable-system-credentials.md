# ADR 0053: Selectable system credentials and noninteractive macOS access

Status: accepted. Supersedes ADR 0040's restriction that non-macOS desktops have
no managed-key storage adapter. The existing recovery bundle and encryption
formats remain unchanged.

## Decision

The Keychain setting means **on = macOS system Keychain**, **off = private local
SQLite**. New macOS installations default on. Windows and Linux default off and
cannot enable it until a supported operating-system adapter is shipped. SQLite
storage is an explicit supported mode, including Vault device recovery material;
it does not claim encryption at rest or the access isolation of the macOS
Keychain. The application PIN is still an application access gate, not a key
derivation or database encryption mechanism.

There was no application-managed file keychain in the previous release. Its
macOS implementation already used the default system Keychain. No new keychain
file is created. The new retained credential owner routes Vault recovery
bundles, custom Telegram application credentials, and proxy credentials through
the selected backend. Telegram's grammers session database remains separately
owned by the Telegram adapter; this setting does not convert session persistence.

The macOS adapter disables Keychain interaction around each operation. A
process-wide guard serializes changes to Apple's interaction policy and
preserves an already-disabled policy. An inaccessible or locked item produces a
typed permission/access failure instead of an unexpected system dialog or
plaintext fallback. Stable signed application identity allows the operating
system to recognize subsequent application builds. Self-signed release signing
does not override user ACLs, unlock a locked Keychain, or provide Apple
notarization. Packaging retains the signing identity across builds and verifies
the signed bundle; signing secrets never enter the repository or artifacts.

## Storage and compatibility

Library schema 23 adds `credential_backend`, `credential_items`, and
`credential_cleanup`. The normal ordered migration chain upgrades every
supported earlier schema without modifying its existing keys, settings, or
recovery records. The backend row is initialized with the platform default
once. Its revision is independent of the application release and advances with
every credential or backend mutation.

When a macOS Library is copied to an unsupported platform, the backend becomes
off automatically while all unavailable Keychain references remain intact with
null local payloads. Status exposes their count, and access fails with missing
key guidance. Re-entering application/proxy credentials or importing the exact
Vault recovery bundle repairs those items in SQLite; metadata and encrypted
files are never reset. This is a missing unlock-material condition, not a
silently generated replacement key.

Credential item codec 1 contains an opaque namespace, logical identity,
Keychain account reference, and optional SQLite payload. Payload buffers use
zeroizing ownership across reads, queued commands, and migration snapshots; the
item record intentionally does not implement secret-bearing debug output.
A Keychain item has no SQLite payload. A local item contains the explicit payload bytes. Runtime,
rather than SQL, interprets and validates each payload codec. Custom Telegram
application-pair codec 1 is one version byte, a positive little-endian signed
32-bit API ID, then exactly 32 ASCII hexadecimal API-hash bytes. Proxy payloads
retain the explicit network-policy codec. Vault entries retain the exact
authenticated recovery-bundle codec from ADR 0040.

Existing Vault service `app.teleark.encryption-key.v1` and its
`vault-id/recovery-generation/wrap-digest` accounts remain readable. Migration
discovers readable legacy entries across retained Vault epochs; unavailable
password-only historical epochs remain intact and continue to support recovery
import. Existing registered-but-missing credentials fail closed. Generic
credentials use unpredictable immutable account references, not hashes that
could disclose an offline verifier for a weak proxy password.

## Ownership and atomicity

A dedicated retained owner, with a queue of 32 commands, serializes credential
reads, writes, conditional legacy imports, and migrations. It performs Keychain
work without holding the Storage worker, UI, or network reactor. SQLite requests
remain on Storage. Destruction never joins an unfinished owner on a latency
sensitive caller. Tests and previews use explicit synthetic adapters; they never
access a user's Keychain.

Migration reports loading, securing, saving, and completion, with cancellation,
phase duration, last activity, and a bounded transition history. It bounds input
to 4,096 credential items, 64 KiB per payload, and 16 MiB of total payload bytes.
An oversized migration fails rather than truncating or omitting credentials.
Every Vault bundle is authenticated against its stored identity. Keychain
writes are read back before publication. One SQLite compare-and-swap transaction
publishes all verified items and the new backend preference. A failure,
cancellation, or competing writer before that transaction retains the original
selection and source bytes. Retrying repeats verification safely.

Enabling Keychain deletes live SQLite payloads in the committing transaction,
with SQLite secure deletion enabled and a subsequent WAL checkpoint. This does
not promise erasure from filesystem snapshots, backups, or another reader's
existing database snapshot. Disabling Keychain retains Vault Keychain recovery
entries: these identities can also be referenced by another Library restored
from a backup. Selected-backend routing does not fall back to those entries.

Obsolete generic API/proxy items are queued for deletion in the same transaction
that retires their active references. The owner retries at most 32 queued
deletions per cleanup pass; denied cleanup remains durable and its count is exposed
in Keychain status. Re-enabling uses fresh generic references so a pending old
deletion cannot remove the newly active value. Vault recovery entries are never
placed in this generic cleanup queue. A process crash after a new immutable
Keychain write but before SQLite publication can leave an unreferenced protected
item. Ordinary failed writes, cancelled migrations, and lost compare-and-swap
races queue their unpublished generic copies for deletion after checking that
SQLite does not consider them authoritative. Failure to persist cleanup when
Storage itself is inaccessible is reported as a sanitized diagnostic. The crash
gap remains, as with the previous Vault write-before-commit design; it cannot
overwrite the authoritative source.

## Evidence

Deterministic temporary-database tests cover backend switching in both
directions, restart, schema-22 upgrades, stale revision rejection, rollback,
lost writes, denied access, missing registered items, conditional imports, and
cancellation while Keychain is blocked. Unrelated Storage reads remain responsive
during a blocked credential operation. Integrated Vault tests cover legacy
Keychain identities, historical epochs, recovery rotation, and authenticated
round trips through both backends. These unit tests use synthetic secrets and
do not access a real operating-system Keychain.

The optional native `scripts/macos/verify-keychain-signing.sh` qualification uses
Swift and Security.framework with an exclusively temporary Keychain and fixed
synthetic payload. On macOS it verifies that a first build creates and reads its
own default ACL item, a changed build signed with the same pinned certificate
and explicit designated requirement reads it after replacement at the same
application path, and a build with the same public identifier but an altered
signing identity is denied without interaction. The valid signed build reads
the item again after that negative control. All Keychain queries and additions
name the isolated Keychain; the script removes it and the temporary signing
Keychains at exit. It neither accesses login-Keychain items nor changes system
trust. This qualification passed on 2026-09-22; the altered identity returned
`errSecAuthFailed` (-25293). Existing user-specific ACLs, locked login Keychains,
and upgrades from previously different signing identities remain outside that
measured scope.
