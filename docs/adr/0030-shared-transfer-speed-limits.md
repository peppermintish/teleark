# ADR 0030: Shared directional transfer speed limits

Status: accepted, 2026-09-10

## Context

Users need independent upload and download caps across concurrent files, including
ongoing encrypted transfers. Per-worker sleeps multiply the effective cap as
concurrency grows; copying policy into a task prevents live updates.

## Decision

Runtime owns one cloneable pair of frontend-neutral Telegram payload budgets for
its DesktopLibrary. Configured Telegram owners, their connections, and replacement
network owners share that pair. A cancellation-safe FIFO admission permit orders
waiters; a short state mutex protects credit and bounded telemetry, and is released
before every async wait. Directional policy changes notify sleepers. Zero bypasses
admission, preserving the unlimited default. Upload readers retain a single bounded
admission future; native and encrypted download loops reserve before network reads.
The opposite direction and control requests stay independent.

The version-1 optional integer settings default to zero for all supported old and
skipped-release upgrades. The storage owner applies live policy only after the
transaction succeeds and before acknowledging it, preserving concurrent save order.
Corrupt settings cannot silently start an unlimited configured network owner.
No encrypted-file, recovery or native-checkpoint bytes change meaning.

The frontend saves in the background and displays acknowledgment, saved/failed
status, a persistent global cap/wait strip, and a bounded directional event timeline.
An in-memory sampler has an explicit presentation role; no service scheduling or
repeated database reads occur in recurring UI callbacks. Existing task cancel
controls release pending admission. UI verification uses English only.

## Consequences and validation

Caps govern admitted payload bytes. Socket framing/retransmission and buffers
already admitted are outside the ceiling; bounded 512 KiB token bursts and transport
parts mean momentary displayed speed can exceed the configured average, particularly
at low rates. This scope is shown in the editor and documented in the transfer
contract. No new dependency or GPL implementation is used.

Controlled-clock tests cover aggregate concurrency, independent directions,
increase/decrease/unlimited changes, queued and active cancellation, bounded history,
short reads, unlimited bypass, idle burst bounds and protocol alignment. Temporary
storage tests cover legacy defaults, persistence, restart, route replacement,
failed preparation and malformed-value preservation. English GUI regressions cover
validation, saved feedback, saving reentry, compact/default controls and feedback
across navigation. Native preview review and release checks are recorded in the
implementation status.
