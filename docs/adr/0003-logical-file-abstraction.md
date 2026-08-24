# ADR 0003: Make Logical File the Primary Abstraction

- Status: Accepted
- Date: 2026-08-24

## Context

One user file may be a native Telegram document or an encrypted package containing a manifest and many application parts, each transported through many MTProto units. Exposing these internal layers would force users, collections, search, and transfer history to manage remote mechanics and would spread Telegram identity through the product model.

## Decision

The primary domain and UI entity is `LogicalFile`. A native file maps to one `RemoteObject`; an encrypted multipart file maps to a `Package`, one authoritative `Manifest`, ordered `FilePart` records, and their `RemoteObject` records. Application parts and MTProto parts remain distinct.

Search results, collections, details, and transfer tasks reference logical-file identity. Part/message/locator data is visible only in appropriate diagnostics and never required in normal workflows.

## Consequences

- The UI consistently presents filename, logical size, encryption, upload, and verification state.
- Multipart/native storage differences stay behind Core APIs.
- The data model must enforce size/range/part/manifest invariants.
- Transfer progress rolls part-level state into a logical task without discarding diagnostic detail.
- Remote deletion/recovery/reconciliation need explicit mappings rather than treating a Telegram message as the file.
- Future storage transports can potentially map to the same model without changing library semantics.
