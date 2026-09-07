# ADR 0014: Automatic private-channel management

- Status: Accepted
- Date: 2026-09-07
- Supersedes: the user-directed channel setup and binding-selection policy associated with ADR 0012. Its ownership, provenance, discovery limits and durable encodings remain in force.

## Decision

TeleArk owns setup of the current account's private storage. The desktop exposes
progress, results and guidance, without Create, Rediscover or candidate-selection
controls. Entering the workspace or refreshing account sources starts management.

Runtime performs a complete bounded discovery and independently validates the
account, creator, private-broadcast metadata and exact description marker. It
reuses a valid saved identity even after a rename. If that identity is absent,
it chooses the validated candidate with the lowest numeric channel ID, regardless
of title or dialog order. With no validated candidates after successful complete
discovery, it creates one private channel. Missing access or an incomplete scan
is an error, never evidence that creation is needed.

A successful result persists the existing account-scoped v1 binding. Result
metadata tells the frontend whether a channel was found, created or substituted
for an unavailable binding. Substitution is automatic but explained visibly.
Other channels, memberships, messages, files and remote titles are not modified;
this policy does not merge packages from multiple channels. Existing files in an
unavailable or unselected channel are not moved into the newly selected one.

## Bounded execution and uncertainty

The serialized Telegram owner retains the existing 60-second request bound.
The frontend retains the operation and retries transient network/conflict errors
at most twice, after 2 and 4 seconds. Retry callbacks validate account and login
generation; an account switch cancels pending timers. Reentering the workspace
can begin another discovery episode. There is no automatic unbounded loop.

Before issuing the non-idempotent create RPC, the owner records an in-memory,
account-scoped creation guard. A dropped future, timeout or error leaves the
guard set. Further attempts can discover the remote result but cannot issue a
second create while uncertainty remains in that process. A successful creation
or subsequent validated discovery clears the guard. On process restart the
normal complete discovery runs before any creation; the guard itself is not a
durable exactly-once guarantee. Cross-process ambiguity and long-delayed remote
visibility require protected real-system qualification.

## Compatibility and evidence

No new persisted bytes, marker, migration, dependency, crypto or manifest format
are introduced. The read-only discovery API retains its explicit ambiguity
result for non-desktop consumers; desktop setup uses the automatic manager.
Deterministic tests cover saved/renamed identity preference, stable selection
under reordered candidates, unavailable bindings, absence, per-account creation
guards and resolution after uncertainty. Existing binding-codec, account,
ownership/private-marker and bounded-discovery tests continue to apply.
