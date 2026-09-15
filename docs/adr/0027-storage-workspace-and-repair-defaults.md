# ADR 0027: File-first storage workspace and required repair defaults

Status: Accepted. Supersedes ADR 0025’s separate optional mute/archive action.

## Decision

A healthy, idle private channel appears as a small connected status immediately before Help in the storage header. Activating it opens a bounded, independently scrolling dialog with channel identity, management details and retained maintenance history. The ready workspace reserves no height for that content; Files and Raw Files receive the remaining height. Pending, failed and repair-required work stays visible in the workspace, including while the vault is locked. Closing details does not cancel background work.

Users can inspect the complete repair steps and confirm or cancel the repair. They cannot select individual operations. Repair validates the original bound channel, restores its identification record, pin and description reference, then applies channel notification muting and archived folder placement. It reads back both settings before reporting success. All three locale catalogs describe this contract. No global Telegram archive preferences are modified.

Mute/archive also runs when identity is already healthy on a retry after interruption. Identity failure prevents these mutations. Cancellation and failures retain the repair token; only success of the entire operation clears it. Existing account isolation, safety checks, bounded ownership and visible phase history remain in force. Every explicit repair reapplies these defaults; this decision does not introduce periodic network reconciliation or change persistent schemas/codecs.

## Validation

Deterministic tests cover remaining file-list height, stable geometry while details open, dialog dismissal, compact/default windows, three locales and two themes. Adapter orchestration tests cover blocked identity/defaults, failure propagation, retry with healthy identity and cancellation before defaults. Live Telegram mutation qualification remains separate from synthetic tests.
