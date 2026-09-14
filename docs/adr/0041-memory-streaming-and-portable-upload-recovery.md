# ADR 0041: Memory streaming, portable pending uploads and receipt rates

Date: 2026-09-15. Status: accepted.

This supersedes ADR 0036's 60 MiB container target, new ciphertext spools and
package retirement solely because temporary Telegram parts expired. Its bounded
concurrency, immutable encryption identities and retained legacy readers remain.
The earlier [performance proposal](../UPLOAD_PERFORMANCE_PROPOSAL.md) is historical.

## Payload and preparation

The encrypted object ceiling is floor(1.9 × 2^30) = **2,040,109,465 bytes**,
including all framing and tags. The current aligned format's plaintext ceiling is
2,039,984,825 bytes. A maximum container contains 3,892 authenticated frames.
Full transport blocks are 512 KiB; the last can be shorter. The first block has
524,160 plaintext bytes after the 96-byte container header and 32-byte frame
header/tag; subsequent full blocks have 524,256 plaintext bytes.

Upload reads plaintext, encrypts in memory and sends through bounded queues.
Download requests bounded ranges, reorders within the concurrency window,
authenticates/decrypts each frame in memory and writes only authenticated plaintext
to a private partial file. Whole-container hashes and the final whole-file hash
must pass before atomic, non-overwriting publication. Verification may stream
remote ciphertext into a hash or decrypt-to-discard sink; it never stages it on
disk. Existing ciphertext spools are read-only compatibility inputs. New recovery
files contain small metadata, wrapped keys, hashes and receipts, never payload
containers. This rule also applies to retries and is recorded in `AGENTS.md`.

Whole-file and per-container BLAKE3 share one sequential source inspection pass.
Encryption subsequently rereads the source and rechecks each container's admitted
digest. A full-file baseline remains mandatory; this is not a one-pass source
snapshot algorithm. The first remote announcement can precede that inspection,
but payload encryption cannot. No cached/metadata-only identity substitutes for
content validation. Removing ciphertext staging reduces disk work without
weakening the mutable-source checks. No measured startup or throughput gain is
claimed by these deterministic tests.

## Portable recovery and publication

Before the source hash pass, a new upload publishes an authenticated `.tarku`
announcement marked incomplete and explicitly without a source hash. After
inspection it publishes a hash-known update, then monotonically retains the
published-container prefix. [Pending format v1](../PENDING_UPLOAD_FORMAT.md) is a
separate envelope; it can never be opened as a completed file manifest.

Another device must have the same source bytes, the recovery key and the same
Telegram account/storage channel. It authenticates scope, recomputes both hash
levels in one pass, imports the published prefix transactionally and streams each
reused remote container through cryptographic verification before trusting it.
Unpublished temporary RPC parts are not a portable receipt. Their lost in-memory
ciphertext is abandoned; a fresh part instance and publication identity are
reserved before re-encryption. Published containers retain their identities.
Known conflicting source hashes fail closed. Older announcements resolve against
newer authenticated prefixes. A stale remote card with an existing authenticated
completed manifest returns that completed file instead of uploading it again.

Pending source admission and import are fenced by generation and durable pause/
cancel intent. A tiny local locator preserves the remote announcement across
preflight restart. Import commits the pending promotion, executable context and
part receipts in one transaction. A failed/stale import preserves the pending
record. Reconstructed receipts are still verified against remote bytes; a database
row is not cryptographic proof.

Every pending update gets a fresh 256-bit salt in its own HKDF domain. The
announcement binds the account and channel in authenticated data. The message ID
of a successfully published update supplies a positive, channel-unique completed
manifest generation, avoiding reuse of a completed-manifest key/nonce across
handoffs. The completed-manifest outbox still seals once and replays its exact
bytes after an ambiguous send. Unsealed old manifest commitments retire the
package/File Key before any new seal.

Announcements are immutable; completion suppresses their logical listing only
after a valid completed manifest is found. Physical metadata messages are retained.
There is no remote distributed lock or automatic orphan-message deletion. A handoff
should pause the original device; concurrent writers can create equivalent
physical publications. They cannot use a different source hash to silently resume
a known file. Discovery remains bounded by the existing catalog/search policy.

## Rates and presentation

Current speed counts newly confirmed application payload: encrypted payload on
Vault transfers and file bytes on native downloads. It excludes manifests,
checkpoint metadata, transport headers and unconfirmed bytes. It is not NIC/TCP
bandwidth and does not prove durable remote publication.

Each RPC completion updates a shared in-memory receipt counter. Replayed prefixes,
duplicate replies and restored checkpoints add no new speed. Partial native
request retries count only the increasing prefix, and stale attempt callbacks are
rejected. Upload, checkpoint persistence and finalization have separate activity.
A checkpoint save never clears upload progress, and a blocked save does not stop
RPC acknowledgement accounting. At most four retiring checkpoint writes can own
blocking workers across the session.

A presentation owner publishes ordinary samples once per second using a trailing
three-second window. Fixed 50 ms receipt buckets bound memory and window-edge
quantization; scheduling jitter does not widen the window to four seconds. The
first usable sample and important state changes publish promptly. No new replies
still advances time: rates expire to zero and ETA becomes unknown. Outstanding
requests show waiting for confirmation and the last-confirmation age. Pause,
failure and account/attempt changes invalidate live rates immediately.

Per-account global rates are maintained by deltas. Stable row identities, changed
row overlays and cached filters/action scopes prevent full task/history scans on
ordinary presentation ticks. Only visible rows are formatted. Charts retain 96
samples; timelines retain 128 events with omission counters. A maximum-size
container's block map groups ranges into at most 128 rendered cells with explicit
range/confirmation tooltips. Rows retain the shared 24-point height and 12-point
text; batches use 34 points. Upload and download rows show processed/total bytes
and ETA with complete values available on hover.

## Compatibility and evidence

SQLite remains read/write 22 with its existing automatic 0–21 upgrade chain.
Executable recovery contexts read v1/v2 and write v2 for explicit geometry; v1
retains its original 60 MiB interpretation and exact frozen bytes. Pending source
admission, part reservations, part receipts, temporary upload bitmap, native
completion bitmap, key wraps and recovery bundles keep their versions. Completed
part/manifest readers retain 1.0 and 2.0; fresh payload writers remain 2.0. Unknown
newer versions remain intact and cannot be mutated by supported-version guards.

Deterministic evidence covers first-block delivery before whole-container
completion, authenticated plaintext output before the next block, corrupted-frame
rejection before output, blocked checkpoint persistence with continuing replies,
silence/jitter/restore rate behavior, stable history projections, frozen recovery
versions, real-crypto two-database handoff, wrong-source rejection and final byte
equality. Normal tests use synthetic data and fake remotes. Live Telegram limits,
network throughput, distributed-writer cleanup and other platforms remain separate
qualification work; no end-to-end speed claim follows from these tests.
