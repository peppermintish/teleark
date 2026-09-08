# ADR 0006: Use a Versioned Remote Manifest as Recovery Authority

- Status: Accepted architecture; exact v1 encoding remains provisional
- Date: 2026-08-24

The historical provisional-format policy below is superseded by [ADR 0017](0017-versioned-automatic-migrations.md). The manifest/recovery design remains in effect.

## Context

SQLite can be lost, corrupted, or unavailable. If it were the only map from logical files to encrypted remote parts and keys, Telegram-hosted data would become unintelligible after local-device loss. Rust in-memory/Serde layouts are not durable compatibility contracts, and remote upload plus local database persistence cannot be atomic.

## Decision

Every completed Vault package has one authoritative, explicitly encoded, versioned manifest stored remotely. Its bounded public header identifies format/package/vault/crypto/layout information; authenticated encrypted metadata contains original name/path, hashes, exact part descriptors, and remote locators. Remote part/manifest names are opaque and recoverable from package identity.

The manifest uses an explicit canonical codec independent of Rust struct layout, authenticates its public header as AAD, and is uploaded/verified only after all required content parts verify. SQLite remains a rebuildable index/checkpoint cache. `MANIFEST_FORMAT.md` defines the provisional v1 candidate and recovery/compatibility behavior.

## Consequences

- A user with Telegram access and valid key material can rebuild a completed library after database loss.
- Completed package publication requires manifest-last ordering and post-upload verification.
- Parts uploaded without an authoritative manifest are incomplete/orphan candidates, not recoverable completed files.
- Old-version fixtures/readers must survive internal refactors; incompatible evolution needs a new major version.
- Parsers handle hostile bytes with strict bounds, canonical checks, unknown-version/algorithm rejection, and fuzzing.
- Public layout metadata and opaque object linkage remain visible to Telegram; filenames/path/hashes/locators remain encrypted.
- Recovery conflict handling is necessary for duplicate package IDs/generations, missing parts, and remote changes.
