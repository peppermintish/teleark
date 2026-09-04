# ADR 0009: Version Native Part Maps and Transfer Session Logs

- Status: Accepted for the v0.3 alpha
- Date: 2026-09-05

## Context

Native Telegram downloads can complete one-mebibyte logical parts out of order. A single contiguous byte checkpoint cannot distinguish a durable completed range from an unwritten hole, and therefore cannot safely support parallel resume. Separately, adaptive controller decisions must remain inspectable after a transfer finishes; ordinary process diagnostics are intentionally lossy and are not a replay source.

Both artifacts persist local operational state. They therefore need explicit versioning, bounded parsing, privacy rules, and tests rather than inheriting Rust or Serde layouts.

## Decision

Each native download owns a private sibling partial and a private `TARKDPM1` part-map sidecar. The big-endian binary header records the exact logical part size (1 MiB), total byte count, part count, and bitmap length. The payload has one completion bit per logical part. Decoding rejects the wrong magic, unsupported part size, inconsistent counts or lengths, impossible trailing bits, and trailing bytes. A sidecar belongs only to the destination identity implied by its private path and the exact expected length. Explicit cancellation removes both files; pause, interruption, and retry retain them.

The adapter issues the Telegram hard-limit-compatible 512 KiB `upload.getFile` requests needed to fill each 1 MiB logical part, writes completed data at its declared offset, and then atomically replaces the owner-only sidecar. Final publication still requires every bit, the exact declared byte count, a flushed/synchronized partial, removal of the sidecar, and atomic no-overwrite publication.

Every real transfer also owns `Logs/Transfers/native-download-<id>.jsonl` or `Logs/Transfers/vault-transfer-<id>.jsonl`; the namespace prevents independent native and Vault ID allocators from colliding. Each line carries `schema: 1` and one structured event (`session_started`, `part_confirmed`, or `session_finished`). The log records only numeric timing, byte, queue, memory, part, controller, DC/lane, and structured result fields. It never records filenames, paths, captions, content, credentials, or key material. Creation and required initial writes are part of starting a transfer; failure is a structured persistence/capacity/permission error rather than silent loss. The GPUI inspector renders the typed in-memory projection in Live mode and traverses controller decisions in Replay mode; it also exposes the retained log path.

The process-wide daily diagnostic JSONL remains a separate, lossy, non-blocking facility and is not a replay contract.

## Consequences

- Native downloads can resume only missing 1 MiB ranges after out-of-order completion.
- Changing the logical part size or part-map layout requires a new magic/version and explicit import or safe restart behavior.
- Session log readers must branch on `schema` and ignore additive fields; an incompatible meaning requires a new schema number.
- Per-transfer logs consume bounded-by-transfer-history disk space and are managed under the user-selected Logs root. Retention controls beyond normal managed-root deletion remain future work.
- The current grammers API does not expose trustworthy physical media-DC connection identities to this adapter. Empty lane telemetry is displayed as unavailable, never fabricated; adding a custom media connection pool requires a separate reviewed transport change.
