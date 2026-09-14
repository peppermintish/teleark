# Upload performance proposal

Status: historical proposal, superseded on 2026-09-15 by
[ADR 0041](adr/0041-memory-streaming-and-portable-upload-recovery.md). Current
behavior uses 1.9 GiB encoded containers, memory-only payload streaming, portable
pending descriptors, one-second rate publication and a three-second window.
Whole-file/container hashes share one inspection pass; a fully one-pass mutable
source upload remains unimplemented. The original recommendations below are
retained as design history, not the current contract.

## Recommendation and order

1. Use one frontend-neutral acknowledgement-rate projection for upload rows,
   totals and inspectors. Measure preparation separately.
2. Optimize the existing source-admission pass without changing its guarantees.
   Introduce bounded overlap only after independently versioning provisional
   content commitments and specifying their failure/recovery behavior.
3. Qualify a stable-source fast path before offering one-pass source hashing,
   encryption and upload. Preserve the conservative path where stability cannot
   be established. Do not equate metadata equality with content equality.

The existing 512 KiB wire geometry and 60 MiB publication containers need not
change. More connections, larger plaintext buffers or faster GUI timers do not
address the two identified startup bottlenecks.

## Evidence in the current implementation

- `vault/source_digest.rs` reads the entire file in a bounded 1 MiB buffer,
  updating both whole-file and container BLAKE3 states before upload admission.
- `vault.rs` starts streaming only after that pass and durable context admission.
  It updates the displayed average and controller goodput after container
  publication. `screens/transfers.rs` uses these values for rows and totals.
- `vault_progress.rs` already observes part acknowledgements, bounded rate
  samples and retry events. Its presentation counters are not recovery receipts.
- `transfer_updates.rs` already coalesces ordinary publications and reuses
  revisioned views. More UI polling would not fix the measurement source.
- Recovery context v1 requires a complete source digest before remote work.
  Part reservations bind their expected digest and immutable encryption identity.
  Upload telemetry cannot replace these durable records.

## Acknowledgement rates and display cost

