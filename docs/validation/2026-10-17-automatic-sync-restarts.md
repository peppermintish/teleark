# Automatic synchronization restart qualification — 0.5.18

Date: 2026-10-17 (Australia/Adelaide). Scope: the existing authorized macOS account and library, followed by native cross-platform release CI. Only aggregate observations belong in this report; account/channel/message identities, filenames/captions, private snapshots, session bytes and credentials are excluded.

## Investigation and resulting behavior

An initial real-account run of the fixed one-second policy still received repeated short Telegram server waits during a large cached-message repair. That run failed the qualification and prompted adaptive admission rather than a claim that fixed spacing solved the account's limits. The existing restart bug also forced differences for unchanged channel heads and discarded server cooldowns. ADR 0067 records the automatic-sync contract and fixes.

The final policy doubles page-start spacing once per new server cooldown window, caps it at 30 seconds, and reduces it by one second only after 60 healthy page results outside an active cooldown. Both the learned interval and the server deadline survive restart. Routine discovery, incremental recovery and retries need no sync command. The inspector is informational, with Close as its only control.

## Real-account procedure and observations

The app used its existing library and authorized session. Private local credentials were loaded only in the scoped build environment; local credential files remained ignored and mode 0600. The first three warm checks used existing data: no test message/file was uploaded and no payload was downloaded. The later explicitly authorized synthetic marker is recorded separately below. A separate local read-only probe captured remote channel heads and hashed metadata for a bounded sample while the GUI process was stopped. Process IDs/start times and diagnostics launch markers verified actual restarts; merely reopening a window was not counted.

The adaptive catch-up launch began at 2026-10-16 19:05:10 UTC. It resumed 1,127 remaining cached-message checks automatically, made 15 verification attempts and one directory scan, and received three further server waits (27, 19 and 7 seconds). Spacing rose from the restored legacy two seconds through four/eight to 16 seconds. Catch-up completed with zero pending repairs/gaps and 10,248 cached file records. This bulk recovery is **not** described as rate-limit-free.

| Warm restart | Observed behavior | Server waits |
| --- | --- | --- |
| 1, 19:08:02 UTC | Restored the account and 16-second interval automatically. One directory scan and one incremental difference for a live channel update; no remaining repair/gap. Observed for about 40 seconds, then normal application quit was verified. | 0 |
| 2, 19:10:09 UTC | After more than a minute offline, an independent snapshot found three advanced channels among 36. Exactly three automatic difference requests caught up all 36 heads; no missing sampled files or pending repair/gap. | 0 |
| 3, 19:12:31 UTC | Observed for 78 seconds with the inspector initially closed. One directory scan, zero difference requests, automatic listening/idle state. All 36 independently recorded heads covered, all 41 sampled files present with matching normalized metadata, zero pending repair/gap. Learned 16-second spacing retained. | 0 |

The metadata probe initially compared raw absent Telegram filenames against the application's canonical generated names and reported 19 differences. The probe was corrected to apply the existing storage normalization (`telegram-document-<message-id>` for absent names). A new independent snapshot then checked 41 remote files: 19 absent filenames normalized, zero missing files and zero differences in normalized name, caption, size, MIME type and optional duration. Private snapshot files remained under the ignored build directory with mode 0600; they are not release artifacts or test fixtures.

## Additional authorized controlled change

After the read-only tests and release push, the user approved a synthetic-file upload and requested continuation. During the additional check on 2026-10-17, with the GUI process stopped, the probe required the existing fixed account binding and fresh remote private-channel health validation. It uploaded one 71-byte plain-text synthetic document, `teleark-sync-qualification-0.5.18-20261017.txt`, to that bound channel. It did not create/repair a channel or change keys. A private create-once send intent prevents blind duplicate publication after an uncertain result.

