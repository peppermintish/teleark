# Security and data integrity

TeleArk's security contract covers confidentiality, account isolation, integrity and recoverable version upgrades. Crypto, manifest and recovery bundles use explicit versioned formats. [Status](IMPLEMENTATION_STATUS.md) records implemented behavior and concrete validation gaps.

## Threat model

Vault mode aims to conceal content, original filenames/paths and manifest metadata from a party holding only Telegram ciphertext, authenticate every frame/manifest, reject corruption before final publication and recover completed packages after local database loss with account access and valid unlock material.

Assets include plaintext, password/recovery KEKs, Master/File Keys, Telegram sessions, local catalog/history and the integrity/availability of manifests, parts and checkpoints. Consider malicious or corrupted remote bytes, copied local databases, parser abuse, process interruption, supply-chain mistakes, secret logging and another local user without the signed-in OS account's access.

Protection does not extend to a compromised unlocked endpoint, permanently lost keys, deleted Telegram content or account loss. Telegram can observe account/channel relationships, ciphertext sizes/counts, opaque remote names, public format markers, timing and network metadata. Native non-Vault content and indexed metadata are not encrypted by Vault. Padding, traffic shaping and anonymity are not designed.

## Key and format contract

```text
Password -> Argon2id -> Password KEK -> wrapped Master Key
Recovery Key --------> Recovery KEK -> wrapped Master Key
Master Key -------------------------> wrapped random File Key
File Key ---------------------------> frames + manifest metadata
```

Use maintained implementations of Argon2id, AES-256-GCM, BLAKE3 and OS randomness. Master/File Keys are random 256-bit values. AES-GCM uses unique 96-bit nonces under each key and domain-separated AAD. BLAKE3 digests supplement authentication; they never replace it. Exact bytes, bounds, parameters and nonce domains are specified in [Crypto](CRYPTO_FORMAT.md) and [Manifest](MANIFEST_FORMAT.md). Never infer durable layout from Rust/Serde or implement custom cryptographic primitives.

Desktop master and recovery keys are generated automatically using OS randomness and protected by the macOS Keychain, independently of the optional application PIN. A self-contained exact-version recovery bundle includes checksummed Recovery Key text and its authenticated Recovery Wrap, allowing reconstruction after database or device loss. The existing password-wrap codec remains readable but the desktop has no password-based Vault unlock flow. Export requires an explicit protected flow; never upload the secret alongside ciphertext. Loss of every unlock path is unrecoverable.

Recovery rotation replaces the active local record, preventing ordinary unlock with its old key. It cannot revoke an exported bundle that still unwraps the same Master Key: that old bundle can still perform disaster recovery. Protect or securely destroy superseded exports. The device-key adapter stores the existing authenticated recovery-bundle codec and cannot replace offline recovery. Keychain write/readback precedes SQLite commit; missing or denied access preserves existing data. See [ADR 0040](adr/0040-automatic-device-keys-and-optional-pin.md) for versioned item identity, failure ordering and the incompatible desktop access-model update.

## Secret lifetime and account isolation

- Redact secret wrapper/manifest metadata `Debug`; serialization occurs only at reviewed wrapping boundaries. Zeroize secret/plaintext buffers where practical and avoid clones. Deterministic randomness is test-only.
- A bounded retained Vault owner alone holds the unwrapped Master Key. GUI secret inputs are cleared on dismissal/locking. Active-window auto-lock is configurable; changing pages does not lock. Sleep/logout and platform behavior still require release qualification.
- A lock request does not revoke keys already held by an active encrypted transfer. Account switching is blocked until Vault work finishes and pauses/drains native workers before sign-out.
- Every native task and Telegram request carries the expected account. Runtime checks the actual session, and schema 9 prevents rebinding existing known task ownership. A one-time transaction resolves legacy NULL rows only from the actual initial restored session; an unauthorized first connection leaves them non-executable permanently. See [Data model](DATA_MODEL.md).
- Each upload batch repeats complete remote storage discovery; each member refetches the target’s fully verified v2 identity before accessing plaintext or keys. Independent candidates are inspected with a fixed fan-out of four. Local bindings grant no permission. Conflicting, incomplete or damaged evidence fails closed. See the remote storage identity section and ADR 0015 for creation and provenance limits.
- QR links are short-lived authorization secrets: memory-only, debug-redacted, refreshed on expiry/token updates and absent from databases, sessions, diagnostics and fixtures. Never capture real login/recovery secrets during UI review.

