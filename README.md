# TeleArk

TeleArk is a desktop file library, media indexer, transfer manager, and client-side encrypted storage application backed by Telegram. Its user-facing abstraction is a **Logical File**: multipart objects, Telegram messages, and MTProto details remain implementation details.

> [!IMPORTANT]
> TeleArk is an early engineering prototype. This milestone establishes repository policy, architecture contracts, localization foundations, and a GPUI mock-data interface. Telegram login, persistent SQLite storage, production indexing, real transfers, encryption, manifests, and disaster recovery are not implemented yet. Do not entrust data to this prototype.

## Product direction

TeleArk is designed to provide:

- a local SQLite/FTS5 index for million-message Telegram channels;
- global file search, facets, collections, and cursor-paginated virtual lists;
- resumable upload and download with explicit verification;
- free-account-compatible application multipart storage (target default: 1900 MiB);
- bounded, framed AES-256-GCM content encryption with BLAKE3 integrity checks;
- versioned manifests that allow recovery when the local database is lost;
- reusable Core APIs for the GPUI app and a future CLI;
- `en-US`, `zh-CN`, and `ja-JP` localization from the first release.

The architecture and on-disk/on-remote formats in `docs/` are design contracts. Fields marked **provisional** must not be treated as a released format until implementation, security review, fixtures, and compatibility tests are complete.

## Architecture at a glance

```text
GPUI frontend        Future CLI / frontend
        \                 /
         frontend-neutral Core API
          /       |       \
  Index Engine  Transfer  Vault/Crypto
          \       |       /
         SQLite ports   Telegram ports
                          |
                       grammers
```

GPUI types may not enter Core. The GUI may not execute SQL, call `grammers`, encrypt data, or own transfer checkpoints. Infrastructure adapters map external types and errors into TeleArk-owned domain types.

Read [the architecture guide](docs/ARCHITECTURE.md) and [implementation status](docs/IMPLEMENTATION_STATUS.md) before interpreting the roadmap as implemented behavior.

## Development

The repository is a Rust workspace with separate Core, i18n, and GPUI frontend foundations. Additional Storage, Telegram, Crypto, Index, Transfer, CLI, and test-support crates will be introduced without weakening dependency direction.

```bash
cargo run -p teleark-gui --bin teleark
cargo fmt --all --check
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check
```

The current executable is an interactive mock-data shell. Its header always
shows a localized **Mock data** badge. For deterministic UI review, launch a
specific route and locale:

```bash
cargo run -p teleark-gui -- --screen=transfers --locale=zh-CN
```

Supported screen values are `library`, `transfers`, `file`, `vault`, `channel`,
`settings`, and `upload`; supported locale values negotiate to `en-US`,
`zh-CN`, or `ja-JP`. Search, sidebar facets, queue insertion, pause/resume,
vault lock/reveal, locale switching, keyboard route activation, and closing the
upload overlay with Escape are presentation-only interactions over deterministic fixtures.

See [development guidance](docs/DEVELOPMENT.md) and [agent/contributor rules](AGENTS.md). Normal CI must remain deterministic and must never require a real Telegram account, phone number, 2FA secret, or session file.

## Security and recovery

The target design uses envelope encryption: an Argon2id-derived password key unwraps a random Vault Master Key, which unwraps random per-file keys. Content is split into authenticated AES-256-GCM frames and described by an encrypted, versioned manifest. These are **design goals, not current capabilities**.

Telegram can still observe account/channel relationships, ciphertext sizes, message counts, timing, and traffic patterns. TeleArk does not promise anonymity or zero metadata leakage. Read [the security model](docs/SECURITY.md), [crypto format](docs/CRYPTO_FORMAT.md), and [manifest format](docs/MANIFEST_FORMAT.md).

## Clean-room policy

TeleArk is independently engineered from permissively licensed documentation and public specifications. **Do not inspect, copy, translate, adapt, or derive implementation details from the GPL-licensed `tdl` project.** Do not ask another person or agent to inspect it on TeleArk's behalf. The same caution applies to other incompatible or unclear sources. See [ADR 0007](docs/adr/0007-permissive-license-clean-room-policy.md).

## Documentation

- [Architecture](docs/ARCHITECTURE.md)
- [Development](docs/DEVELOPMENT.md)
- [Data model](docs/DATA_MODEL.md)
- [Crypto format](docs/CRYPTO_FORMAT.md)
- [Manifest format](docs/MANIFEST_FORMAT.md)
- [Transfer engine](docs/TRANSFER_ENGINE.md)
- [Index engine](docs/INDEX_ENGINE.md)
- [UI guidelines](docs/UI_GUIDELINES.md)
- [Internationalization](docs/I18N.md)
- [Security](docs/SECURITY.md)
- [Implementation status](docs/IMPLEMENTATION_STATUS.md)
- [Architecture decisions](docs/adr/)

## License

Licensed under either of Apache License, Version 2.0 or MIT license at your option.

- [Apache-2.0](LICENSE-APACHE)
- [MIT](LICENSE-MIT)
- [Third-party notices](THIRD_PARTY_NOTICES.md)

Contributions are understood to be offered under the same dual-license terms unless explicitly stated otherwise.
