# Security and release review

TeleArk is an early alpha with provisional crypto, manifest and recovery-bundle formats. Connected upload/discovery/recovery adapters and deterministic tests exist; independent security review, durable encrypted controls and credentialed crash/system evidence remain incomplete. Keep independent copies of important data and recovery material. [Status](IMPLEMENTATION_STATUS.md) is the current capability record.

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

Password changes rewrap the same Master Key. A self-contained exact-version recovery bundle includes checksummed Recovery Key text and its authenticated Recovery Wrap, allowing reconstruction of local Vault metadata and a new password after database loss. Export requires an explicit protected flow; never upload the secret alongside ciphertext. Loss of every unlock path is unrecoverable.

Recovery rotation replaces the active local record, preventing ordinary unlock with its old key. It cannot revoke an exported bundle that still unwraps the same Master Key: that old bundle can still perform disaster recovery. Protect or securely destroy superseded exports. Future OS Keychain convenience must not replace offline recovery or determine durable crypto bytes; its adapter remains disabled.

## Secret lifetime and account isolation

- Redact secret wrapper/manifest metadata `Debug`; serialization occurs only at reviewed wrapping boundaries. Zeroize secret/plaintext buffers where practical and avoid clones. Deterministic randomness is test-only.
- A bounded retained Vault owner alone holds the unwrapped Master Key. GUI secret inputs are cleared on dismissal/locking. Active-window auto-lock is configurable; changing pages does not lock. Sleep/logout and platform behavior still require release qualification.
- A lock request does not revoke keys already held by an active encrypted transfer. Account switching is blocked until Vault work finishes and pauses/drains native workers before sign-out.
- Every native task and Telegram request carries the expected account. Runtime checks the actual session, and schema 9 prevents rebinding existing known task ownership. A one-time transaction resolves legacy NULL rows only from the actual initial restored session; an unauthorized first connection leaves them non-executable permanently. See [Data model](DATA_MODEL.md).
- Each upload batch repeats complete remote storage discovery; each member refetches the target’s fully verified v2 identity before accessing plaintext or keys. Independent candidates are inspected with a fixed fan-out of four. Local bindings grant no permission. Conflicting, incomplete or damaged evidence fails closed. See the remote storage identity section and ADR 0015 for creation and provenance limits.
- QR links are short-lived authorization secrets: memory-only, debug-redacted, refreshed on expiry/token updates and absent from databases, sessions, diagnostics and fixtures. Never capture real login/recovery secrets during UI review.

Each immutable package writer owns an AEAD-usage registry and fresh File Key identities. Full restart-time hydration of previously used encryption identities remains a stabilization requirement.

## Untrusted bytes, paths and finalization

Parse fixed headers and bounded lengths before allocation. Reject unsupported required versions/algorithms, noncanonical/trailing data, invalid AAD/tags/digests, duplicate/reordered/missing/overlapping/out-of-range parts and frames. Filenames and remote path metadata are untrusted: prevent traversal, absolute escape, separator confusion, reserved-name abuse and unintended overwrite.

Vault downloads authenticate the manifest and each frame, verify part/whole-file digests, flush a controlled private partial and atomically publish without overwrite. Wrong keys, cancellation, missing parts, disk failure and corruption must not expose a final path as complete. Native downloads enforce exact declared length and a strict versioned completion bitmap, with the same publication rule; they currently offer no cryptographic content authentication. See [Transfer](TRANSFER_ENGINE.md).

Telegram and SQLite cannot commit atomically. The generic engine tests ambiguous remote success/checkpoint reconciliation using deterministic fakes. Connected Vault uploads verify parts before publishing the manifest last, but progress is memory-only, encrypted restart controls are absent and pre-manifest interruption can leave orphan ciphertext. Completed disaster recovery depends on retained account access, manifest/parts, supported codecs and separately protected key material. A verified Vault output is registered after atomic publication; persistence failure leaves the file intact, and a crash before registration can leave it untracked. Finalization crash injection, orphan cleanup and credentialed recovery remain open.

## Local data, API configuration and logs

SQLite is not encrypted. Schema 10 also retains restored Vault output paths, package/account identity, sizes and times; this inventory reveals local names without unlocking the Vault. Deleting a file does not erase that record. Availability probes are metadata checks, not authentication. It can reveal native filenames/captions, source paths, channel membership, collections, sizes, timestamps and transfer history. Unix-like per-user directories are restricted; platform permissions, SQLite sidecars and backups still require an audit. Telegram session credentials live in the separate adapter session cache and must never enter the Library database or repository.

