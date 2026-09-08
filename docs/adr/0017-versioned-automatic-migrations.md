# ADR 0017: Versioned compatibility and automatic migration

- Status: Accepted
- Date: 2026-09-08
- Supersedes the provisional-format and review-prerequisite policy in ADRs 0004, 0006 and 0008; their cryptographic, manifest and recovery designs remain unchanged.

## Decision

TeleArk targets a mature product. Delivery is assessed by working behavior, correctness, recovery and validation evidence. Stage labels and mandatory third-party review are not prerequisites for development or release.

App versions and persistent schema/format versions are separate. Each release documents the versions it reads/writes and the source versions it upgrades automatically, including skipped releases. Incompatible bytes require a new version and a shipped migration path; internal refactors cannot reinterpret existing data. Existing v1 crypto/manifest/recovery fixtures define the compatibility baseline.

Normal supported upgrades automatically detect and migrate local data, settings and recoverable transfer state without manual scripts, exports or resets. Older remote packages remain readable; required format conversion uses authenticated replacement generations and preserves the original recovery path until the replacement is verified. Unlock or account access may be necessary, but routine migration needs no approval dialog.

Migration work runs under retained, bounded backend owners. Transactions, backups or copy-on-write retain recoverable originals. Each step can safely resume or roll back after failure; the target version becomes authoritative only after validation and commit. Unknown/newer formats are preserved with actionable guidance. Account ownership, keys, user files and recovery material are never discarded to make an upgrade succeed.

The UI displays migration activity before expensive work and every subsequent phase, including preparation, backup, conversion, verification and index rebuilding. Show progress when measurable, otherwise phase duration and recent activity. Keep the window responsive and gate only conflicting actions. Test version chains, skipped upgrades, interruption, insufficient space, malformed input, preserved contents/keys and UI feedback.

## Implementation boundary

This decision changes the development contract, not encoded bytes or the app version. SQLite already applies ordered transactional migrations through schema 10. A complete migration coordinator with progress, recovery and cross-format conversion remains implementation work recorded in [status](../IMPLEMENTATION_STATUS.md). Existing test evidence remains factual; adopting this policy does not establish unperformed validation.
