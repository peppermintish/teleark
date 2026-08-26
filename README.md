# TeleArk

TeleArk is a desktop file library for people who keep large media and document
collections in Telegram. It is being built to make those collections feel like
ordinary files: searchable, organized, resumable, verifiable, and recoverable
without asking you to manage Telegram message IDs or multipart pieces.

The central object in TeleArk is always one **logical file**. A movie may
eventually be stored as dozens of encrypted Telegram objects, but the library
will still present one name, one size, one transfer, and one recovery record.

> [!IMPORTANT]
> TeleArk is an early alpha. The local library described below is functional,
> while Telegram account setup, channel indexing, uploads, downloads, Vault,
> and recovery are not yet connected to the desktop interface. Do not rely on
> this build as the only copy of important data or secrets.

## What works today

The current desktop build provides a useful local catalog:

- import one or more files with the macOS file picker;
- retain the catalog between launches in a local SQLite database;
- search imported filenames with a local full-text index;
- filter the library by common file types;
- load additional result pages for larger catalogs;
- inspect real names, sizes, types, dates, paths, and local status;
- open an imported file or reveal its original location in Finder;
- switch live between English, Simplified Chinese, and Japanese, remember an
  explicit choice, or follow the system language;
- use compact, standard, and large desktop window layouts.

Importing records file metadata only. TeleArk does not copy, move, alter, or
upload the selected file in this alpha. The Transfer, Channel Index, Upload,
Key Vault, and most Settings content is visibly marked **Preview** because it
still uses demonstration data.

## Run TeleArk

There is no signed installer yet. On macOS, install the stable Rust toolchain,
open this repository in Terminal, and run:

```bash
cargo run -p teleark-gui --bin teleark
```

Open **Library**, choose **Import files**, and select the files you want to add
to the catalog. Search and file-type filters operate on the persistent local
index.

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
- Preview transfer and channel rows are fictional and do not contact Telegram.
- The current encryption and manifest implementation is still provisional and
  is not wired into desktop storage workflows.
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
