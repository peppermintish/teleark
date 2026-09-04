# Diagnostics and Performance Tracing

Status: process diagnostics are implemented for desktop startup, runtime
storage, Telegram, native download, and encrypted transfer operations. Real
transfers additionally write schema-versioned session logs for complete
controller replay; Index-engine event coverage remains incomplete.

## User-visible behavior

TeleArk writes newline-delimited structured JSON diagnostics to the managed
files root's `Logs` directory. Settings shows the active directory, reports how
many events were dropped by the bounded writer, opens the directory in the
system file manager, and explains which metadata may be present.

Native download details expose the matching `DL-<task id>` trace identity,
live transferred bytes, current speed and ETA, queue wait, elapsed time,
average completed speed, created/started/finished times, attempt count,
verification outcome, a bounded lifecycle timeline, and a localized failure
reason with retry/user-action guidance. Pause, resume, cancel, retry,
interruption, and checkpoint failures are separately traced. Transfer failure
and verification failure are separate: a network or permission failure says
that verification was not reached.

The transfer inspector has Live and Replay presentations for typed controller
telemetry. It shows C/W/F/P/E/Qe, goodput, encryption and disk rates, estimated
BDP and target inflight bytes, buffer/memory pressure, part states, bottleneck
classification, soft-limit and FloodWait outcomes, and the before/after/reason
for every retained decision. Unknown physical media-DC lanes are shown as
unavailable rather than filled with preview data.

## Transfer session logs

Every real native or Vault transfer creates an owner-only
`Logs/Transfers/native-download-<id>.jsonl` or
`Logs/Transfers/vault-transfer-<id>.jsonl` file before data movement. Schema 1 uses
newline-delimited `session_started`, `part_confirmed`, and `session_finished`
records. Writes are flushed at decision boundaries so a completed task can be
replayed even when the general diagnostics writer dropped unrelated events.
The format, bounds, and compatibility rules are governed by ADR 0009.

Session logs contain numeric IDs, offsets, sizes, rates, timings, memory/queue
counters, parameter transitions, and structured enum names. They do not contain
source or destination paths, filenames, captions, content, secrets, or raw
adapter error text. The GUI may show the local log path because it is already
user-owned local metadata; the path itself is never written into the log.

## Writer policy

- Daily UTC rotation, newline-delimited JSON, 15 retained files.
- Dedicated non-blocking writer thread with a 4,096-line bounded queue.
- When the queue is full, diagnostic events are dropped instead of blocking a
  storage, Telegram, transfer, or GUI owner; the cumulative dropped count is
  visible in Settings.
- The process retains the writer guard so buffered events flush on normal exit.
- The subscriber accepts only `teleark` and `teleark_*` tracing targets;
  unreviewed dependency events are excluded even at DEBUG level.
- Moving the managed root moves diagnostics on the next application start;
  Settings shows that restart requirement while the current subscriber remains
  active.
- The JSON field set is diagnostic output, not a stable public or durable data
  format. Tools must tolerate added or omitted fields.

## Safe field policy

Allowed fields include operation/event names, crate target, source line,
thread identity, application version, task/chat/message numeric IDs, byte
counts, attempts, queue wait, duration, average rate, verification class, and
structured error category/disposition.

The following must never be recorded: API Hashes, Telegram session data, QR
links/tokens, phone numbers, login codes, passwords, recovery or encryption
keys, filenames, captions, channel titles, local paths, file content, plaintext
buffers, or unreviewed adapter error strings. New tracing fields require a
privacy review and deterministic regression coverage where practical.

## Event families

```text
diagnostics.initialized
application.starting
storage.operation.completed | storage.operation.failed
telegram.operation.completed | telegram.operation.failed
transfer.download.queued | started | paused | resumed | cancelled
transfer.download.completed | failed | interrupted | checkpoint_failed
```

Storage and Telegram operation events contain only a fixed safe operation name,
elapsed milliseconds, and structured result kind. Authentication operations do
not record their phone/code/password/API Hash arguments. Download events omit
the filename and destination even though those values are available in the
local GUI snapshot.

Interactive `scan_file_page` operations are bounded to 200-message transport
chunks. Completion/failure timing therefore identifies a slow individual
network chunk instead of hiding a multi-minute 5,000-message aggregate.
Timeout and user cancellation terminate the adapter future and are recorded
through the existing structured failure event without source content.

## Dependency decision

The implementation uses Tokio's `tracing 0.1.44`, `tracing-subscriber 0.3.23`,
and `tracing-appender 0.2.5`. The latter two are direct MIT-licensed,
maintained dependencies. JSON formatting uses the subscriber's documented
`json` feature; the appender supplies documented bounded non-blocking writes,
daily rotation, retention, and a retained flush guard. No GPL `ztracing` or
`zlog` code/dependency is used.