Define current upload speed as newly acknowledged ciphertext payload per elapsed
monotonic time. It is application-level confirmation throughput, not measured
TCP/interface bandwidth and not proof of final publication. Telegram saves parts
temporarily before a separate publication request; see the
[official file API](https://core.telegram.org/api/files).

Keep separate counters for acknowledged payload, logical-file progress and
durably recoverable/published work. A duplicate acknowledgement within one
execution/container/upload generation contributes nothing twice. Restored ACK
bits establish a baseline, not a new speed burst. A replacement Telegram file ID
starts an explicit transfer epoch: actual retransmission can consume throughput,
but cannot add a second copy to useful logical-file progress. Invalidated remote
parts must not continue to count as recoverable. Only expose retry wire volume
when measured at an appropriate transport boundary.

Proposed initial presentation policy:

- Aggregate into fixed 250 ms buckets and show a trailing three-second rate.
  Use elapsed time since the first dispatch during warm-up, never a fixed full
  denominator for an unobserved interval or division by a near-zero ACK spacing.
  Mark insufficient history as collecting samples. These are tunable defaults,
  not measured optimal settings.
- Publish the first usable ACK-derived observation promptly; do not wait for a
  container. Keep ordinary UI updates at most four per second, while phase,
  cancellation, failure and completion transitions publish promptly.
- Age buckets even without new ACKs. A completely observed window with no ACKs
  has zero confirmation throughput; an unavailable observation is unknown.
  Outstanding sends may still be using the network: show waiting for confirmation
  and the last-ACK age instead of implying that zero ACK throughput proves zero
  network traffic. Pause and disconnect invalidate the live value immediately.
- Use the same buckets/time horizon for per-file and global rates. Maintain
  aggregate deltas incrementally; do not sum stale averages or scan historical
  task collections on each frame. Bound memory across active tasks and retained
  history. Cancel presentation deadlines when no active/visible timed state needs
  them; timers must not read SQLite or drive service queues.
- Keep an explicitly labelled end-to-end average, including preparation, for
  history. Do not overwrite old average-field semantics with current rate. If
  additional averages are persisted, version and document their time/byte scope.
  ETA uses matching logical bytes and rates, requires sufficient stable history,
  and becomes unknown during stalls. Publication and durable completion remain
  separate from payload progress; only successful completion reaches 100%.

Keep real per-part transitions in a bounded timeline. Coalesce high-frequency
measurements before expensive snapshots or log formatting; increment omission
counters when retention drops records. A checkpoint operation can be represented
as concurrent persistence activity rather than repeatedly erasing transfer byte
counters. Never hide a real wait or terminal transition to reduce repaint cost.

## Preparation without weaker content guarantees

First instrument monotonic durations for admission/queue wait, target verification,
source opening/reading, hash CPU work, recovery commits, first block production,
first dispatch and first ACK. Report actual bytes read, processing rate and the
known wait reason independently of upload speed. Distinguish startup latency from
total transfer duration. Preserve the policy requiring verified target identity
before touching plaintext or file keys; network prewarming must not bypass it.

Low-risk work retains the full-file gate: reuse bounded buffers, keep reads
sequential, overlap bounded read and hash work where beneficial, and benchmark
limited hashing parallelism in a shared worker budget. Do not give every active
file a full machine's worth of hash threads. BLAKE3 already supports incremental
SIMD hashing, and its documentation warns that threading can be slower depending
on input size, disk and CPU contention; see the
[public Hasher API](https://docs.rs/blake3/latest/blake3/struct.Hasher.html).
The existing 1 MiB read buffer is already substantial; increasing it alone is not
an established improvement. Do not introduce mutable-file memory mapping or
change durability barriers merely to improve a benchmark.

A bounded look-ahead design can shorten time to first transport without removing
the verification pass: hash the first container, durably bind its digest to a
fresh reservation, then stream its encryption/upload while admission examines a
bounded next range. The initial dependency becomes one container rather than the
whole file. This still performs two logical reads and is not a one-pass solution.
It may contend on slow storage, so cap look-ahead across the whole session and
measure cold-cache as well as cached inputs.

That design requires a new recovery-context state for an incomplete whole-file
commitment. A provisional record cannot masquerade as a v1 record with a zero or
placeholder digest. Each container must still match its pre-admission digest;
the whole-file digest, exact layout and source checks must finish before a logical
file manifest is committed. Earlier uploaded containers are provisional physical
objects, not a completed file. Failure must suppress the manifest, retain evidence
and manage orphan objects explicitly. A partially inspected mutable source cannot
claim the same complete source baseline as v1. Keep source-dependent recovery on
the existing conservative path until this case has a proven policy; restart with
fresh identities where continuity cannot be established. This is a substantive
recovery-contract change, not removal of a hashing call.

For a true single-pass fast path, first obtain a stable, privately owned source
view, then feed each read to whole-file hashing, container hashing and encryption
in order, emitting the existing 512 KiB wire blocks. Final digests become known at
EOF; publish the authenticated manifest only after complete validation, durable
seals and required remote receipts. A single-pass digest describes the bytes read;
it alone does not prove that a concurrently modified source was a consistent file.
An open descriptor, advisory lock or unchanged size/mtime does not establish that
proof.

APFS copy-on-write clones are a candidate on supported same-volume sources;
[Apple's documentation](https://developer.apple.com/documentation/foundation/about-apple-file-system)
describes efficient same-volume cloning. Qualify the actual API's concurrent-write
and crash behavior before treating it as the source-consistency boundary. Define
whether the upload represents the captured version or must fail on later source
changes; these are different user-visible semantics. A clone remains a complete
plaintext object despite sharing physical storage: private access, same-volume
placement, disk/COW growth budgets, lifetime and crash cleanup require explicit
design. Never silently substitute a full physical copy, assume all external/network
volumes support clones, or advertise a zero-cost/constant-time operation.

In all variants reserve encryption identity durably before first use. Retry sealed
immutable ciphertext verbatim; retire the identity transactionally before
re-encrypting an unsealed/invalid attempt. An unfinished source digest must never
authorize key/nonce reuse. Preserve v1 readers and supported older encrypted
formats, migrate automatically with recoverable originals, and keep newer unknown
records intact. Do not serialize an opaque library hash state as a stable recovery
codec; recompute it from an authenticated stable source or define a separately
versioned, validated checkpoint if one is justified.

## Acceptance evidence before implementation is called complete

- Controlled clocks: first ACK, bursty/out-of-order/duplicate ACKs, retries,
  restored baselines, container rollover, expired temporary uploads, zero-ACK
  windows, observation gaps, pause and stale account/execution callbacks.
- Operation counts: bounded rate publications and incremental global totals,
  no idle history scans or database reads, and bounded sessions at maximum tuning.
- Blocked source/network/storage/log owners do not freeze unrelated files or UI.
  A blocked first-container publication must not be needed to display its speed.
- Source mutation, same-size/restored-mtime changes, replacement/truncation,
  interrupted seals, missing/corrupt spools, low disk and process death cannot
  publish an unvalidated logical file or cause encryption-identity reuse.
- Supported/skipped version upgrades preserve keys and recoverable bytes, with
  restart/rollback and unsupported-newer-data coverage.
- Release-build measurements separately report time to first ACK, source bytes
  read, hashing CPU, memory, UI frame cost and total completion time, across small
  files, large files and batches on cached/local/slow sources. No throughput
  improvement is claimed without these measurements. UI review remains English,
  light mode, 900×600 and actual full-screen, with independent inspectors.
