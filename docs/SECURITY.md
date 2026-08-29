# Security Model

Status: threat model for an early alpha with a provisional cryptographic codec candidate. Framed encryption, key wrapping, strict manifest parsing, AEAD-identity hydration APIs, candidate vectors, authenticated Manifest publication, production filesystem/Telegram adapters, and fake-remote database-loss recovery exist. A production Key Vault/unlock service, credential storage, independent review, and the encrypted desktop workflow do not. TeleArk must not yet be used to protect valuable data.

## Security goals

The intended Vault mode protects file content, original filename/path, and encrypted manifest metadata from disclosure to Telegram or an attacker who obtains only Telegram-stored ciphertext. It authenticates every crypto frame and manifest metadata, detects corruption/tampering before completion, limits memory during large transfers, and supports recovery from Telegram storage after local database loss when the user retains account access and a valid unlock/recovery secret.

The product also aims to prevent accidental exposure through final filenames, logs, debug formatting, crash checkpoints, fixtures, and source control.

## Assets

- plaintext files and original names/paths;
- passwords and Recovery Keys;
- derived password/recovery KEKs;
- Vault Master Key and per-file File Keys;
- Telegram session credentials and account identity;
- local indexed metadata, collection membership, and transfer history;
- integrity and availability of manifests, parts, and local recovery state.

## Adversaries considered

- a party able to inspect or modify objects stored in Telegram;
- network and storage corruption (transport security remains a Telegram/MTProto responsibility);
- an attacker who copies the local database but not the separately protected Telegram session cache;
- malformed or malicious manifest/frame bytes presented to parsers;
- accidental process termination between remote success and local persistence;
- dependency/supply-chain mistakes and accidental secret logging;
- another local user without access to the signed-in OS account or unlocked process.

## Explicit non-goals and limits

TeleArk does **not** provide complete anonymity, zero metadata leakage, protection from a fully compromised unlocked endpoint, or guaranteed availability of Telegram-hosted content. It cannot protect plaintext while a legitimate user has it open, prevent screen capture/malware with equivalent local privileges, or recover keys the user has permanently lost.

Telegram can observe at least:

- account and storage-channel relationships;
- ciphertext object sizes and counts;
- message/upload/download timing;
- traffic patterns and IP/network metadata available to the service;
- opaque remote filenames and public format-identification fields;
- any native, non-Vault Telegram metadata/content the user chooses to index.

Padding/traffic shaping is not currently designed. Multipart sizes and timing may reveal approximate logical size and activity.

## Candidate cryptographic design

The provisional design uses:

- Argon2id to derive a password KEK from a per-vault random salt and benchmarked parameters;
- a random 256-bit Vault Master Key, separately wrapped for password and recovery paths;
- a fresh random 256-bit File Key per package;
- AES-256-GCM for file-key wrapping, master-key wrapping, encrypted manifest metadata, and framed content;
- unique 96-bit nonces under every AES-GCM key and domain-separated authenticated data;
- BLAKE3 whole-file and per-part digests for integrity/identity checks in addition to GCM authentication.

No custom AES, GHASH, GCM, Argon2, or random-number generator implementation is permitted. Hardware acceleration may be used only when the chosen maintained crate, target, and CPU actually provide it; the product must not promise it universally.

Exact candidate bytes and nonce invariants are in `CRYPTO_FORMAT.md`. Explicit codecs and fixed candidate vectors now exercise those bytes, including tamper/wrong-key/layout rejection and duplicate key/nonce-identity prevention. The design remains provisional until independently reviewed, continuously fuzzed, integrated into a safe service/transfer lifecycle, and proven by database-loss recovery. BLAKE3 is not a replacement for GCM authentication.

## Key hierarchy and recovery

```text
Password -> Argon2id -> Password KEK -> unwrap Vault Master Key
Recovery Key --------> Recovery KEK -> unwrap Vault Master Key
Vault Master Key --------------------> unwrap random File Key
File Key ----------------------------> content frames + manifest metadata
```

Changing a password rewraps the Vault Master Key; it does not re-encrypt all file content. OS Keychain integration is a convenience adapter, not the durable recovery authority. A Recovery Key must be generated from secure randomness, displayed/exported through an explicit protected flow, and never silently uploaded alongside the material it unlocks.

Loss of every valid password/recovery/keychain path makes encrypted data unrecoverable by design. TeleArk has no backdoor. The UI must state this clearly before enabling Vault storage and should encourage an offline recovery-key backup.

## Secret handling

- Never log passwords, Recovery Keys, master/file keys, KEKs, Telegram sessions, or plaintext frame buffers.
- Secret wrapper `Debug` output is redacted; serialization is opt-in only at explicit wrapping boundaries.
- Zeroize key/password buffers where practical and avoid unnecessary clones.
- Use cryptographically secure production randomness; deterministic RNG inputs are test-only.
- Keep secrets out of command lines, panic messages, telemetry, screenshots, test fixtures, and source control.
- Limit unlocked-key lifetime and define lock-on-sleep/logout behavior before release.
- Do not hold secrets in GUI view models longer than needed.

