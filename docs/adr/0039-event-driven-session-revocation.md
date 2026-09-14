# ADR 0039: Event-driven Telegram session revocation

Date: 2026-09-15. Status: accepted.

## Decision

When Telegram invalidates the logged-in session, pause recoverable work and return
to sign-in with the reason retained in the lower-left status area. Do not quit the
process or automatically resume paused work after the next login. This extends the
pause/settlement protocol in ADR 0038; ordinary manual quit retains its confirmation.

The Telegram adapter owns an authorization event projection. A primary connection
closure or a typed authorization error on an ordinary RPC requests one home-DC
`updates.getState` confirmation. Only a 401, `AUTH_KEY_DUPLICATED`, or MTProto 404
from that home-DC confirmation latches session loss. A secondary file DC's missing
authorization is only a hint. I/O failure, disconnect alone, permission errors,
rate limits, server errors and the confirmation's 10-second timeout do not log out.

There is no periodic authorization probe or automatic probe retry. A retained async
owner sleeps on watch notifications, coalesces duplicate hints during one check,
and never blocks other request lanes. The confirmation uses a separate logical
client over the existing primary sender pool, with `NoRetries`; its own RPC failures
do not feed back into the hint source. Consecutive transport-closed notifications
coalesce until an incoming update or a successful confirmation shows connectivity.
Adapter `RetryPolicy` wrappers delegate the original policies after observing typed
failures, preserving normal transfer and synchronization retry behavior.

Authorization snapshots carry account and connection generations. Revocation stays
latched through pause and failed local cleanup; late authentication replies cannot
restore the session. Ordinary account RPC admission closes immediately, while the
local lifecycle account identity remains available for durable pause controls. Route
replacement and local retirement serialize, then fence and join the old reactor
before clearing its invalid session. Cleanup retries require the same revoked account
generation. A new login can start only after a fresh endpoint is published.

## Pause and presentation

The GUI subscribes to the retained authorization state, including startup and login
reply races. It invalidates stale callbacks, closes auxiliary work surfaces, stops
scans/synchronization and blocks account, proxy and workspace actions while pausing.
It paints the reason before dispatching native and encrypted pause requests to
independent retained background owners. A blocked native writer cannot delay delivery
of the encrypted pause request. Existing indexed durable-queue passes include tasks
not materialized in the UI. Partial files, encrypted checkpoints and recovery data
remain intact. Local key locking follows both pause acknowledgments.

After pause, retire the old transport, atomically replace only the revoked Telegram
session with fresh v1 session data, and return to sign-in with a fresh QR flow. No
logout RPC is needed for a remotely invalidated key. Failure preserves a blocked,
retryable screen; no automatic loop or reopening of admission occurs. Only the
current manual-quit GUI callback may reopen native admission after an abandoned quit,
so a stale quit worker cannot undo session-loss pausing. An explicit Quit during
session-loss handling closes the app only after the same successful barrier.

The lower-left reason stays visible while pausing, retiring, on failure, and after
returning to sign-in. It clears on a subsequent successful login. Active steps show
elapsed time since the latest phase change and an expandable, bounded 16-entry activity timeline;
older entries are counted when omitted. A one-second **presentation-only** timer
updates that duration while pausing/retiring, then stops. It never checks Telegram,
drives work queues or reads the database. English/light preview fixtures are
`session-loss-pausing`, `session-loss-retiring`, `session-loss-failed` and
`session-loss-paused`.

## Compatibility and evidence

No persistent format changes: Telegram session v1, existing native task codecs and
Vault job/context codecs keep their established meanings and automatic migrations.
Unknown/newer sessions remain intact and report a cleanup error. A failed atomic
session replacement preserves the original; retry is idempotent. No key epochs,
account indexes, partial files or recovery bundles are removed.

Deterministic tests cover silence, blocked confirmation, duplicate/stale events,
secondary-DC false positives, typed loss versus network failures, failed cleanup,
key removal, retained generations, stale GUI completions, retry, return to sign-in,
bounded history and small/full-screen layouts. Existing native partial-file and
260-row hidden Vault queue tests exercise the reused durable pause barriers.
Live mobile revocation remains an integration qualification; ordinary tests use
synthetic data and fake remotes only.

References: [Telegram RPC errors](https://core.telegram.org/api/errors),
[`updates.getState`](https://core.telegram.org/method/updates.getState),
[MTProto transport errors](https://core.telegram.org/mtproto/mtproto-transports#transport-errors),
and the MIT OR Apache-2.0 public grammers 0.10 APIs. No Telegram client implementation
under an incompatible license was consulted.
