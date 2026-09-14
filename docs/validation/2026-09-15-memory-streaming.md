# Memory streaming and portable recovery validation

Date: 2026-09-15. Scope: [ADR 0041](../adr/0041-memory-streaming-and-portable-upload-recovery.md).
All files, accounts, keys and remote replies used by these tests/previews are
synthetic. No local credentials or live Telegram transfer were used.

## Automated checks

| Check | Result |
| --- | --- |
| Workspace/all-target tests, locked dependencies | 766 passed, 0 failed, 10 existing ignored tests |
| GUI tests after final presentation/preview changes | 166 passed, 0 failed, 3 existing ignored tests |
| Workspace/all-target compilation | Passed |
| Workspace/all-target strict Clippy (`-D warnings`) | Passed |
| Formatting check | Passed |
| Core tests, including documentation tests | Passed |
| I18n tests, including documentation tests | Passed; synchronized catalog validation only |
| Workspace documentation, no dependencies, warnings denied | Passed |
| Dependency advisory, source, license and ban checks | Passed with existing duplicate-dependency warnings |
| Debug desktop build and unsigned macOS bundle validation | Passed |

The existing `block 0.1.6` future-incompatibility notice remains. No dependency or
lockfile change is part of this task. Ignored tests are not counted as passes.
The Linux fuzz campaigns, other desktop platforms and live-network qualification
were not run.

## Behavioral evidence

- The encoded-container ceiling includes the 96-byte header and every frame's
  overhead: at most 2,040,109,465 bytes and 3,892 aligned frames. The geometry
  regression rejects an extra plaintext byte beyond the admitted ceiling.
- The producer delivers the first block before whole-container encryption ends;
  new upload work produces no ciphertext spool. Downloads output an authenticated
  frame before consuming the next frame, and tampered frames produce no plaintext
  output from that frame. Complete container/file verification still gates final
  publication.
- A deliberately blocked checkpoint writer leaves RPC confirmation accounting
  running. Checkpoint completion is required before the relevant operation retires;
  metadata persistence does not reset the visible uploaded-byte count.
- Controlled time exercises first samples, three-second silence expiry, scheduler
  jitter, restored/duplicate receipts, pause and account changes. Native download
  callbacks reject stale attempts/accounts and deduplicate partial RPC prefixes.
  Download dispatch starts waiting/sampling before its first confirmation.
- A real-crypto, two-database handoff publishes a pending descriptor, pauses the
  first device after a container publication, imports the recovery key on the
  second, rejects a wrong source, reuses the authenticated published container and
  completes upload/download with exact final-byte equality. Unpublished lost
  ciphertext receives a new encryption identity before re-encryption.
- SQL import tests cover transactional rollback, durable pause/cancel fencing and
  the actual indexed receipt-range query plan. Frozen v1/v2 executable contexts and
  the pending v1 envelope cover exact bytes, key preservation and unsupported or
  corrupted input rejection. Existing schema upgrade/restart/rollback tests pass.
- A 10,000-task operation-count regression reuses filters, row keys and action
  scopes across 100 unchanged publications and a numeric update; the visible row
  resolves the changed value. Batch ETA uses all live receipt rates and ignores
  stale snapshot speed. Existing virtual-list tests bound visible materialization.

## Native interface review

The unsigned app was launched with `--preview-ui --screen=transfers --locale=en-US`
in light mode only. `batch-groups` covers adjacent expanded upload/download
batches, and `upload-pipeline` covers the encrypted transfer inspector. Its final
fixture contains a maximum-size container with 3,892 blocks to exercise grouping.

Review covered a 900×600 window and actual macOS full-screen mode entered with the
native full-screen shortcut; the accessibility tree reported **Exit full screen**.
The list keeps processed/total bytes, ETA, progress and actions in aligned columns;
completed rows have no remaining-time estimate. Batches use 34-point headers and
children retain 24-point rows/12-point text. Full-screen details dock alongside
the table; compact details have an accessible close control and independent
scrolling. The inspector displays the confirmation-rate basis, bounded chart,
grouped block map and event timeline with truncation/aggregation disclosure.

These are synthetic layout checks, not measured transfer rates. Preview sample
values must not be cited as network throughput or performance improvements.

## Practical limits

Source inspection still reads the complete source before payload encryption.
Whole-file and per-container hashes share that one pass; encryption rereads the
source and checks its admitted digests. This removes redundant hashing and new
ciphertext disk staging, but it does not establish a one-pass source snapshot or
a measured startup-time gain.

Cross-device continuation requires the same source bytes, recovery key, Telegram
account and storage channel. Published containers are verified and reused;
unpublished temporary RPC parts are safely retransmitted. Immutable pending
messages are retained and hidden from the logical listing after completed-manifest
verification. There is no distributed exclusive-writer lock or automatic remote
orphan cleanup; pause the original device for handoff.