Personal API ID/Hash pairs are transactionally saved/removed in SQLite, validated and loaded by Runtime, and redacted from ordinary debug output. A readable database/backup exposes the API Hash. Distributor-owned pairs may be compiled into a build and are extractable. They identify an application and do not authorize a Telegram user. Personal pairs override distributor defaults; never reuse Telegram Desktop credentials.

Both process and per-transfer logs use an explicit allowlist: fixed operation/event names, structured classes and numeric IDs, offsets, sizes, timings, rates, attempts and queue/controller counters. Never record API Hashes, session data, QR tokens, phone numbers, codes/passwords/keys, filenames, captions, channel titles, paths, content/plaintext or unreviewed adapter error prose. New fields require review and deterministic coverage where practical. Third-party tracing targets are excluded. [Transfer and diagnostics](TRANSFER_ENGINE.md#progress-session-logs-and-diagnostics) owns bounds, retention and session schemas; full process queues drop events rather than block owners.

## Dependencies and clean-room provenance

Review dependency necessity, direct/relevant transitive licenses, maintenance and advisories before adding/upgrading. `cargo deny check`, license notices and exact reviewed exceptions enforce the policy. Security-sensitive changes must rerun vectors, compatibility and tamper/recovery tests.

Never inspect, copy, adapt or derive implementation from GPL `tdl`, including agent summaries, schemas, tests, architecture, naming, control flow or RPC sequencing. Use official Telegram/MTProto specifications, public grammers APIs/docs, permissively licensed sources whose license was verified first and original TeleArk work. [ADR 0012](adr/0012-gpui-kit-and-private-storage-channel.md) records the permissive prefixed GPUI dependency graph and bans the incompatible unprefixed logging crates.

## Independent audit and format-stability gate

No independent audit has been completed. The reviewer must be organizationally independent from implementation, disclose conflicts, identify the exact commit and sign a report separating findings, residual risks and exclusions. Retain its hash/reviewed commit. Findings include severity, affected bytes/API, exploitability, reproduction, remediation and retest evidence.

Scope covers `crates/teleark-crypto`, its vectors under `tests/vectors`, both format documents, key wrapping/KDF/nonce/AEAD registries, parser allocation/canonical/version bounds, redaction and the integrated transfer/recovery lifecycle. Reproduce crypto tests and strict Clippy, plus `cargo fuzz run part_decode` and `cargo fuzz run manifest_decode`. Daily CI runs bounded ten-minute campaigns; release evidence must record materially longer campaigns, toolchain, corpus hash, executions, coverage, peak memory and minimized/replayed findings. Fuzz-only cargo-fuzz/libfuzzer-sys licenses were reviewed (MIT/Apache-2.0 and permissive NCSA); they do not enter release binaries.

`FORMAT_MAJOR = 1` is a candidate identifier. Remove provisional markers only after all five gates:

1. Signed independent review with critical/high findings resolved.
2. Long-running fuzz evidence and retained regression corpora.
3. Independently generated or cross-implementation vectors.
4. Integrated upload, fresh-database discovery, authenticated download and byte-for-byte recovery, including tamper/wrong-key/frame-order/nonce/parser/partial/crash checks and protected real-system evidence.
5. An adopted release ADR freezing canonical bytes and backward-read obligations.

Internal tests, fake remotes and UI demonstrations do not substitute for independent review or real recovery evidence.

## Vulnerability reports

Until a private security contact exists, contact repository owners through an available private channel rather than publishing exploitable details. Include revision, impact, reproduction and embargo needs; omit real sessions, keys and user data.

## Remote storage identity

Storage identity is remote-authoritative: current creator/private metadata, single-member/admin checks, no bots/discussion/TTL, an exact versioned description pointer and a pinned non-forwarded account/channel-bound record are cross-checked before use. SQLite cannot authorize repair or writes. Conflicting candidates or damaged recognizable markers stop setup. Legacy marker/title upgrades are explicitly weaker compatibility evidence and must complete v2 verification before Vault writes. Public metadata can be forged by the account owner; this is not cryptographic app-origin authentication. Concurrent first creation lacks a Telegram atomic uniqueness primitive. [ADR 0015](adr/0015-remote-authoritative-storage-identity.md) records these limits; Vault manifest authentication remains independent.
