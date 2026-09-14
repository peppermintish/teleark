# ADR 0026: Session unlock and retained task key leases

Status: Accepted. Supersedes ADR 0025's lock-triggered health-check cancellation and the inactivity-lock behavior described in the previous preferences revision. Retained-key authentication and account isolation still apply.

## User behavior

An account session unlocks once. Navigation, window inactivity, native file selection, network reconnect and transfer completion do not lock it. Browsing unlock stays on the current page so the user can inspect files and notices before choosing an action. An upload intent returns to the preserved draft and requires explicit submission. Ordinary unlock has one password field; creation and password changes retain confirmation. Missing/unavailable Vault services do not offer initialization as a substitute for a failed configuration read.

Explicit lock immediately revokes session admission and hides decrypted file names, paths, identifiers and name-based search matches. It clears secret inputs and recovery text. Transfer phase/progress and supported stop controls remain visible. Already admitted uploads (including all members of a confirmed batch), downloads, scans and their internal retries continue with their existing authorization. A new or manually resubmitted operation needs a session key; it cannot borrow an older task's key.

## Ownership and bounds

`VaultSession` stores the current active and optional historical key under a short mutex. Admission snapshots the key references and matching wrapped-key metadata into an operation-scoped lease before returning a `VaultJob`. Admission performs no filesystem, crypto, database or network work. Only the background executor waits for completion. Each admitted transfer has a retained counter permit, so account-switch checks see queued work even before transfer rows have been prepared.

Three retained owners handle key operations, serialized encrypted transfers and catalog reads. Each has a bounded 16-command queue with explicit capacity failure. The transfer lane preserves the existing encryption memory budget and prevents independent large encrypted pipelines from multiplying it. Full health checks keep their existing single retained worker and bounded scans. Each lease retains at most the active and one historical key, not the entire historical key collection.

Lock clears session key references and advances a generation without awaiting any owner or slow dependency. Best-effort nonblocking wakeups clear idle presentation caches; a busy owner cleans up after its operation. Existing leases are not revoked. Owners drop operation key references after execution; full-health workers release their own references on completion/cancellation. A late unlock result cannot republish keys across a lock generation. Key revisions invalidate decrypted catalog caches when the active/historical key set changes, even if no remote catalog revision changed.

GUI key, upload, download and scan completion handles are retained separately. Lock never replaces a transfer handle or cancels its scan token. Upload receipts merge with the running scan instead of cancelling it. Account and session generations reject stale secret-bearing callbacks. Locked scan completions acknowledge completion while discarding decrypted rows; reopening the view reconstructs them from authenticated inventory. Transfer projections redact visible rows without suppressing state transitions or formatting every retained row during idle rendering.

Account switching remains a separate explicit action: the UI waits for admitted encrypted transfers to finish or be stopped before switching, stops old-account synchronization, revokes session access and rejects old-account callbacks. This does not introduce durable pause/resume for the encrypted transfer format; existing supported controls and recovery paths retain their scope. Process exit cannot keep an in-memory task running.

## Compatibility and evidence

No persistent schema or encryption codec changes. SQLite remains read/write schema 15 with its supported automatic upgrades; manifest and recovery bundle versions are unchanged. The old `lock_vault_when_hidden` boolean remains readable/writable with its original bytes but is deprecated and no longer drives desktop behavior. New preferences default it to false. System biometric/device-key integrations remain outside this change.

Deterministic tests use held transfer work to prove lock and password unlock complete independently, old queued leases remain usable, locked new admission fails and task references are released independently of the session. Additional tests cover stale unlock completion, historical-key upload restrictions, GUI task/token retention, upload draft preservation and locked file-name/path/search redaction with progress intact. Native layout verification uses synthetic preview data only; it is not live Telegram interoperability evidence.

The application-facing lock is superseded by [ADR 0037](0037-application-pin-and-transfer-drain.md). Application PIN locking retains the runtime key session; cryptographic session revocation remains separate.
