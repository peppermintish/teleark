# ADR 0028: Upload selections without a file-count cap

Status: Accepted. Supersedes ADR 0013’s 128-file user limit and refines ADR 0016’s queue materialization contract.

## Decision

Both file selection and native OS file drops accept any file count. Dropped files append to the current draft; canonical duplicates are removed, invalid input preserves the previous draft, and folders are explicitly unsupported. Selecting/dropping files prepares the composer; confirmation and the existing unlock flow still control submission. Filesystem inspection stays on the background executor, with retained ownership, cancellation checks, visible timing and account-generation rejection.

The selected-file viewport materializes visible rows only. Total bytes are computed when the selection changes. One current draft and one admitted upload selection may coexist; path/source metadata and retry paths scale with that explicit finite input. File contents are streamed through the existing bounded encryption/transport pipeline, never loaded as a whole selection. There is no fixed application count quota.

Removing the old admission check alone would exceed the 256-member Vault history and silently fail to publish later queue rows. The frontend-neutral Vault owner therefore materializes internal windows of 128 files and processes one file at a time. Every window has actual batch membership and remains subject to whole-terminal-batch history eviction. One complete channel preflight covers the selection; per-file target checks remain mandatory. A single cancellation token and shared-error policy span every window. No later file starts after a stop/shared failure. Failure/cancellation results retain all affected source paths for explicit retry; scheduling errors after partial success preserve earlier successes.

Selection-wide progress is independent of retained transfer rows: queued/metadata/channel/upload/finished phases, inspected/succeeded/failed/cancelled/pending counts, phase time, last activity and a maximum five-entry timeline. The GUI presents it during background work on every route and retains the latest terminal summary on Transfers. Counts include every file; the UI explains that detailed history shows recent internal batches. Presentation timers only refresh measured times/progress and never drive the work queue.

`VaultUploadReport.completed_count` counts all successes. Recent successful manifest metadata is limited to 128 receipts and 16 MiB; the durable authenticated manifest catalog remains authoritative and is refreshed when immediate receipts are truncated. This avoids retaining every part map for a large selection. SQLite, encrypted codecs and recovery formats are unchanged; no dependency changes are needed. Existing empty-file and unreadable-source handling remains unchanged.

## Evidence

Deterministic native-drop event tests cover append/deduplication, invalid directories, repeated large payloads and no automatic upload. A blocked selection test covers immediate acknowledgment, overlap rejection and stale-account callbacks. A 10,000-row composer test verifies virtualization and reachable actions across three locales, two themes and compact/default windows. Runtime fixtures exceed both previous count and transfer-history bounds: 513 distinct sources, all-success execution, shared failure and cancellation after the first internal window, bounded receipts/history and exact complete outcomes. Progress tests cover cancellation before filesystem access and bounded history after 10,000 records. Tests use temporary synthetic data and fake upload/validation callbacks, not Telegram mutations.
