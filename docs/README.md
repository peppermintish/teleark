# Documentation guide

Use the current guides below for behavior and contribution requirements. Release
history lives in [CHANGELOG.md](../CHANGELOG.md); architectural decisions retain
their original context and identify superseding decisions. Git preserves removed
progress notes and superseded proposals.

## Start here

| Guide | Purpose |
| --- | --- |
| [Project overview](../README.md) | Features, first launch, source builds and recovery limits |
| [Development](DEVELOPMENT.md) | Local configuration, tests, previews and contribution workflow |
| [Packaging](PACKAGING.md) | Platform prerequisites, artifacts, signing and native verification |
| [Script guide](../scripts/README.md) | Script callers, supported entry points and test safety |
| [Implementation status](IMPLEMENTATION_STATUS.md) | Current capabilities, compatibility and unfinished qualification |
| [Security](SECURITY.md) | Threat model, credential handling, redaction and reporting |
| [Architecture](ARCHITECTURE.md) | Crate boundaries, background ownership and data flow |

## Contracts

| Reference | Owns |
| --- | --- |
| [Data model](DATA_MODEL.md) | SQLite tables, migration chain and account isolation |
| [Preferences](PREFERENCES_FORMAT.md) | Versioned settings, storage identity and access controls |
| [Crypto](CRYPTO_FORMAT.md) | Key wrapping, encrypted part geometry and recovery bundles |
| [Manifest](MANIFEST_FORMAT.md) | Authenticated logical-file metadata and locators |
| [Pending uploads](PENDING_UPLOAD_FORMAT.md) | Portable incomplete-upload envelope |
| [Encrypted transfer recovery](VAULT_TRANSFER_RECOVERY.md) | Durable jobs, reservations, receipts, controls and restart behavior |
| [Transfer engine](TRANSFER_ENGINE.md) | Scheduling, native checkpoints, progress and diagnostics |
| [Index engine](INDEX_ENGINE.md) | Coverage, history synchronization and indexing boundaries |
| [Localization](I18N.md) | Catalog completeness, fallback and display formatting |

## Design and evidence

[ADRs](adr/) preserve accepted choices, including superseded ones that explain
compatibility obligations. ADR 0035 covers durable recovery; ADR 0035a covers source
metadata validation and disambiguates a historical numbering collision.

[ADR 0067: Automatic synchronization](adr/0067-paced-automatic-startup-sync.md#automatic-synchronization-contract)
defines Runtime-owned start and recovery, correctness across waits/restarts, and
the informational synchronization inspector with Close as its only control.

[Access interface](ACCESS_UI_DESIGN.md) records layout decisions and the scope of
native reviews. [Performance](PERFORMANCE.md) records measured probes, fixes and
measurement limits. The [2026-09-15 streaming validation](validation/2026-09-15-memory-streaming.md)
is a dated evidence record, not a claim about every later revision. The
[2026-09-26 open-source audit](validation/2026-09-26-open-source-audit.md) records
script fixes, verification scope and specific Git-history cleanup recommendations.

When behavior changes, update its owning contract and the current status. Add a
release entry when releasing. Avoid copying a full implementation diary into each
guide; a short result with its tested scope is easier to keep accurate.
