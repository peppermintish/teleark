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
- **Desktop controls** — Light and dark themes, a collapsible sidebar, keyboard shortcuts, configurable storage paths, SOCKS5/HTTP CONNECT proxy settings, and ten interface languages with English fallback for untranslated messages.

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

Install Rust through rustup and Apple Command Line Tools (`xcode-select --install`) on macOS. On Windows, install the native MSVC Rust toolchain via rustup and Visual Studio Build Tools with the "Desktop development with C++" workload and Windows SDK. The repository uses `rust-toolchain.toml` and the checked-in `Cargo.lock`.

Run commands from the repository root in bash/zsh on macOS/Linux, or in PowerShell / Git Bash on Windows. The single CI/CD workflow packages Windows x64, universal macOS (Intel and Apple Silicon), and Linux x64 for matching version tags. It publishes portable outputs and native installers; see the **[build and packaging guide](docs/PACKAGING.md)** for files and platform requirements. Hosted release qualification is tracked in [implementation status](docs/IMPLEMENTATION_STATUS.md).

### Load `.env` values before building

[`.env.example`](.env.example) is an illustrative reference template only, documenting the expected variable names with public **TEST ONLY** identifiers. It is never automatically loaded, sourced, or fallen back to by Cargo, TeleArk, or any build/run script. Local environment files are Git-ignored.

Create a private local configuration without overwriting an existing file:

**macOS / Linux (bash / zsh):**

```bash
if [ ! -e .env.local ]; then
  (umask 077; set -C; cat .env.example > .env.local)
fi
chmod 600 .env.local
```

**Windows (PowerShell):**

```powershell
if (-not (Test-Path .env.local)) {
  Copy-Item .env.example .env.local
}
icacls .env.local /inheritance:r /grant:r "$($env:USERNAME):(R,W)"
```

The `icacls` command removes inherited permissions and replaces the current user's grant with read/write access. It does not remove existing explicit grants to other users or groups. Inspect the result with `icacls .env.local` and remove such grants before storing credentials so the file has user-restricted access equivalent to Unix mode `0600`.

Edit `.env.local` and replace both sample values with your application's API ID and API Hash from [Telegram's API development panel](https://my.telegram.org/apps).

After configuring `.env.local`, build and launch using the platform helper:

**macOS / Linux (bash / zsh):**

```bash
scripts/run.sh
```

To build without launching:

```bash
scripts/build-local.sh
```

**Windows (PowerShell):**

```powershell
powershell -NoProfile -File .\scripts\run.ps1
```

To build without launching:

```powershell
powershell -NoProfile -File .\scripts\build-local.ps1
```

These commands launch a child PowerShell process so loaded credentials do not remain in the calling session's environment. PowerShell 7 users may substitute `pwsh` for `powershell`.

*Note on Windows execution policies:* If PowerShell script execution is disabled by default (`PSSecurityException` or `running scripts is disabled on this system`), you can run `run.ps1` temporarily without modifying system-wide or user default settings:

- **Single invocation (recommended):** Pass `-ExecutionPolicy Bypass` to PowerShell for that specific run:
  ```powershell
  powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run.ps1
  ```
  *(Or in PowerShell 7+: `pwsh -NoProfile -ExecutionPolicy Bypass -File .\scripts\run.ps1`)*

- **Current console session only:** Temporarily allow scripts in the active terminal window (reverts automatically as soon as the terminal is closed):
  ```powershell
  Set-ExecutionPolicy -Scope Process -ExecutionPolicy Bypass
  powershell -NoProfile -File .\scripts\run.ps1
  ```

**Windows (Git Bash):**

In Git Bash, run `scripts/run.sh` or `scripts/build-local.sh`. If editing `.env.local` with Windows editors, ensure the file retains Unix (`LF`) line endings so the bash regex validation accepts the 32-character API Hash without trailing carriage returns.

The helpers load `.env.local`, validate both values and reject the public sample API ID (`17349`). If `.env.local` is missing, the helper stops immediately with an error and will never fall back to `.env.example`.

Neither Cargo nor TeleArk loads environment files automatically: application identifiers are embedded **at compile time** via `option_env!`, so supply them on each build and rebuild after changing them. A direct `cargo run --release -p teleark-gui --bin teleark --locked` without environment variables builds without embedded credentials; when launched, TeleArk will prompt for personal API credentials in the UI (Settings / Login) and save them through the selected Keychain/SQLite credential backend. Personal credentials saved in Settings take precedence; the built executable needs no environment file alongside it.

Source only your trusted local configuration and keep it out of logs and packages. Embedded application identifiers are extractable from binaries.

### Script dry runs

Add `--dry-run` to preview what each script would do without loading credentials, building, launching, packaging or running tests:

**macOS / Linux:**

```bash
scripts/run.sh --dry-run
scripts/build-local.sh --dry-run
scripts/package-macos.sh --dry-run
python3 scripts/test-build-local.py --dry-run
```

**Windows (PowerShell):**

```powershell
powershell -NoProfile -File .\scripts\run.ps1 --dry-run
powershell -NoProfile -File .\scripts\build-local.ps1 --dry-run
```

*(If script execution is disabled, use `powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run.ps1 --dry-run`)*

For a packaging preview with custom paths, use `scripts/package-macos.sh --dry-run path/to/teleark path/to/TeleArk.app`. Dry runs describe the planned operations; they do not validate local credentials or installed tools.

### First launch

1. **Sign in** with QR or phone/code, including two-step verification when enabled.
2. **Open TeleArk** to create or rediscover the dedicated private channel owned by your account.
3. **Set up the Key Vault**, export a recovery bundle and keep it offline. Then upload files, browse **Files**, and follow work in **Transfers**.

Encrypted uploads go to the private TeleArk channel.

### UI preview

Preview a synthetic workspace without signing in, opening real user state or contacting Telegram:

```bash
cargo run -p teleark-gui -- --preview-ui --screen=transfers --locale=en-US --window-size=900x600
```

Use English (`en-US`) and light mode for visual previews and layout reviews, checking both 900×600 and actual native full-screen mode. Existing automated dark-theme tests may remain. See [Development](docs/DEVELOPMENT.md#isolated-ui-review) for more preview routes, contributor guidance and quality checks.

## Data and recovery

Encrypted storage protects remote file contents and original names. Telegram can still observe channel relationships, ciphertext sizes and timing. The local SQLite catalog is unencrypted and contains metadata and saved personal API credentials; Telegram sessions use a separate adapter cache. On macOS, the catalog defaults to `~/Library/Application Support/TeleArk/library.sqlite3`; storage paths are configurable in Settings.

Recovery after database loss requires a valid recovery bundle, access to the Telegram account, and retained manifests and encrypted parts. Keep recovery material separately from the stored files. Rotating a recovery record does not revoke older exported bundles that wrap the same Master Key. See [Security and data integrity](docs/SECURITY.md) for details.

**Current limits:** interrupted encrypted uploads retain history but do not automatically resume. Encrypted transfers use a temporary 60 MiB plaintext part ceiling; ordinary Telegram downloads verify byte length rather than a cryptographic content hash. OS Credential unlock is disabled, and macOS packages are unsigned. [Implementation status](docs/IMPLEMENTATION_STATUS.md) tracks remaining work and validation; [Changelog](CHANGELOG.md) records release history.

## License

TeleArk is dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option. Distributions include [third-party notices](THIRD_PARTY_NOTICES.md).
