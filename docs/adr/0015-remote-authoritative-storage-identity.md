# ADR 0015: Remote-authoritative storage identity

- Status: Accepted
- Date: 2026-09-07
- Supersedes: ADR 0014's saved-binding preference, lowest-ID selection and marker-only identity policy. Automatic setup and bounded work remain.

## Decision

Every desktop login/setup discovers storage from Telegram. SQLite bindings are
write-through caches, never authority for selection, repair, creation or upload.
Exactly one remotely eligible candidate is required. Multiple candidates stop
setup and writes; no arbitrary winner, merge, deletion or automatic replacement
is allowed. Local database absence, corruption or a stale binding cannot change
the remote choice. The compatibility read-only discovery API retains its old
status types, but does not grant permission to upload.

Discovery reads a complete dialog snapshot (less than 10,000 entries), then full
metadata for at most 64 owned channels or channels with the reserved app name.
Owned public/renamed channels are included so damaged privacy cannot silently
be treated as absence. Incomplete scans and failed reads are errors. A channel
that retains a TeleArk description prefix or reserved title but has inconsistent
metadata blocks setup instead of disappearing from the candidate set.

Cross-checks use the current authenticated account, creator ownership, full
private broadcast metadata, no public usernames/aliases, one participant and
one administrator, no bots, linked discussion or automatic message deletion.
The v2 description points to an exact message ID. That message must exist in the
same channel, be pinned, not forwarded, and carry the supported identity version
and exact account/channel IDs. No locally supplied ID can bypass these checks.
Vault preflight repeats complete remote discovery and identity verification.

The canonical channel title is `🔒 TeleArk · Managed Storage`. Its localized
human description and pinned identity message warn against editing/deleting the
channel or messages in other apps because file access and recovery may break.
After remote verification, TeleArk can restore the recognizable title and
localized description. It never repairs an arbitrary locally cached channel.

## Compatibility and remote bytes

[Preferences format](../PREFERENCES_FORMAT.md) specifies the explicit text codecs
and fixtures. Unique legacy v1 channels require the legacy exact description
marker, reserved old/new title and current ownership/privacy checks before an
automatic upgrade. This is compatibility evidence, not v2 verification. Setup
searches at most 64 remote identity-message results to recover interrupted
initialization, otherwise sends and pins the record, writes the v2 pointer, and
refetches verification before enabling use. Old channels renamed without the
reserved title fail closed instead of being claimed on local knowledge.
No local creation-intent setting, dependency or database schema is added.

## Limits that cannot be hidden by a local lock

Telegram's [createChannel](https://core.telegram.org/method/channels.createChannel)
has no account-scoped unique application key or compare-and-set parameter.
Independent devices can race after both observe absence. Creation is followed
by complete rediscovery, and competing channels stop setup; every subsequent
Vault preflight also checks uniqueness. A process-local guard only suppresses
blind retries of an uncertain RPC; it is not identity evidence or a distributed
lock. This does **not** guarantee exactly one physical channel under concurrent
first creation or indefinitely delayed visibility. A Telegram-supported atomic
primitive would be needed for that stronger guarantee without an external
coordinator. Do not advertise it as achieved.

Likewise, these remote fields detect accidental confusion, copied records with
wrong IDs, damage and privacy changes. They do not cryptographically prove which
application created a channel: the account owner can forge all channel content.
If every remote identifying trace is deleted, no fresh device can distinguish
that history from first use. No channel-identification claim replaces Vault
manifest authentication. Credentialed multi-device and interruption testing
remains a protected qualification gate.

Only official Telegram methods ([pinning](https://core.telegram.org/api/pin),
[edit title](https://core.telegram.org/method/channels.editTitle),
[edit description](https://core.telegram.org/method/messages.editChatAbout)) and
MIT OR Apache-2.0 grammers public APIs informed this implementation.
