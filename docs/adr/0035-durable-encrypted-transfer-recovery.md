# ADR 0035: Durable encrypted transfer recovery

Status: accepted; integrated into DesktopVault execution, recovery controls and guidance.

## Context

DesktopVault uses its own encrypted upload/restore pipeline. Generic TransferEngine checkpoint tests cannot establish its restart behavior. Schema-16 upload receipts retain outcomes but lack keys, source identity and part checkpoints. Treating those receipts as resumable can create new packages or duplicate remote objects while suggesting that work continued.

## Decision

Use an account-scoped durable job ledger with immutable task/package context and generation-fenced control transitions. Persist pause/cancel intent before signalling a worker; acknowledge stopped states only after owned work drains. Cold-start recovery preserves intent and invalidates old execution generations. Explicit retryable and blocked outcomes remain visible rather than automatically looping.

Persist independently versioned wrapped-key context and immutable part reservations. Reserve AEAD and publication identities durably before use. Recovery must authenticate keys/manifests, validate source/local extents, and reconcile ambiguous publication before resending. Ciphertext caches and non-overwriting output publication are part of the required implementation, not optional qualifications. Critical SQLite writes use FULL synchronous WAL commits and fullfsync where supported, on the storage owner.

Admit a complete upload selection in one durable transaction before executing any
file. Encode one metadata record at a time and keep filesystem/network work outside
the transaction. This supersedes independently committed 128-file groups: process
death during admission must not leave an unintended runnable prefix. Show saving
before the transaction and acknowledge the saved count only after commit. The
write lock spans metadata insertion for the selection; cancellation is checked
between records and failure rolls back all new rows. Rendering remains bounded to
one execution window.

Do not fabricate recovery context for legacy receipts. Describe unrecoverable work with structured reasons and appropriate actions. The UI must explain the distinction before users rely on restart recovery. Keep transfer work and history visible across navigation and vault locking, while requiring the correct account/key for resumption.

## Consequences and evidence

Schemas 17–22 are additive and preserve supported existing data; independently versioned layouts and evidence limits are in [recovery records](../VAULT_TRANSFER_RECOVERY.md). Storage tests cover rollback, skipped upgrades, immutable identity, indexed bounded queries and stale-result rejection. Runtime acceptance tests use the real DesktopVault owners, SQLite and crypto with a synthetic wire transport, including mid-file pause/retry/cancel, restart, lost publication replies and account replacement. Separate child-process tests bypass graceful shutdown before reopening durable output. Native English/light compact and actual full-screen reviews cover recovery guidance and cleanup. This establishes the delivered macOS development behavior, not live Telegram or other-platform qualification.
