# TeleArk

TeleArk turns Telegram collections into a desktop file workspace. Browse ordinary channels, keep encrypted logical files in your own private TeleArk channel, and follow uploads/downloads from a permanent Transfers page. One logical file stays one file even when its encrypted storage uses multiple objects.

> [!IMPORTANT]
> v0.4.2 is an early alpha. Real Telegram browsing/native downloads and encrypted upload/discovery/restore are connected. Crypto formats remain provisional; independent security review, durable encrypted transfer controls and signed packaging are unfinished. Keep independent copies of important data and recovery material.

## Start using the workspace

1. Sign in by phone/code (and two-step verification if enabled) or QR. A restored session opens a centered avatar/name welcome screen with **Log In** and **Switch Account**.
2. Open **TeleArk** and create or rediscover its dedicated private channel. The channel must be owned by the active account. A distinct sidebar entry keeps it separate from ordinary channels; renaming it does not lose the binding.
3. Use **Files** for authenticated complete logical files or **Raw Files** for original Telegram documents and encrypted storage objects. The Help guide explains the relationship and recovery requirements.
4. Choose **Upload**. If the Key Vault needs setup/unlock, the dialog returns to the requested action after successful unlock and recovery acknowledgement. Keep the exported recovery bundle offline.
5. Use **Transfers** for progress, native pause/resume/cancel/retry, batch actions and optional detailed diagnostics. Channel refresh and long channel lists do not hide this destination.

New uploads go only to the private TeleArk channel. Existing Saved Messages packages remain recoverable through **Settings → Key Vault → Advanced**. TeleArk does not move or delete old remote objects automatically.

## Available features

- Apple-style GPUI Kit workspace with a collapsible icon sidebar, visible free disk space, light/dark appearance, responsive layouts, keyboard shortcuts and English, Simplified Chinese and Japanese.
- Bounded channel browsing with a virtualized file table, search, time/type filters, single/multiple downloads and channel batches.
- Persistent native download history, missing-part restart resume, bounded retries/FloodWait handling and Balanced/Max Throughput strategies.
- Encrypted single/multi-file upload with reviewable batches, authenticated manifest discovery, verified restoration and non-overwriting final publication.
- Background checks for missing/changed local downloads, safe new-task re-download and account-scoped local output history.
- Password/recovery unlock, explicit recovery-bundle export/restore, password change and recovery rotation. Advanced controls stay folded until needed.
- Library separates accessible local downloads/imports from the current account’s indexed remote files, with independent type filtering, search, paging, details and Open/Reveal/Download actions. Importing metadata is separate from uploading a file.
- Configurable managed storage, privacy-bounded diagnostics, live/replay transfer details and the full localized [changelog](CHANGELOG.md) in **Settings → About**.

Native downloads currently verify byte length rather than a cryptographic content hash. Encrypted transfers use bounded buffers and a temporary 60 MiB plaintext part ceiling; their task controls/checkpoints are not yet durable. OS Credential unlock remains visibly disabled until its platform adapter is reviewed. See [implementation status](docs/IMPLEMENTATION_STATUS.md) for precise limits.

## Build and run

There is no signed installer yet. On macOS with the repository's Rust toolchain and Apple Command Line Tools:

```bash
cargo run -p teleark-gui --bin teleark
```

Source builds without distributor configuration provide an explicit API setup action on login or in Settings. Register your own Telegram application in its [API development panel](https://my.telegram.org/apps), then enter that application's API ID and API Hash. Personal settings override a distributor-owned pair; TeleArk never reuses Telegram Desktop credentials.

The default catalog lives at `~/Library/Application Support/TeleArk/library.sqlite3`. Settings exposes the managed root containing Downloads, Cache and Logs. Switching accounts pauses/drains native work and preserves completed files/history; old unknown-account work is retained without becoming executable under a new login.

For isolated layout review without opening real user state or contacting Telegram:

```bash
cargo run -p teleark-gui -- --preview-ui --screen=transfers --locale=zh-CN --window-size=900x600
cargo run -p teleark-gui -- --preview-ui --preview-state=about --preview-dark --locale=ja-JP
```

The minimum window is 900×600; oversized requests fit the display. [Development](docs/DEVELOPMENT.md) lists all preview routes, quality gates and documentation recovery commands.

## Data and recovery

The local SQLite catalog is unencrypted and contains filenames, paths and other useful metadata. Personal API Hashes are stored there too; readable backups expose them. Distributor credentials are extractable from their binary. Telegram sessions stay in a separate adapter cache; QR authorization links remain memory-only.

Vault protects remote original names/content, but cannot hide channel relationships, ciphertext sizes, timing or message counts from Telegram. A valid recovery bundle plus account access and retained manifests/parts can reconstruct completed packages after database loss. Rotating the current recovery record cannot revoke old exported bundles wrapping the same Master Key. Protect those exports separately.

See [Security](docs/SECURITY.md), [Architecture](docs/ARCHITECTURE.md) and [Implementation status](docs/IMPLEMENTATION_STATUS.md). The concise [contributor guide](AGENTS.md) routes contributors to the relevant contracts; accepted ADRs and before/after documentation checkpoints preserve design history.

## License

TeleArk is licensed under your choice of [Apache-2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT). Distributions include [third-party notices](THIRD_PARTY_NOTICES.md).
