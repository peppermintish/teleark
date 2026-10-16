# ADR 0066: Grammers 0.11 and source-reported media duration

Status: Accepted — 2026-10-16. Adds SQLite schema 25 and Telegram session codec v2 for Community peers. Does not change container, part, manifest, key-wrap or recovery-bundle bytes.

## Context

Grammers 0.11 updates to Telegram API layer 230, replaces `chrono` timestamps with `jiff`, moves retry policy from `ClientConfiguration` to sender-pool connection parameters, changes peer caching to support batches, and adds Telegram Community peers. GPUI Kit 0.7.3 includes the selectable-text implementation used by the desktop inspectors.

Telegram's audio/video document attributes provide duration, but filenames and MIME types do not. The app must not invent duration or read whole media payloads to display it. Existing encrypted manifests do not contain playback duration.

## Decisions

- Upgrade the grammers crates together at exactly 0.11.0 and GPUI Kit to exactly 0.7.3. Configure retry policies per sender pool. The primary pool keeps bounded `AutoSleep`; sync uses `NoRetries` so the coordinator owns retry/rate-limit feedback. One-shot authorization confirmation invokes the raw pool handle to avoid starting a retry loop from the check itself.
- Store Telegram-reported media duration as nullable, non-negative integer milliseconds in `logical_files.duration_ms` and `native_download_tasks.media_duration_ms` (SQLite schema 25). Carry it through normalized Telegram updates, cached file projections, remote upserts, native download history and `LogicalFile`. Old records and sources that omit duration remain NULL. UI uses a compact `M:SS`/`H:MM:SS` formatter; unknown duration is rendered as unavailable, never inferred from a file extension or MIME type.
- Do not alter authenticated manifest codec bytes for duration. Managed-file entries only have a duration when authenticated metadata provides one; existing manifests do not, so their video/audio duration remains unavailable until a separately versioned metadata contract is designed.
- Keep old Telegram session bytes unchanged. Session codec v1 remains the writer when no `ChannelKind::Community` occurs. If a Community peer is cached, write codec v2; both versions are readable by this release. A reader that does not know Community must not reinterpret version-2 bytes as version 1. The v1 fixture and round-trip tests use synthetic peer IDs and synthetic key bytes.
- Use GPUI Kit `SelectableText` for detail values, captions, transfer detail rows and inspector summaries. The app's existing `component::Root` owns text-selection scope, Copy action and clipboard integration; tests mount that root and exercise selection and copy rather than only checking an element style.

## Compatibility and migration

Schema 25 adds nullable `logical_files.duration_ms` and `native_download_tasks.media_duration_ms`. The ordered transactional migration is restartable and does not rewrite existing file or ciphertext bytes. Existing rows retain NULL. Native-download output projections carry source duration into the Local list; existing transfer elapsed-duration fields keep their prior meaning. Applications supporting schema 24 reject schema 25 while preserving the database. Tests cover read/write, filtered search projections, restart, migration rollback and automatic upgrade from older versions.

Grammers timestamps cross into TeleArk as Unix milliseconds; persisted timestamps retain their existing locale-neutral units. Existing upload/download transfer data, encrypted containers, parts, manifests, key wraps and recovery bundles retain their versioned readers and writers.

## Verification

Deterministic tests cover duration conversion, DB persistence, old-session v1 fixture compatibility, conditional v2 session writing, GPUI list display, inspector drag selection and clipboard copy. Workspace tests, strict Clippy, formatting, documentation and `cargo deny` are required before the tagged release. Hosted platform packaging and Store ingestion remain governed by the existing tagged release workflow.
