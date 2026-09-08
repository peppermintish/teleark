# ADR 0021: Bounded Telegram request lanes

Status: Accepted, 2026-09-08.

## Decision

The retained Telegram worker dispatches at most eight read operations, four transfer operations and one mutable control operation. Thirty-two additional requests can wait inside the dispatcher, alongside the existing bounded ingress channel. A saturated lane cannot consume another lane's execution slots. Excess pending requests return the existing typed capacity error; admission never stops consuming login, logout or shutdown requests.

Read operations share the retained connection and an immutable chat-map snapshot. Only the control lane publishes a replacement map or changes login state. The single control owner retains the storage-creation ambiguity guard even when its network future is cancelled. Fresh remote private-channel identity validation remains required.

Login and logout requests form session barriers. They cancel and drain active read/transfer tasks, cancel the current control operation, discard pending older requests and block new reads until the barrier completes. Interrupted callers receive an error through their dropped reply channel; they cannot receive a late successful reply from the abandoned operation. Existing frontend account/request generations remain required. Closing the request owner drains every lane before the runtime exits. No extra sessions or network clients are created by the dispatcher.

## Consequences and evidence

This supersedes awaiting every complete file transfer in the single Telegram command loop. Long downloads and metadata discovery can coexist with channel reads. Concurrency is bounded independently of caller count; it does not promise a Telegram bandwidth gain or bypass flood waits.

Controlled futures exercise the production dispatcher with all transfer slots blocked and a blocked metadata operation, queue saturation, login admission, preservation of a cancelled control mutation, rejection of old replies, deferred new reads and owner shutdown. See [the performance audit](../PERFORMANCE.md). Live Telegram throughput is outside these deterministic tests.


## Native task retirement corollary

The Runtime native-download queue follows the same isolation principle. Its global scheduled-ID lock protects brief publication and admission decisions; never retain it across acknowledged storage writes or backend cleanup. The scheduled ID itself retains ownership through retirement. A retry accepted during that ownership queues intent for the retained worker, which completes old writes/cleanup before persisting and starting a replacement. For an idle task, reserve its identity during acknowledged queued persistence so refill cannot overtake it. Abrupt loss before a pending intent is persisted retains the prior durable task state; it must not promote an incomplete file or allow concurrent owners. Controlled blocked-cleanup/Storage-reply tests cover this ordering (performance finding 17).
