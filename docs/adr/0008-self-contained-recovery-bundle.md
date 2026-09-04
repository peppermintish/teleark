# ADR 0008: Export a Self-Contained Vault Recovery Bundle

- Status: Accepted for the provisional v1 candidate
- Date: 2026-09-04

## Context

A raw Recovery Key can unlock a Recovery Wrap only when the local Library database still contains that wrap. TeleArk's recovery goal also covers loss of the local database while encrypted packages remain in Telegram Saved Messages. Exporting only the raw key would therefore provide a misleading backup, while uploading the recovery material beside ciphertext would defeat the intended separation.

## Decision

Export one exact, versioned, self-contained secret containing the canonical checksummed Recovery Key text and the existing authenticated 88-byte Recovery Wrap. The `TARK-RB1` grammar and checksum rules are specified in `CRYPTO_FORMAT.md`; parsing is canonical and performs no whitespace, case, or punctuation normalization.

The bundle is displayed only immediately after Vault creation or recovery rotation and can be written through an explicit user action to a newly created private file. It is never silently persisted in the Library database, logs, diagnostics, Telegram, or ordinary UI state after the user hides it. A fresh installation can authenticate the bundle, recover the same Vault Master Key, persist the embedded Recovery Wrap, and create a new Password Wrap.

Recovery rotation replaces the active local Recovery Wrap and causes the ordinary unlock flow to reject a bundle for an older generation. Rotation cannot revoke an already exported bundle because that bundle still contains a valid wrap of the same Vault Master Key. The UI and security documentation state this limitation explicitly.

OS credential storage remains an optional future unlock adapter and is not part of this durable format.

## Consequences

- Database-loss recovery no longer depends on retaining local wrapped metadata.
- The exported bundle is as sensitive as the data it unlocks and needs offline protection.
- Exact prefix, lengths, uppercase hexadecimal representation, checksum domain, and embedded wrap bytes are provisional persistent-format contracts covered by canonical/tamper tests.
- A future incompatible recovery representation requires a new bundle version and migration/import support; it must not silently reinterpret `TARK-RB1`.
- Revocation of copied disaster-recovery bundles would require Master Key rotation and package rewrapping or re-encryption, which is not implemented.