Each immutable package writer owns an AEAD-usage registry and fresh File Key identities. Encrypted restart support must hydrate previously used encryption identities before any reuse; this is unfinished implementation work.

## Untrusted bytes, paths and finalization

Parse fixed headers and bounded lengths before allocation. Reject unsupported required versions/algorithms, noncanonical/trailing data, invalid AAD/tags/digests, duplicate/reordered/missing/overlapping/out-of-range parts and frames. Filenames and remote path metadata are untrusted: prevent traversal, absolute escape, separator confusion, reserved-name abuse and unintended overwrite.

Vault downloads authenticate the manifest and each frame, verify part/whole-file digests, flush a controlled private partial and atomically publish without overwrite. Wrong keys, cancellation, missing parts, disk failure and corruption must not expose a final path as complete. Native downloads enforce exact declared length and a strict versioned completion bitmap, with the same publication rule; they currently offer no cryptographic content authentication. See [Transfer](TRANSFER_ENGINE.md).

Telegram and SQLite cannot commit atomically. The generic engine tests ambiguous remote success/checkpoint reconciliation using deterministic fakes. Connected Vault uploads verify parts before publishing the manifest last, and schema 16 persists admitted upload summaries and terminal results. Live samples remain in memory, encrypted restart controls are absent and pre-manifest interruption can leave orphan ciphertext. Completed disaster recovery depends on retained account access, manifest/parts, supported codecs and separately protected key material. A verified Vault output is registered after atomic publication; persistence failure leaves the file intact, and a crash before registration can leave it untracked. Finalization crash injection, orphan cleanup and credentialed recovery remain open.

## Local data, API configuration and logs

SQLite is not encrypted. Schema 16 adds upload names, sizes, account/channel/package/batch identity and task totals; it stores no source paths, content or key material. Interrupted uploads are never automatically resent because a remote publication may precede its local acknowledgment. Pre-v16 memory-only upload history cannot be reconstructed with trustworthy provenance. Schema 10 also retains restored Vault output paths, package/account identity, sizes and times; this inventory reveals local names without unlocking the Vault. Deleting a file does not erase that record. Availability probes are metadata checks, not authentication. It can reveal native filenames/captions, source paths, channel membership, collections, sizes, timestamps and transfer history. Unix-like per-user directories are restricted; platform permissions, SQLite sidecars and backups still require an audit. Telegram session credentials live in the separate adapter session cache and must never enter the Library database or repository.

Personal API ID/Hash pairs are transactionally saved/removed in SQLite, validated and loaded by Runtime, and redacted from ordinary debug output. A readable database/backup exposes the API Hash. Distributor-owned pairs may be compiled into a build and are extractable. They identify an application and do not authorize a Telegram user. Personal pairs override distributor defaults; never reuse Telegram Desktop credentials.

