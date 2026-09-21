# ADR 0048: Native download connection failover and part diagnostics

Status: Accepted. Extends the native download behavior in ADR 0009 and the bounded session-log contract in ADR 0022.

## Context

Four failed native downloads had eight transfer connection slots. Their retained session logs recorded 104 part attempts lasting about 60 seconds, all on the same slot (`part_index % 8 == 5`), including each terminal fourth attempt. The adapter assigned a part to `part_index % connection_count` on every retry. A stalled slot therefore made every part mapped to it fail after four deadlines, even while other slots completed work. The part event and durable failure projection retained only `Network`, so the right-hand inspector could not explain the timeout or identify the repeated slot.

## Decision

Native download retries start at the next local transfer connection slot. A slot that reaches the measured 60-second request deadline is skipped for later parts in that download; if every slot times out, the existing bounded retry policy still applies. No Telegram rate-limit deadline is shortened. Completed part bitmap entries, output identity and account checks remain unchanged.

Part retry and failure events carry a structured, sanitized cause and local slot index. The inspector presents the cause, part, attempt, slot and elapsed time at the top of the failure details and labels each timeline event with its assigned slot. These slots are local client-pool indices, not claims about Telegram DCs or physical network connections.

Native `part_state` session-log records use schema 2 for the new fields. Schema-1 records keep their original interpretation. On restoring a failed task, the runtime reads a bounded tail of its private log and takes terminal part evidence only from the latest session. Older logs supply part, attempt and elapsed time while labeling cause and slot unknown. Missing, malformed or unavailable logs leave the existing generic failure category; they never block transfer history. No SQLite or partial-file codec migration is needed.

## Verification and limits

Deterministic tests cover slot failover and quarantine, retry bounds, localized cause mapping, the compact failure inspector, and schema-1/schema-2 log restoration boundaries. The production failure was diagnosed from sanitized local session logs. A real Telegram retry with the affected account remains a separate user operation; the change does not claim to repair the underlying stalled network path.
