# ADR 0029: Durable upload history

Status: accepted (2026-09-10)

## Problem

Connected Vault upload rows lived only in the transfer snapshot store. Closing the application discarded completed, failed, cancelled and queued rows, leaving Transfers → Uploads empty at the next launch. A remote manifest proves that a package exists; it does not prove which local operation or account session uploaded it. Sanitized transfer logs intentionally lack filenames and account identity, so they cannot safely rebuild those lost rows.

## Decision

Schema 16 adds account-scoped `vault_upload_history`. Runtime acknowledges each bounded admission window and each file's start and terminal summary through the existing Storage owner, before proceeding past that durable boundary. The final completed state is published only after persistence acknowledges it. Storage writes are transactional; network progress callbacks continue to update memory without SQL or acknowledged cross-owner calls. A failed save is visible and ends the remaining admitted queue window instead of leaving it waiting indefinitely.

At service construction, persisted queued/running states become `interrupted` in one idempotent update. This never resends content: Telegram publication and the local commit cannot be atomic, and blind resending could duplicate a package. The interface explains the interruption and directs the user to check Storage and select the original file for a new upload. This supersedes the memory-only upload-history limitation in ADR 0013; resumable encrypted transfers are still not implemented.

On account entry, a retained history job is submitted in frontend generation order and awaited in the background before remote dialog loading. Its phase, elapsed time and failure/retry use the existing visible catalog activity owner. This works while the Vault is locked and when the network is unavailable. A stale waiter cannot submit after a newer account request; UI results also retain the existing generation check. The revisioned snapshot replacement preserves newer live observations.

The view loads the latest 256 records for that account and extends the boundary to retain its entire batch (maximum 128 members). Older records stay in SQLite and are counted in the footer. Rendering stays bounded; active rows are preserved during retention. The account/sequence and account/batch/sequence queries have actual query-plan regressions. This release does not add browsing of older upload pages.

## Data and privacy

The explicit schema stores operation/account/channel/batch IDs, original filename, package ID, plaintext size, confirmed bytes/parts, queue/start timestamps, duration, average throughput and locale-neutral state/failure codes. State and error codec v1 is independent of the app version. No source path, plaintext content, key or Telegram credential is added. Local SQLite is not encrypted: original upload names and sizes are visible to someone who can read its files, like the existing download inventory. Locking continues to hide names in the interface.

Restored rows expose saved totals. They do not fabricate live connections, charts, rates or controller history. An existing sanitized per-transfer log can still be opened when present in the configured log directory. Earlier application versions never saved these upload summaries; migration cannot reconstruct already-lost local history from remote packages or logs. Remote files and encrypted format readers are unchanged.

## Compatibility and evidence

Automatic transactional upgrades support SQLite schemas 0–15 to read/write 16, including skipped versions. Failed migration leaves the previous version, key wraps and original data intact; reopening retries the missing step. Newer schemas are rejected without modification. Existing preferences, crypto, manifest, recovery, native checkpoint and session-log codecs retain their versions.

Deterministic tests cover service restart while locked, account switching, complete/interrupted/failed/cancelled summaries, stable batches, retained live observations, unknown error codes, bounded history, query plans, transactional failure, migration rollback/retry with wrapped-key preservation and a blocked persistence acknowledgment that does not stop another upload's observer. Native previews use synthetic data and English by default.
