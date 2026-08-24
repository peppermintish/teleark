# ADR 0001: Separate Core from the GPUI Frontend

- Status: Accepted
- Date: 2026-08-24

## Context

TeleArk needs a pixel-faithful GPUI desktop app now and a reusable future CLI or alternate frontend. Indexing, Telegram integration, SQLite, transfers, crypto, manifests, and recovery are long-lived business capabilities. Embedding them in GPUI views would couple correctness and testing to one event/render model, leak presentation types across the system, and make a future frontend a rewrite.

## Decision

Define frontend-neutral domain/application Core APIs and one-way adapter boundaries. GPUI owns presentation and invokes Core commands/queries; it consumes project-owned snapshots/events. Core, Storage, Telegram, Crypto, Index, and Transfer code may not depend on GPUI or its types. The GUI may not issue SQL, call `grammers`, perform crypto, manage sessions, or own transfer/index checkpoints.

Infrastructure adapters implement Core-facing ports and map external types/errors at their edge. `grammers` types remain in the Telegram adapter; SQL remains in the storage adapter; localization returns ordinary frontend-neutral text/data.

## Consequences

- Core can compile and test without a GUI runtime and can serve a future CLI.
- GUI work can start with explicit mock implementations while backend contracts evolve.
- Commands/events and DTO mapping require deliberate design and some adapter code.
- Background work needs a clear GPUI/Tokio handoff and event throttling.
- CI must eventually include an independent `teleark-core` test proving no GUI dependency.
- A framework change should affect the frontend crate, not durable business/format logic.