Remote readback confirmed name, caption and size. Before restart the channel cursor lagged the post-upload head and the message was absent from the local cache. A new GUI process restored the same account and learned 16-second spacing without any sync command. The independent verifier then found the new document with matching normalized metadata, the channel cursor covering the captured head and no pending repair/gap for that channel. The launch had one directory scan and three difference requests at that observation, with zero error/server-wait responses. This additionally tests catch-up against a deliberately created remote change, rather than only natural updates. The synthetic file is retained in the private channel; no remote deletion was requested or performed.

The account had also been offline for roughly twelve hours before this additional launch. Unrelated channel repair/gap work remained automatic in the background at the marker observation; the marker result is not a claim that every channel's historical repair had already finished at that instant.

## Deterministic and local release checks

- Workspace/all-target tests: **1,007 passed, 10 ignored, zero failures**. The ignored tests are not passes.
- Workspace/all-target check and strict Clippy; final Runtime strict Clippy after the restart assertion; formatting and whitespace checks pass.
- Focused cooldown-owner restart test passes with a controlled clock: restored eight-second admission and a live deadline precede startup discovery; shutdown remains cancellable; expiry resumes automatically without GUI commands.
- Codec compatibility covers missing records, additive optional v1 spacing, legacy v1 automatic adoption, preserved deadlines/account scope, invalid/newer original preservation and healthy recovery persistence. Pacing tests cover overlapping waits, bounded adaptation, gradual recovery and cancellation without held locks.
- Explicit Core and non-visual i18n tests, warning-denied documentation and Cargo-deny pass. Existing dependency warnings remain; no new dependency was added.
- LF policy, synthetic distribution credentials (including redaction), Store configuration/client/workflow graph/MSIX, release-version, macOS library and Linux payload checks pass.
- Seven-package release assembly/checksum/rejection fixtures pass in an offline Ubuntu 22.04 container. The host's Bash 3 cannot execute that associative-array script; the container provides the required toolchain.
- English/light synthetic native layout review for the implementation covers 900×600 and actual macOS full-screen, independent inspector scrolling and the absence of sync-management controls. Automated GUI checks also cover both layout flags and retain existing theme coverage. Live observations used English/light mode; the final live run also verified native full-screen entry/exit and independent history scrolling.

A release rebuild initially ran out of disk space during linking. Only generated incremental build caches were removed; the rebuilt app and subsequent checks passed. Account data was not reset. An initial build without embedded local API configuration was excluded from successful live runs. One controlled process termination during the earlier investigation also resumed from the durable checkpoint; the three warm observations above use actual process relaunches following normal quit.

## Limits and release delivery

These observations establish automatic recovery and absence of a server wait on the measured warm restarts for this account. They do not guarantee Telegram will never rate-limit large imports, concurrent clients or future traffic. Required work remains paced and the server deadline authoritative.

The existing development app reported no usable channel key. This qualification covers account/channel cursors and ordinary cached Telegram metadata, not decrypted Vault contents or encrypted transfer round trips. No key was replaced, reset or imported. Supported encrypted formats remain covered by synthetic compatibility/recovery tests.

Local source checks qualify this revision; native Linux/Windows/macOS CI, package verification and GitHub publication are separate delivery gates. Store ingestion acceptance and Microsoft certification/publication are distinct outcomes and must be reported separately.

## Published release evidence

[Release v0.5.18](https://github.com/peppermintish/teleark/releases/tag/v0.5.18) was published at 2026-10-16 19:37:41 UTC from release commit `9267bc7`. The [tagged CI run](https://github.com/peppermintish/teleark/actions/runs/38953479262) completed successfully: repository quality, native Linux x64/Windows x64/macOS arm64 tests, all three package groups, verified GitHub publication and independent Store delivery. The release contains the exact seven platform packages, both licenses, third-party notices and unified `SHA256SUMS`.

The sanitized Store receipt records phase `accepted`, status `PreProcessing`, at 2026-10-16 19:42:39 UTC for the verified 0.5.18 MSIX. This is ingestion acceptance, not a claim that Microsoft certification or Store publication completed.
