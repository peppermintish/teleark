# ADR 0051: Retry deleted outputs from a completed native batch

- Status: Accepted
- Date: 2026-09-22
- Extends: ADR 0013's explicit native re-download and ADR 0050's fresh observations

## Decision

The transfer toolbar and native batch header offer Retry when their scope includes
a completed download whose current local observation is Missing. Failed/stopped
members retain their existing retry behavior. Existing, changed, unavailable and
still-checking outputs do not become automatic batch re-download candidates.
Partial selections stay scoped when collapsed; selecting a header and its children
never dispatches a member twice.

Runtime rechecks selected completed destinations off the UI thread immediately
before preparing replacements. It validates ownership and historical completion,
deduplicates task IDs, preserves source batch/channel boundaries and creates a new
batch with new task identities and exclusively allocated destination reservations.
The original completed batch and every existing file remain intact. A file restored
since the GUI observation is skipped. Cancellation and account checks surround
filesystem work; no scheduling lock is held while checking a path. Failed preparation
releases only unclaimed empty reservations, preserving durable tasks and unrelated
encrypted recovery data.

The GUI acknowledges preparation through its retained action owner, then selects
the replacement batches in Downloads. Errors use the existing localized action
feedback. Availability revisions invalidate cached action scopes; unchanged
observations reuse projections and batch target indexes, including idle frames.

## Compatibility and evidence

SQLite remains read/write schema 22 with automatic upgrades from 0–21. Native
cleanup codec 1 and all payload/recovery codecs are unchanged; no migration is
needed. This extends native download batches; Vault upload retry is unchanged.

Synthetic regressions cover mixed missing/existing batches, duplicate selections,
recreated/changed/unavailable files, preserved history, account rejection,
cancellation during a blocked check, admission failure and reservation cleanup.
GUI checks cover partial collapsed selections, availability-driven cache changes,
and English light-mode layouts at 900×600 and actual native full-screen.
