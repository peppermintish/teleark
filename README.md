# TeleArk

**A native desktop workspace for your files on Telegram.**

Browse channel files, store encrypted files in your own private Telegram channel,
and manage uploads and downloads in one place. TeleArk combines a searchable
local library with authenticated encrypted storage and recovery tools.

[Build and run](#build-and-run) · [Documentation](docs/README.md) ·
[Packaging](docs/PACKAGING.md) · [Changelog](CHANGELOG.md)

## Features

- **Channel browsing:** search and filter indexed files, select individual files,
  or download a filtered batch into its own directory.
- **Encrypted storage:** upload files to a dedicated private channel. Authenticated
  manifests describe each complete logical file, even when it spans multiple
  Telegram objects.
- **Transfers:** follow queues, phases, retries, speed limits and diagnostic
  timelines. Native and encrypted transfers have durable controls and supported
  restart recovery; older upload summaries without recovery records cannot resume.
- **Library:** browse local and remote files separately. Background filesystem
  observations distinguish available, missing, changed and inaccessible downloads.
- **Keys:** select one stored key that authenticates the managed channel, import a
  missing key, and export the selected key for recovery on another device.
- **Desktop controls:** light/dark themes, keyboard shortcuts, configurable paths,
  SOCKS5/HTTP CONNECT proxies, an optional application PIN and ten interface
  languages, with English fallback for incomplete translations.

The **Files** view presents authenticated logical files; **Raw Files** exposes the
underlying Telegram documents, encrypted parts and manifests. See
[current capabilities and limits](docs/IMPLEMENTATION_STATUS.md) before relying on
recovery or platform support.

## Build and run

Install Rust through rustup. The repository selects its toolchain in
[`rust-toolchain.toml`](rust-toolchain.toml) and dependencies in `Cargo.lock`.
Platform prerequisites and package output formats are in the
[build guide](docs/PACKAGING.md).

From the repository root, build and launch without embedding any API credentials:

```sh
cargo run --release -p teleark-gui --bin teleark --locked
```

On first launch, enter your own API ID and API Hash from
[Telegram's API development panel](https://my.telegram.org/apps) through the
login/settings interface. Sign in with QR or phone/code and complete any two-step
verification. TeleArk discovers or creates your managed private channel. Once it
confirms the channel key, export that key from Settings → Encryption keys and keep
it separately from your Telegram files.

### Optional build-time configuration

Local build helpers embed application identifiers from a private `.env.local`.
They require your own pair and reject the public test ID in
[`.env.example`](.env.example). Neither Cargo nor TeleArk loads environment files
automatically, and the helpers never fall back to the example. Follow the
[scoped configuration instructions](docs/DEVELOPMENT.md#local-development-environment)
to create and protect the local file without overwriting existing values.

```sh
# macOS, Linux or Windows Git Bash
scripts/build-local.sh
scripts/run.sh
```

```powershell
# Windows PowerShell; PowerShell 7 users can substitute pwsh
powershell -NoProfile -File .\scripts\build-local.ps1
powershell -NoProfile -File .\scripts\run.ps1
```

For a single Windows invocation blocked by execution policy, add
`-ExecutionPolicy Bypass` before `-File`. Do not change machine-wide policy.
Personal credentials saved in the app override the embedded pair. Embedded
identifiers are extractable from executables; they do not authorize a Telegram
user. Rebuild after changing them.

The local build/run helpers and macOS packager support `--dry-run` to describe
operations without loading credentials or building. Other scripts have their own
arguments; see the [script guide](scripts/README.md).

### Isolated UI preview

Preview synthetic data without signing in or opening real application state:

```sh
cargo run -p teleark-gui --bin teleark --locked -- --preview-ui --screen=transfers --locale=en-US --window-size=900x600
```

Visual reviews use English and light mode at 900×600 and actual native full-screen.
The [development guide](docs/DEVELOPMENT.md#isolated-ui-review) lists preview routes,
interaction checks and test commands.

## Data and recovery

Encrypted storage protects remote content and original filenames. Telegram can
still observe account/channel relationships, ciphertext sizes and timing. The
local SQLite catalog is unencrypted and reveals metadata. macOS uses system
Keychain credentials by default; disabling it stores credentials and recovery
secrets in the private SQLite database. Windows and Linux currently use SQLite.
The optional PIN gates the interface and does not encrypt local storage. Telegram
sessions live in a separate adapter cache.

Recovery after database loss requires the matching key bundle, account access,
and retained manifests and encrypted parts. Keep the key separately. Files that
require different keys in one managed channel are not supported by the current
single-key desktop view; stored credentials and ciphertext remain intact.

Encrypted transfers stream through bounded memory with a 1.9 GiB encoded-container
ceiling. Native Telegram downloads verify byte length, without cryptographic
content authentication. macOS app binaries are self-signed, without Apple
notarization or Gatekeeper trust. Live-account, crash and platform qualification
limits are recorded in [implementation status](docs/IMPLEMENTATION_STATUS.md).
Read the [security contract](docs/SECURITY.md) for threat boundaries and private
vulnerability reporting guidance.

## Contributing

Start with [Development](docs/DEVELOPMENT.md) for checks and local setup,
[Architecture](docs/ARCHITECTURE.md) for module boundaries, and
[AGENTS.md](AGENTS.md) for contribution requirements. Ordinary tests and previews
use synthetic data; never include credentials, sessions, keys or personal files in
issues, fixtures or commits.

TeleArk is dual-licensed under [MIT](LICENSE-MIT) or
[Apache-2.0](LICENSE-APACHE), at your option. Distributions include
[third-party notices](THIRD_PARTY_NOTICES.md).
