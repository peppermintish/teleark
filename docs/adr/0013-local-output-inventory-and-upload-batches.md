# ADR 0013: Local output inventory and bounded upload batches

- Status: Accepted
- Date: 2026-09-07
- Scope: desktop usability, local SQLite schema 10 and retained Vault owner

## Context

A completed transfer is historical evidence; it cannot prove its destination still exists. Native history already survives restart, but restored Vault outputs had no durable local identity. Multi-file uploads also need honest queued members and bounded ownership, rather than presentation-only groups or one spawned worker per file.

## Decision

Add `vault_downloaded_files` in schema 10 using the existing explicit platform path codec. Scope records by account and preserve package/chat identity, destination, size and completion time. Project completed native history directly instead of duplicating or guessing older records. Read at most 128 outputs per cursor page. Current presence is a Runtime filesystem observation, separate from transfer state, persisted crypto evidence and remote recovery authority.

Register restored Vault outputs after verified, atomic non-overwriting publication and before reporting success. A failed registration leaves the output intact and reports persistence failure; a crash in that interval may leave an unregistered file. Do not infer completion by walking arbitrary directories. Pre-v10 Vault downloads cannot be backfilled without additional evidence.

A retained observer checks bounded pages off the UI thread, prioritizes recent outputs, rejects stale account results and bounds its presentation cache. Missing, changed-size and unavailable paths are distinct. No automatic download, file deletion or history mutation follows a missing-file observation. Explicit native re-download allocates a new task and safe destination. Same-size replacement is outside metadata detection.

Multi-file upload accepts at most 128 nonempty source files, deduplicates canonical paths and validates size/mtime again before each file. Queue snapshots belong to the existing single Vault owner and record real membership. Source-specific failures may continue; shared authorization, permission or network failures stop further file starts. Stop requests act at file boundaries, leaving the current transfer's key/manifest lifecycle intact. Tasks remain memory-only; this is not durable encrypted pause/resume.

Preserve Core/GUI isolation: Storage owns SQL, Runtime owns metadata/volume probes and transfer policy, and GUI owns presentation/focus. Crypto, manifest, recovery-bundle, native bitmap and session-log encodings do not change.

## Alternatives and consequences

Filesystem watchers alone cannot reconstruct state after restart or reliably distinguish offline volumes. Rewriting Completed on external deletion would destroy historical meaning. Both are rejected. A future watcher may accelerate the same authoritative metadata check without replacing it.

The local Vault inventory exposes restored names/paths and package/account relationships in plaintext SQLite; document this with the existing local-data threat model. Removing a file does not erase its record. Inventory cleanup, arbitrary directory rediscovery, pre-v10 Vault backfill and crash reconciliation remain future work.

Regression evidence covers external deletion/recreation/size changes, unavailable parents, account-isolated pagination/reopen, literal path-format bytes, preserved native history, re-download account checks, source preflight and batch stop/failure policy. GPUI wheel-event tests separately prove inspector content moves without scrolling its underlying list at either boundary. Protected Telegram and crash/system qualification remain release gates.