Telegram QR login links contain short-lived authorization secrets. TeleArk keeps
them only in memory, redacts them from adapter/runtime `Debug` output, refreshes
them on Telegram's login-token update or expiry, and never writes them to the
Library database, session file, ordinary logs, or fixtures.

The candidate crate uses redacted secret and manifest metadata `Debug` implementations, zeroizing key/password/plaintext buffers where practical, OS randomness in production, and deterministic randomness only behind test support. Its mandatory in-memory AEAD-usage registry is a safety invariant, not durable storage: a future Vault service must own it and hydrate all existing identities before any post-restart encryption.

## Parser and output safety

Manifests and frames are untrusted. Parse fixed headers and bounded lengths before allocation; reject unsupported required algorithms/versions; verify AAD/tag/digests; reject duplicate, reordered, missing, overlapping, or out-of-range parts/frames. Fuzz parsers against panics, hangs, and unbounded allocation.

Downloads write only to a controlled `.partial` destination. Authentication failure, wrong key, cancellation, disk failure, missing part, or hash mismatch must never expose the final target path as a valid complete file. Flush verified output and atomically rename only after whole-file verification.

Native, non-Vault Telegram downloads use the same publication rule but currently
verify only the exact byte count declared by Telegram. They create a private
collision-resistant application partial name, reject an existing final path,
flush before atomic no-replace publication, and remove the partial on an observed failure. This
is truncation/finalization safety, not cryptographic content authentication.

Path metadata is untrusted: prevent traversal, absolute-path escape, reserved-name abuse, separator confusion, and overwrite without explicit policy.

## Local data and credentials

The SQLite database may reveal indexed native filenames/captions, local source paths, channel membership, collections, sizes, timestamps, and transfer history unless a future local-database encryption feature explicitly changes that threat model. The current desktop alpha persistently imports this local metadata. Vault manifests protect remote original names, but local search necessarily stores useful metadata while the library is available. The default per-user database directory is restricted on Unix-like systems; platform permissions, auxiliary SQLite files, and backup behavior still require a release audit.

Telegram sessions use adapter/platform protection and must never be committed. macOS Keychain support, when added, cannot replace recovery-key backup and must be isolated from durable crypto format definitions.

The desktop persists the Telegram application API ID and API Hash together in
the local SQLite settings table so authentication can resume after restart.
The pair is transactionally saved/removed, validated before use, loaded by the
runtime rather than returned to the GUI, and redacted from ordinary debug
output. SQLite is not encrypted, so anyone who obtains a readable Library
database or backup can obtain the API Hash. The Settings and onboarding UI make
this tradeoff explicit. Telegram login/session secrets remain in the separately
protected adapter session cache and are not stored in the Library database.

An official distributor may compile credentials registered for its own TeleArk
application into its build. Embedded API credentials are extractable from the
binary and are therefore application identifiers, not secret authorization or
an account session; distributors must monitor and rotate them when necessary.
A personal SQLite pair overrides the distributor pair and can be removed
explicitly. TeleArk does not embed or reuse Telegram Desktop's published
credentials. Telegram's official documentation states that sample IDs are
limited to testing and that third-party applications must obtain their own API
ID.

## Availability and crash consistency

Telegram and SQLite cannot participate in one transaction. Idempotent package/part identity, recoverable opaque names, durable checkpoints, and reconciliation prevent blind duplicate upload after a crash. Recovery depends on Telegram account/channel availability, retained manifests/parts, supported format codecs, and valid key material; Telegram deletion/account loss remains an availability risk.

The generic Transfer engine exercises ambiguous-success and checkpoint-failure
reconciliation against deterministic fakes, including no-duplicate remote
parts, and the runtime composes it with real Telegram and SQLite adapters. A
separate bounded native-download worker is connected to the desktop. Native
download queue entries are currently in-memory and do not resume after restart;
credentialed Telegram crash/system testing still remains.

## Dependency and clean-room security

All dependencies require license, maintenance, and security review. CI should enforce advisories, banned/duplicate policy as appropriate, and license allowlists when repository configuration is added. Security-sensitive crate upgrades require vectors and compatibility/tamper tests.

Never inspect or derive implementation from GPL `tdl`; legal provenance is part of supply-chain integrity. Use official Telegram/MTProto documentation, public `grammers` APIs/docs, public specifications, and original analysis.

## Required security tests before production use

- correct encrypt/decrypt plus ciphertext/tag/AAD tamper rejection;
- wrong File Key, Master Key, password, and Recovery Key rejection;
- frame reorder, omission, duplication, truncation, and invalid final-frame rejection;
- nonce uniqueness across all supported part/frame indices;
- manifest tamper/version/algorithm/length-limit rejection;
- fixed crypto vectors and old manifest fixtures;
- parser fuzzing and allocation bounds;
- `.partial` failure safety;
- crash injection around remote upload/checkpoint boundaries;
- database-loss recovery through a fake remote, followed by real protected integration testing.

No UI demonstration substitutes for these tests or for external cryptographic review.

## Reporting vulnerabilities

Until a private security contact is published, avoid disclosing exploitable details in a public issue. Contact the repository owners through an available private channel and include the affected revision, impact, reproduction conditions, and proposed embargo needs. Do not include real user data, sessions, passwords, or keys in reports.
