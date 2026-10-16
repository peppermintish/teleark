# ADR 0065: Managed file progress, batch restore and reserved synchronization

Status: Accepted — 2026-10-05. Supersedes ADR 0031's shared four-channel execution limit. Retains account barriers, automatic synchronization and all encrypted formats.

## Problem

The managed Files footer reused successful key-operation feedback while synchronization, manifest reads or inventory application were still pending. Individual restore actions did not support selected or filtered batches. Four blocked normal channel calls could exhaust synchronization capacity, and a separate Telegram read limit could still delay the managed channel after adding a coordinator slot.

## Decision

Publish PTS checking and difference polling before expensive work. Derive managed readiness from the account/channel-scoped catalog, active synchronization and manifest phases, key selection and applied inventory. File-local errors remain visible. A generic Vault success never implies that all inventory work is complete. Retained one-second presentation timers update phase duration and last activity only while actual work waits; they do not poll Storage or drive queues.

Reserve one managed-channel execution beside four normal channel executions and one directory execution. Select runnable work without letting a saturated normal lane hide eligible managed work. Reserve one Telegram managed read and one managed pending admission beside eight ordinary reads and 32 ordinary pending requests. All lanes share the same account replacement barriers; old requests cannot publish across them. Owners and queues remain bounded and retained.

Checkbox identity is the package ID in a managed-specific selection namespace. Missing, unreadable or pending files cannot be selected for restore. Date/type/search projections are cached by source revision and filter values; selection survives filtering and is pruned after inventory changes. Download selected and download matching use the existing default download location and authenticated restore API. Filenames and completed destinations remain intact.

Runtime accepts at most 65,536 unique positive package IDs per batch and runs each file through the existing streaming restore path. This bounds the ID selection to approximately 512 KiB, independently of payload size. It retains incremental counts and a 64-event timeline with explicit truncation, publishes admission/execution/terminal states, and retains failed IDs for explicit retry without copying them in every progress sample. A batch occupies one existing download worker; the shared download admission gate remains authoritative. Individual transfers retain their charts, part maps, logs and pause/cancel/retry controls. Navigation retains the active batch and exposes its status globally.

Stop remaining acknowledges stopping before the next file, while the current file finishes safely using ordinary transfer controls. Account/key replacement fences future files and callbacks. Failed files can be retried as a new selection; successful outputs are never overwritten or removed. Batch selections and summaries are session-owned; started transfers keep their existing durable recovery metadata. No new schema/codec is introduced and no existing bytes change meaning: SQLite read/write 24 and automatic upgrades 0–23 to 24, part/manifest readers 1.0/2.0 and desktop writes 2.0, key/recovery formats 1.0 and existing pending/transfer codecs remain supported.

## Verification

Deterministic tests block every normal synchronization/Telegram read lane while a managed call completes, then apply account barriers to reject old replies. Tests cover feedback before RPC entry, delayed catalog/display readiness, checkbox propagation and eligibility, cached filter boundaries, bounded timelines, failed-only retry, stale scope and stopping a blocked current restore. Service composition uses real crypto, temporary SQLite/files and a synthetic Telegram remote to compare verified batch destinations byte for byte. English/light previews and interaction tests cover 900×600 and actual native full-screen. Tagged native platform and packaging gates qualify distribution; synthetic tests do not establish live Telegram performance.