Both process and per-transfer logs use an explicit allowlist: fixed operation/event names, structured classes and numeric IDs, offsets, sizes, timings, rates, attempts and queue/controller counters. Never record API Hashes, session data, QR tokens, phone numbers, codes/passwords/keys, filenames, captions, channel titles, paths, content/plaintext or unreviewed adapter error prose. New fields require review and deterministic coverage where practical. Third-party tracing targets are excluded. [Transfer and diagnostics](TRANSFER_ENGINE.md#progress-session-logs-and-diagnostics) owns bounds, retention and session schemas; full process queues drop events rather than block owners.

## Validation and migration safety

For crypto behavior or format changes, validate affected key wrapping/KDF/nonce/AEAD identities, canonical codecs, parser allocation bounds, redaction and integrated upload/recovery. Retain versioned vectors, tamper/wrong-key tests, fresh-database recovery and crash/interruption regressions. Run affected crypto tests and parser fuzz targets (`cargo fuzz run part_decode`, `cargo fuzz run manifest_decode`); record the tested revision, scope and findings. Daily CI runs bounded ten-minute fuzz campaigns. Fuzz-only cargo-fuzz/libfuzzer-sys licenses were reviewed (MIT/Apache-2.0 and permissive NCSA); they do not enter release binaries.

[ADR 0017](adr/0017-versioned-automatic-migrations.md) defines automatic version upgrades. Crypto conversion authenticates old content before producing a new immutable generation with fresh encryption identities. Verify the replacement before switching authority, retaining a recoverable original throughout. Never overwrite encrypted bytes under the same key/nonce identity or discard a recovery bundle merely because a new writer is available. Local migration backups preserve the same access controls as the source; plaintext and keys never enter progress events or diagnostics.

Compatibility is demonstrated by supported-version fixtures and migration/recovery tests. External review may add evidence but is not a mandatory approval gate. Test results describe their actual coverage; simulated recovery does not establish behavior on a real Telegram account.

## Vulnerability reports

Until a private security contact exists, contact repository owners through an available private channel rather than publishing exploitable details. Include revision, impact, reproduction and embargo needs; omit real sessions, keys and user data.

## Remote storage identity

Storage uses a fixed account/channel binding validated against fresh remote account, ownership and private configuration. For an already bound safe peer, damaged/unpinned management metadata is repairable and does not authorize replacement-channel creation or disable independently authenticated files. New discovery still requires verified remote identity; titles never authorize adoption. Explicit repair changes only the identity record, pin and description pointer. Newer identity formats are preserved. [ADR 0025](adr/0025-fixed-channel-and-retained-key-epochs.md) supersedes the relevant identity policy in ADR 0015. Concurrent cross-device first creation still lacks a Telegram atomic uniqueness primitive.

Schema 15 retains encrypted manifest envelopes and old wrapped-key epochs. It does not persist decrypted manifest metadata. Old ciphertext is not reinterpreted by a new password. At most the active and one historical master key are unlocked; independent health checks hold bounded Arc references, are cancelled on lock/new epoch, and release those references when the cancellable operation stops. Presence checks are not content-authentication claims. Remote data deleted before its manifest was retained cannot be recovered from this inventory.

## Proxy egress policy

An enabled proxy is mandatory for all TeleArk network sockets, including Telegram authentication, datacenter migration/reconnect, avatars and file transfers. Unreachable, rejected, timed-out or disconnected proxies produce persistent visible errors; direct access requires explicitly disabling the proxy and applying that change. Apply cancels/closes the old runtime before persisting and creating a new route. Missing/corrupt/future policy data blocks startup. Numeric proxy endpoints avoid DNS leakage; independent framework HTTP is blocked and external links are copied while restricted. See [ADR 0024](adr/0024-fail-closed-proxy-routing.md) for bounds, tests and application/OS scope.

SOCKS5 password authentication and HTTP CONNECT Basic authentication do not encrypt the connection to the proxy. Telegram payload protection remains MTProto's responsibility. Proxy credentials are local protected SQLite settings, not OS-keychain storage, and must never enter logs or support bundles. Tests/previews use synthetic credentials and loopback peers only.

### Application PIN and active operations

An optional installation-wide PIN gates the entire desktop UI. It is independent
of automatically managed file encryption keys and never revokes the runtime key session: queued
and running uploads, downloads and message/manifest synchronization continue.
The gate clears secret input widgets and displayed recovery material, closes
auxiliary batch windows, and restricts interaction to sign-in, account switching,
proxy configuration and native window/quit controls. Account changes do not reset
the PIN. This is an application access boundary, not an OS or memory-erasure
boundary; a user with write access to application storage can remove its settings.

The versioned PIN verifier wraps only a disposable random key using the existing
Argon2/password-wrap primitive. PIN creation/verification and storage run off the
UI thread after visible acknowledgment. Current-PIN checks protect changing or
disabling it; failed saves preserve the verifier and corrupt/newer records fail
closed. See [ADR 0037](adr/0037-application-pin-and-transfer-drain.md) for the record,
rate limit, callback fences and transfer drain behavior. Runtime Vault session
revocation remains an account-lifecycle and cryptographic-access operation, as
described in [ADR 0026](adr/0026-session-unlock-and-task-key-leases.md).
