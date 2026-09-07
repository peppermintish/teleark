# ADR 0016: Bounded, visible upload preflight

- Status: Accepted
- Date: 2026-09-07
- Refines: ADR 0015's validation scheduling for multi-file upload batches; remote identity requirements remain unchanged.

## Decision

The desktop displays preparation feedback immediately after upload submission.
The Vault owner performs bounded source metadata inspection and publishes every
queued transfer plus the active cancellation owner before network preflight.
It checks complete remote uniqueness once per batch, then freshly verifies the
target channel's current owner/private configuration and pinned identity before
each file is read/encrypted. Single-file non-batch API calls retain complete
preflight. No SQLite binding grants or denies upload authority.

Discovery inspects at most four independent candidates concurrently, in bounded
groups within the existing owned request deadline. Futures do not outlive that
request; incomplete or failed checks still fail closed. Successful complete
preflight refreshes the adapter's peer addressing hints. Target-only checks use
those hints to address Telegram but always fetch current remote identity data.
This is not an identity cache or a persisted authorization lease.

A batch preflight failure becomes a visible terminal outcome for every queued
member. Cancellation requested during preflight prevents any file from starting
once the bounded validation call returns. Account switching is blocked during
submission, queued preflight and active Vault work. Runtime locks are not held
across validation or file upload.

## Consistency window and evidence

Global uniqueness is observed at batch entry. Target ownership, privacy and
identity are observed for each member. An independently created competing
channel during an already-running batch may be found at the next complete
preflight or setup; there is no Telegram atomic distributed lease. ADR 0015's
concurrent-create and app-origin proof limitations still apply.

Deterministic tests assert that queue rows and cancellation ownership exist
inside the injected validation boundary, validation executes once, shared errors
and cancellation remain visible, and the GUI renders feedback before any runtime
row exists. The existing remote codec/privacy/cross-account tests remain gates.
No database/manifest/crypto format or dependency changes are introduced.
