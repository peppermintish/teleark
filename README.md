# TeleArk

TeleArk is a desktop file library for people who keep large media and document
collections in Telegram. It is being built to make those collections feel like
ordinary files: searchable, organized, resumable, verifiable, and recoverable
without asking you to manage Telegram message IDs or multipart pieces.

The central object in TeleArk is always one **logical file**. A movie may
eventually be stored as dozens of encrypted Telegram objects, but the library
will still present one name, one size, one transfer, and one recovery record.

> [!IMPORTANT]
> TeleArk is an early alpha. The local library, Telegram browsing/native
> downloads, and the Vault-backed Saved Messages upload/scan/restore path are
> connected. The cryptographic formats are still provisional, encrypted task
> checkpoints/controls and OS credential integration are incomplete, and no
> independent security review has been completed. Do not rely on this build as
> the only copy of important data or secrets.

## What works today

The current desktop build provides a Telegram channel browser, Saved Messages
package view, local catalog, and Telegram index:

- import one or more files with the macOS file picker;
- retain the catalog between launches in a local SQLite database;
- search imported filenames with a local full-text index;
- filter the library by common file types;
- load additional result pages for larger catalogs;
- inspect real names, sizes, types, dates, paths, and local status;
- open an imported file or reveal its original location in Finder;
- switch live between English, Simplified Chinese, and Japanese, remember an
  explicit choice, or follow the system language;
- use compact, standard, and large desktop window layouts;
- sign in to Telegram by scanning a short-lived QR code, or with a login code
  and optional two-step verification;
- choose a real Telegram channel directly from the sidebar and browse
  downloadable documents page by page;
- inspect Saved Messages either as raw Telegram files or as authenticated
  TeleArk logical files reconstructed from manifests;
- create and unlock a Key Vault from Settings, change its password, rotate and
  explicitly export a self-contained Recovery Bundle, or restore it after the
  local database is lost;
- select a local file from Saved Messages, encrypt its content/name/metadata,
  upload and verify its parts, then publish an authenticated Manifest;
- restore a managed TeleArk file through authenticated decryption, whole-file
  verification, and atomic non-overwriting publication;
- switch between channels without mixing their results;
- download a Telegram document automatically into the configured TeleArk
  managed-files location through the bounded desktop transfer queue;
- see real queued, running, completed, and failed downloads on Transfers;
- inspect per-download timing, average speed, verification state, lifecycle
  events, and localized failure reasons, with privacy-bounded JSON diagnostics;
- scan a source history in bounded pages;
- retain Telegram file identities and scan progress in SQLite so later scans
  continue from the last committed page.

Legacy local Library import records metadata only. The Saved Messages Upload
action is separate and performs a real encrypted Telegram upload after the Key
Vault is unlocked. Native Telegram downloads and managed Vault restores are
also real alpha workflows.

## Run TeleArk

There is no signed installer yet. On macOS, install the stable Rust toolchain,
open this repository in Terminal, and run:

```bash
cargo run -p teleark-gui --bin teleark
```

The app opens on **Channels**. Connect an account, choose a channel by its real
name, browse/download documents, or use the two **Saved Messages** views to see
raw Telegram objects and recognized TeleArk packages. **Transfers** separates
uploads and downloads and shows detailed direction, performance, encoding, and
verification information.

An official TeleArk distribution can include API credentials registered by its
distributor, making Telegram sign-in available immediately. Source builds that
do not include a distributor pair ask for the API ID and API Hash assigned to
your own Telegram application. You can create them in Telegram's
[API development panel](https://my.telegram.org/apps), skip the startup prompt,
and add or remove the pair later in **Settings**. Personal
credentials override the distributor pair. TeleArk never embeds or reuses
Telegram Desktop credentials.

The default catalog is stored at:

```text
~/Library/Application Support/TeleArk/library.sqlite3
```

For a particular screen, language, or test window size:

```bash
cargo run -p teleark-gui -- --screen=channel --locale=ja-JP --window-size=960x640
```

Available screen values are `library`, `transfers`, `file`, `channel`,
`settings`, and `upload`. Key Vault is intentionally available only inside
Settings. Locale values are `en-US`, `zh-CN`, and `ja-JP`.
Window sizes below `900x600` are raised to the supported compact minimum.
Requests larger than the active display are fitted to its visible bounds.

## Where TeleArk is heading

The intended complete workflow is:

1. Connect one or more Telegram accounts and choose private storage channels.
2. Index existing channel media into a fast local library.
3. Search and organize logical files without browsing message history.
4. Queue resumable uploads and downloads with integrity verification.
5. Optionally protect content, filenames, and metadata with a personal Vault.
6. Rebuild the local library from remote manifests after losing the computer or
   database.

TeleArk is designed around ordinary Telegram account limits. Large files use a
compatibility-oriented multipart layout rather than depending on Premium-only
upload limits.

## Data and safety

- The local catalog contains filenames, source paths, sizes, and timestamps.
  Protect your macOS account and backups accordingly.
- A personal API ID and API Hash are stored in the same local SQLite database
  so they survive restarts. SQLite is not encrypted; anyone who can read the
  database or its backups can read the API Hash. Distributor credentials are
  compiled into that distributor's build instead. Telegram login sessions
  remain in a separate protected session cache.
- The Channels and Saved Messages routes contact Telegram only after you provide
  credentials and choose an action. QR login links stay in memory and are
  redacted from debug output. Native downloads and Vault-backed Saved Messages
  upload/restore are connected alpha workflows.
- The current encryption, manifest, Recovery Key, and Recovery Bundle formats
  remain provisional. Keep independent copies of source data and recovery
  material; replacing the current recovery record does not revoke old exported
  Recovery Bundles.
- OS Credential is shown disabled in Settings because its platform adapter is
  still under development. Password and explicit offline Recovery Bundle flows
  are the available unlock/recovery methods.
- Native downloads verify Telegram's declared byte length and publish the final
  path only after a temporary file is complete. They support restart resume and
  pause/resume/cancel/retry controls, but do not yet provide a cryptographic
  content hash.
- The encrypted Vault cannot hide all metadata from Telegram; account and
  channel relationships, timing, ciphertext sizes, and message counts may
  remain observable.

See the [security model](docs/SECURITY.md) and
[implementation status](docs/IMPLEMENTATION_STATUS.md) for precise current
limitations.

## More information

- [Implementation status](docs/IMPLEMENTATION_STATUS.md)
- [Security model](docs/SECURITY.md)

## License

Licensed under either of Apache License, Version 2.0 or MIT license at your
option.

- [Apache-2.0](LICENSE-APACHE)
- [MIT](LICENSE-MIT)
- [Third-party notices](THIRD_PARTY_NOTICES.md)
