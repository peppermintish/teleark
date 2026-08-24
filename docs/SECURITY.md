# Security Model

Status: target threat model for an early prototype. The production crypto, key vault, manifests, and recovery path described here are not implemented in the foundation/mock-UI milestone. TeleArk must not yet be used to protect valuable data.

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
- an attacker who copies the local database but not an unlocked process/keychain;
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

## Target cryptographic design

The provisional design uses:

- Argon2id to derive a password KEK from a per-vault random salt and benchmarked parameters;
- a random 256-bit Vault Master Key, separately wrapped for password and recovery paths;
- a fresh random 256-bit File Key per package;
- AES-256-GCM for file-key wrapping, master-key wrapping, encrypted manifest metadata, and framed content;
- unique 96-bit nonces under every AES-GCM key and domain-separated authenticated data;
- BLAKE3 whole-file and per-part digests for integrity/identity checks in addition to GCM authentication.

No custom AES, GHASH, GCM, Argon2, or random-number generator implementation is permitted. Hardware acceleration may be used only when the chosen maintained crate, target, and CPU actually provide it; the product must not promise it universally.

Exact proposed bytes and nonce invariants are in `CRYPTO_FORMAT.md`. The design remains provisional until implemented, independently reviewed, fuzzed, and locked by golden vectors. BLAKE3 is not a replacement for GCM authentication.

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

## Parser and output safety

Manifests and frames are untrusted. Parse fixed headers and bounded lengths before allocation; reject unsupported required algorithms/versions; verify AAD/tag/digests; reject duplicate, reordered, missing, overlapping, or out-of-range parts/frames. Fuzz parsers against panics, hangs, and unbounded allocation.

Downloads write only to a controlled `.partial` destination. Authentication failure, wrong key, cancellation, disk failure, missing part, or hash mismatch must never expose the final target path as a valid complete file. Flush verified output and atomically rename only after whole-file verification.

Path metadata is untrusted: prevent traversal, absolute-path escape, reserved-name abuse, separator confusion, and overwrite without explicit policy.

## Local data and credentials

The SQLite database may reveal indexed native filenames/captions, channel membership, collections, sizes, timestamps, and transfer history unless a future local-database encryption feature explicitly changes that threat model. Vault manifests protect remote original names, but local search necessarily stores useful metadata while the library is available. Document platform file permissions and backup behavior before release.

Telegram sessions use adapter/platform protection and must never be committed. macOS Keychain support, when added, cannot replace recovery-key backup and must be isolated from durable crypto format definitions.

## Availability and crash consistency

Telegram and SQLite cannot participate in one transaction. Idempotent package/part identity, recoverable opaque names, durable checkpoints, and reconciliation prevent blind duplicate upload after a crash. Recovery depends on Telegram account/channel availability, retained manifests/parts, supported format codecs, and valid key material; Telegram deletion/account loss remains an availability risk.

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
