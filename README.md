# TeleArk

TeleArk is a desktop file library for people who keep large media and document
collections in Telegram. It is being built to make those collections feel like
ordinary files: searchable, organized, resumable, verifiable, and recoverable
without asking you to manage Telegram message IDs or multipart pieces.

The central object in TeleArk is always one **logical file**. A movie may
eventually be stored as dozens of encrypted Telegram objects, but the library
will still present one name, one size, one transfer, and one recovery record.

> [!IMPORTANT]
> TeleArk is an early alpha. The local library and bounded Telegram channel
> browsing/download described below are functional. Encrypted desktop uploads,
> encrypted recovery, and Vault unlock flows are not complete. Do not rely on
> this build as the only copy of important data or secrets.

## What works today

The current desktop build provides a useful local catalog and Telegram index:

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
- choose a real Telegram source and browse downloadable documents page by page;
- switch between channels without mixing their results;
- choose a destination and download a Telegram document through the bounded
  desktop transfer queue;
- see real queued, running, completed, and failed downloads on Transfers;
- scan a source history in bounded pages;
- retain Telegram file identities and scan progress in SQLite so later scans
  continue from the last committed page.

Importing local files records metadata only. TeleArk does not copy, move, alter,
or upload those selected files. Native Telegram downloads are real. Upload, Key
Vault, encrypted desktop recovery, and most Settings content remain Preview.

## Run TeleArk

There is no signed installer yet. On macOS, install the stable Rust toolchain,
open this repository in Terminal, and run:

```bash
cargo run -p teleark-gui --bin teleark
```

Open **Library** to import local files, or open **Telegram Sources** to connect
an account, browse channel documents, download them, and optionally add them to
the persistent local index. **Transfers** shows downloads started in this run.

An official TeleArk distribution can include API credentials registered by its
distributor, making Telegram sign-in available immediately. Source builds that
do not include a distributor pair ask for the API ID and API Hash assigned to
your own Telegram application. You can create them in Telegram's
[API development panel](https://my.telegram.org/apps), skip setup to use the
local Library only, and add or remove the pair later in **Settings**. Personal
credentials override the distributor pair. TeleArk never embeds or reuses
Telegram Desktop credentials.

The default catalog is stored at:

```text
~/Library/Application Support/TeleArk/library.sqlite3
```

For a particular screen, language, or test window size:

```bash
cargo run -p teleark-gui -- --screen=library --locale=ja-JP --window-size=960x640
```

Available screen values are `library`, `transfers`, `file`, `vault`, `channel`,
`settings`, and `upload`. Locale values are `en-US`, `zh-CN`, and `ja-JP`.
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
- The Telegram Sources route contacts Telegram only after you provide
  credentials and choose an action. QR login links stay in memory and are
  redacted from debug output. Native download tasks are real; Upload and Vault
  remain previews.
- The current encryption and manifest implementation is still provisional and
  is not wired into the desktop encrypted-storage workflow.
- Native downloads verify Telegram's declared byte length and publish the final
  path only after a temporary file is complete. They do not yet provide
  cryptographic content verification, restart resume, pause, or retry controls.
- A future encrypted Vault cannot hide all metadata from Telegram; account and
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
