# Targeted channel synchronization — 2026-10-22

## Scope and behavior

Release 0.5.19 replaces recurring ordinary-channel difference sweeps with push-driven updates and targeted recovery from missing sequences or newer directory heads. It retains per-channel durable PTS and unfinished history/repair work. Account-wide lost-update signals reconcile remote directory heads first. Ordinary view subscriptions and managed storage subscriptions use Telegram’s returned final-difference timeout (one-second default/minimum); admission and FloodWait remain authoritative. Closing or locking an ordinary view retires its subscription without cancelling independent recovery work.

The latest viewed channel is a retained, coalesced identity, independent of command-queue capacity and transient wake tokens; the owner rechecks this retained demand immediately before parking. Waiting workers are projected separately from executing work, retain their known deadlines, and cannot inflate the shell’s active channel count. Per-channel icons reserve a shared trailing slot and include the full channel name and localized phase in the accessible label and tooltip. Timeline capacity and dropped-event disclosure remain bounded.

## Deterministic evidence

- Focused synchronization suite: **68 passed**. It covers contiguous/duplicate pushes without RPC, missing-sequence recovery, account/connection isolation, persistence failure, cancellation, blocked reads, managed execution reservation and cooldowns.
- The owner regression opens 32 cached channels across repeated launches with no redundant difference calls; advances one remote head and catches up only that channel; replaces its connection without another catch-up; opens its view and renews once at the returned 30-second deadline; closes it and advances to a later quiet directory probe without any additional channel differences. All three necessary difference calls target the same affected channel.
- Clock-controlled cases cover returned/default/minimum subscription timeouts, pushes preserving renewal deadlines, registration before directory discovery and closed queued polls preserving real gap demand.
- Saturating command admission and consuming wake tokens cannot lose the latest view or close identity. The coordinator checks retained view demand before parking.
- Active/waiting projection cases cover overlapping work, completion of another channel, preserved wait deadlines and cancellation. Server-error outcomes retain the account-wide cooldown rather than a shorter channel wait.
- GUI tests exercise active, pacing, server-wait and completed row states, 24-point row geometry, the 14-point shared icon slot, sidebar virtualization and distinct shell counts at 900×600 and native full-screen flags. The canonical English About release record is synchronized with CHANGELOG.md.

## Native visual review

Windows synthetic `--preview-ui` runs used `--locale=en-US` and light mode only. The requested 900×600 content window and actual native 1920×1080 full-screen mode were inspected using the application’s F11 action, not a maximized or oversized window.

Active synchronization displays the small sync icon on the affected channel and “Syncing 1 channel” in the shell. The server-wait fixture displays a pause icon, “Waiting to sync”, **0 active**, a fixed next-check time, a reason and elapsed/last-activity text. Channel titles remain aligned. Inspector scrolling leaves the file list stationary at both sizes. Escape dismisses the inspector and leaves Select all, Batch, Download and Earlier history reachable. Synthetic filenames and channel titles remain unchanged, including Unicode content; interface text is English.

No real Telegram session, personal credentials, local user documents, keys or remote channel contents were accessed. Screenshots were inspected through the native review tool and are not committed.

## Compatibility and dependency review

SQLite read/write schema remains **26**, with existing supported automatic upgrades **0–25 → 26**, including skipped releases. Directory and cooldown codecs remain v1. Persisted observation timestamps retain their meaning; they no longer schedule ordinary-channel sweeps. Payload/container/manifest readers and writers, key wraps, recovery bundles and transfer checkpoint formats are unchanged. No new persistent format or migration is introduced.

The release audit found the pre-existing RustCrypto `inout` 0.2.3 yanked. Only that existing transitive dependency advances to compatible **0.2.5**, which declares **MIT OR Apache-2.0** and Rust **1.85**, within the application’s Rust 1.88 floor. Notices and deny.toml record the update without a new exception or relaxed policy. The updated graph passes `cargo deny --locked check` for advisories, bans, licenses and sources. Crypto compatibility/streaming tests remain part of the full release gate.

## Release checks

The local full-workspace run passed **986 tests**, with **10 existing ignored tests**. The final coalesced-view wake/stop refinements additionally pass the **68-test focused sync suite**; the application-lock subscription calls are covered by the **5-test focused lock UI suite**, which also passes. The full native Windows/Linux/macOS suites run again against the exact tagged release commit before publication. Formatting, locked all-target checking, strict Clippy, explicit Core (25)/i18n (27) tests, warning-denied rustdoc and dependency auditing pass. Full source checks include formatting, locked workspace/all-target checking, strict Clippy, the full workspace test suite, explicit Core/i18n tests, warning-denied rustdoc, dependency auditing and release-script fixtures. Script fixtures cover LF policy, distribution credential redaction, Store configuration/submission, workflow dependencies, version/checksum validation, MSIX validation, exact seven-package assembly, macOS library restrictions and Linux payload integrity.

## Published release

[Release v0.5.19](https://github.com/peppermintish/teleark/releases/tag/v0.5.19) is published from annotated tag `v0.5.19` at `aecdda3ed1aac82c97a3cca40393ec80109a9691`. [Tagged run 39755121385](https://github.com/peppermintish/teleark/actions/runs/39755121385) passes the exact-commit Windows/Linux/macOS test suites, repository quality, all three platform packaging jobs and final release assembly/publication.

The public release is stable (neither draft nor prerelease). All **11 assets** are uploaded: seven platform packages, two licenses, third-party notices and `SHA256SUMS`. Independent inspection of the published manifest confirms checksum entries for all **10 payload files**, without missing, extra or duplicate names. CI performs the actual downloaded-package hash and package-content verification before publication. Microsoft Store delivery was still processing its upload/commit step at this checkpoint. This is not evidence of completed certification or public Store availability.


## Limits

These deterministic and synthetic checks establish request selection, scheduling, state transitions and UI behavior; they do not measure live-account synchronization speed or certify a server rate-limit frequency. Persisted adaptive admission can still delay necessary requests after prior server waits. Initial imports and genuine gap repair remain paginated background work. Actual macOS/Linux builds and release package verification run in tagged CI. Store ingestion/certification is separate from GitHub Release publication.
