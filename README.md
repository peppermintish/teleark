# TeleArk

**A native desktop workspace for your files on Telegram.**

Browse channel files, store encrypted files in your own private Telegram channel, and manage uploads and downloads in one place. TeleArk combines a searchable local library with encrypted storage and recovery tools.

[Features](#available-features) · [Build and run](#build-and-run) · [Build and packaging guide](docs/PACKAGING.md) · [Architecture](docs/ARCHITECTURE.md)

## Available features

- **Channel browsing** — Search and filter files by type or date, select multiple files, and download channel batches.
- **Encrypted storage** — Upload individual files or batches to a dedicated private channel. Authenticated manifests keep each file discoverable and let TeleArk verify its contents during restoration.
- **Transfer management** — Follow progress, queues, retries and live or historical diagnostics. Ordinary Telegram downloads support pause, resume, cancel, retry and restart recovery. Upload history survives restarts; separate upload and download speed limits apply to active transfers.
- **Local and remote library** — Browse downloaded/imported files separately from indexed Telegram files, with search, filters and file actions. Background checks identify missing or changed local downloads.
- **Key management and recovery** — Unlock with a password or recovery material, export and restore recovery bundles, change passwords and retain older key versions for existing files.
- **Desktop controls** — Light and dark themes, a collapsible sidebar, keyboard shortcuts, configurable storage paths, SOCKS5/HTTP CONNECT proxy settings, and English, Simplified Chinese and Japanese interfaces.

## How it works

TeleArk treats a **logical file** as one file, even when its encrypted storage spans multiple Telegram objects. The **Files** view presents complete authenticated files; **Raw Files** exposes the underlying documents, encrypted parts and manifests.

| Layer | Technology |
| --- | --- |
| Application | Rust 2024, organized as a Cargo workspace |
| Desktop interface | GPUI Kit with virtualized lists and native windows |
| Telegram connectivity | MTProto through `grammers` |
| Local catalog | SQLite, FTS5 search and versioned migrations |
| Encryption | AES-256-GCM, Argon2id password derivation and BLAKE3 integrity digests |
| Localization | Fluent catalogs through `teleark-i18n` |

Core, Runtime, Storage, Telegram, Crypto, Index and Transfer are independent of the GUI framework. Background workers handle network, filesystem, database and encryption work through bounded queues, keeping the interface responsive. See the [architecture](docs/ARCHITECTURE.md) and [encrypted file format](docs/CRYPTO_FORMAT.md) for the contracts and module boundaries.

## Build and run

**macOS is the current build and release baseline.** Install Rust through rustup and Apple Command Line Tools (`xcode-select --install`). The repository uses `rust-toolchain.toml` and the checked-in `Cargo.lock`.

Run the following commands from the repository root in bash or zsh. For platform prerequisites, standalone executables, macOS `.app` / `.dmg` / `.pkg` packages and Windows/Linux recipes, see the **[build and packaging guide](docs/PACKAGING.md)**. Windows and Linux builds and installers remain unverified.

### Load `.env` values before building

Create a private local configuration without overwriting an existing file:

```bash
if [ ! -e .env.local ]; then
  (umask 077; set -C; cat .env.example > .env.local)
fi
chmod 600 .env.local
```

Edit `.env.local` and replace both sample values with your application's API ID and API Hash from [Telegram's API development panel](https://my.telegram.org/apps). [`.env.example`](.env.example) is a template containing public **TEST ONLY** identifiers; it is never a fallback build configuration. Local environment files are Git-ignored.

After configuring `.env.local`, copy and run this single command to build and launch:

```bash
scripts/build-local.sh && ./target/release/teleark
```

To build without launching:

```bash
scripts/build-local.sh
```

The helper loads `.env.local`, validates both values and rejects the public sample API ID. Neither Cargo nor TeleArk loads environment files automatically: application identifiers are embedded **at compile time**, so supply them on each build and rebuild after changing them. Personal credentials saved in Settings take precedence; the built executable needs no environment file alongside it.

Source only your trusted local configuration and keep it out of logs and packages. Embedded application identifiers are extractable from binaries. On Windows, use Git Bash with native Windows Rust and restrict the file with Windows permissions as well.

### First launch

1. **Sign in** with QR or phone/code, including two-step verification when enabled.
2. **Open TeleArk** to create or rediscover the dedicated private channel owned by your account.
3. **Set up the Key Vault**, export a recovery bundle and keep it offline. Then upload files, browse **Files**, and follow work in **Transfers**.

New uploads go to the private TeleArk channel. Older Saved Messages packages remain recoverable through **Settings → Key Vault → Advanced**.

### UI preview

Preview a synthetic workspace without signing in, opening real user state or contacting Telegram:

```bash
cargo run -p teleark-gui -- --preview-ui --screen=transfers --locale=en-US --window-size=900x600
```

Add `--preview-dark` for the dark theme. See [Development](docs/DEVELOPMENT.md) for more preview routes, contributor guidance and quality checks.

## Data and recovery

Encrypted storage protects remote file contents and original names. Telegram can still observe channel relationships, ciphertext sizes and timing. The local SQLite catalog is unencrypted and contains metadata and saved personal API credentials; Telegram sessions use a separate adapter cache. On macOS, the catalog defaults to `~/Library/Application Support/TeleArk/library.sqlite3`; storage paths are configurable in Settings.

Recovery after database loss requires a valid recovery bundle, access to the Telegram account, and retained manifests and encrypted parts. Keep recovery material separately from the stored files. Rotating a recovery record does not revoke older exported bundles that wrap the same Master Key. See [Security and data integrity](docs/SECURITY.md) for details.

**Current limits:** interrupted encrypted uploads retain history but do not automatically resume. Encrypted transfers use a temporary 60 MiB plaintext part ceiling; ordinary Telegram downloads verify byte length rather than a cryptographic content hash. OS Credential unlock is disabled, and macOS packages are unsigned. [Implementation status](docs/IMPLEMENTATION_STATUS.md) tracks remaining work and validation; [Changelog](CHANGELOG.md) records release history.

## License

TeleArk is dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option. Distributions include [third-party notices](THIRD_PARTY_NOTICES.md).
