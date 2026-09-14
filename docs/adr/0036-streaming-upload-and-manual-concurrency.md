# ADR 0036: Streaming upload, immutable retry and manual concurrency

Date: 2026-09-14. Status: accepted.

## Decision and scope

The previous path prepared whole encrypted containers and read each published
container back, tying progress to repeated work and large buffers. Replace the
production upload path with one bounded crypto producer per file, a reusable block
queue and concurrent `upload.saveBigFilePart` RPCs. Keep the 60 MiB application
container as the publication/recovery grouping; do not create a Telegram message
for each 512 KiB frame. Part and manifest codecs advance to 2.0 to authenticate
changed geometry while retaining v1 readers and frozen fixtures.

Default admission is three upload files and ten part RPCs per file, using two
shared upload connections and a two-block ready queue. Download defaults are three
files, eight logical parts and two shared connections. Both directions default to
four attempts. All exposed limits are manual and bounded; byte caps remain shared.
No throughput profile, adaptive probe or automatic parameter rollback is active.
This supersedes the production tuning decisions of ADR 0010/0011 and the old
whole-container encryption/read-back flow, while retaining their historical tests.

Use separate retained grammers SenderPools to obtain actual MTProto connections.
Client clones from the same pool do not multiply TCP connections. Pools share the
authenticated session and the existing mandatory gateway. Assign parts round-robin
across direction-specific pools; do not introduce another dynamic lane controller.
Telegram's [official file guidance](https://core.telegram.org/api/files) recommends
parallel part saves and separate transfer connections. A fixed bounded window and
backpressure implement that guidance without an independent scheduling framework.

Reserve encryption identity before encoding. Reuse only immutable ciphertext on
retries. If the spool is not sealed/valid, atomically advance the part's 128-bit
instance ID (no wrapping) before encoding under its new derived content key. Initial
IDs/File Keys come from the OS RNG. File-ID/ACK checkpoints are independent from
cryptographic identities; restarting Telegram temporary storage does not encode.
The source digest is checked before a sendMedia can publish the encrypted object.

A 24-hour attempt window is TeleArk policy, not guaranteed Telegram retention.
Expired or unfinished legacy-v1 attempts automatically receive a fresh package and
File Key; sealed manifest finalization retains its original authenticated bytes.
The local transaction either preserves all previous recovery state or commits the
new attempt. Existing completed encrypted files are never rewritten.

## Evidence and limits

Deterministic tests exercise ten RPCs reaching a barrier before any completes,
three files progressing concurrently, a fourth filling one released slot, zero
fresh content read-backs, blocked/failing part independence, retained-task
cancellation, key-instance retirement, recovery without a historical UI read,
24-hour boundaries/clock rollback, version fixtures and SQL rollback/stale leases.
The inspector renders measured acknowledgement rates, part states and a bounded
replay timeline with explicit truncation. Logs are independently bounded and never
block transport. Settings reports CPU AES/GCM acceleration capability, not a
benchmark result.

A partial unsealed container restarts with a new instance; only sealed ciphertext
can resume individual 512 KiB parts. Large ciphertext spools are released after
publication; orphan remote-message cleanup remains separate. No claim of measured
end-to-end bandwidth or real Telegram multi-DC qualification follows from fake-wire
tests. See [recovery](../VAULT_TRANSFER_RECOVERY.md),
[crypto](../CRYPTO_FORMAT.md), and [preferences](../PREFERENCES_FORMAT.md).

If a manifest commitment exists without its sealed envelope, recovery retires the
package and File Key before any new encryption. An interrupted seal is never
recomputed with the same key/nonce. A saved v1 or v2 envelope is replayed unchanged,
including its authenticated version and frame geometry.
