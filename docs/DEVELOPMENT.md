# Development

Use `rust-toolchain.toml` and the checked-in lockfile. Start with [AGENTS.md](../AGENTS.md); [implementation status](IMPLEMENTATION_STATUS.md) is the capability/limitation record.

## Quality gates

```bash
cargo fmt --all --check
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
cargo test -p teleark-core --locked
cargo test -p teleark-i18n --locked
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
cargo deny check
```

Use deterministic temporary databases/files, fake remote stores, injected clocks and bounded workers. Reproducible fixes require regression tests; migrations preserve data from every supported prior version. Crypto/manifest changes preserve canonical vectors, tamper rejection and recovery equality. Keep long fuzzing, million-row benchmarks and credentialed Telegram tests in separate protected/manual workflows. Never point tests at a developer's real session or database.

Before dependency changes, review direct/transitive licenses, maintenance and advisories. The published GPUI Kit/gpui-pre graph and exact permissive exceptions are recorded in [ADR 0012](adr/0012-gpui-kit-and-private-storage-channel.md); the unprefixed GPL graph remains banned. Distributions include both licenses and `THIRD_PARTY_NOTICES.md`. A passing deny policy does not imply every transitive crate is maintained forever.

GPUI Kit's development-only `test-support` feature drives actual wheel-event regression tests. Its eight newly locked dependencies were reviewed: `convert_case 0.11.0` (MIT), `proptest 1.11.0`, `proptest-macro 0.5.0`, `quick-error 1.2.3`, `rand_xorshift 0.4.0`, `rusty-fork 0.3.1`, `unarray 0.1.4` and `wait-timeout 0.2.1` (MIT OR Apache-2.0). These are existing upstream test helpers, excluded from release features; no existing package was upgraded. Necessity is the real scroll/focus harness; the full locked advisory/license gate remains required.

## Native macOS build

```bash
cargo run -p teleark-gui --bin teleark
cargo build -p teleark-gui --release --locked
scripts/package-macos.sh target/release/teleark dist/TeleArk.app
```

The packaging script creates a native `.app` with Info.plist, the original application icon at standard/Retina sizes and license resources. The release archive contains this unsigned bundle.

GPUI Kit enables the macOS runtime-shader path, allowing development with Apple Command Line Tools without the standalone Metal compiler. Preserve that feature unless a replacement is validated. A signed/notarized release and the macOS deployment floor, Apple Silicon/Intel matrix and other desktop platforms need separate qualification.

Source builds use their own Telegram API ID/Hash configured from the login/settings UI. Distributors may set `TELEARK_DISTRIBUTION_TELEGRAM_API_ID` and `TELEARK_DISTRIBUTION_TELEGRAM_API_HASH` through protected build secrets. Both must be valid; personal saved credentials override them. Embedded identifiers are extractable and do not authorize a Telegram user. Never reuse Telegram Desktop credentials, log real pairs or commit them to fixtures.

## Isolated UI review

Always include `--preview-ui` for layout work:

```bash
cargo run -p teleark-gui -- --preview-ui --screen=transfers --locale=en-US --window-size=1360x760
cargo run -p teleark-gui -- --preview-ui --preview-state=unlock --locale=zh-CN --window-size=900x600
cargo run -p teleark-gui -- --preview-ui --preview-state=about --preview-dark --locale=ja-JP
```

Preview disables Library, Telegram, diagnostics, native-transfer and Vault runtime constructors. Fixtures contain a synthetic account, 200 channel titles, 5,000 raw rows, Unicode managed files and native/upload batches with local-file states; a missing real runtime never produces fake transfer success. Preview actions cannot authenticate or move real Telegram data. `--preview-state` and `--preview-dark` are interpreted only in preview mode.

| Option | Values |
| --- | --- |
| `--screen` | `account`, `storage`, `channel`, `transfers`, `library`, `file`, `settings`, `upload` |
| `--preview-state` | `login`, `returning`, `setup`, `locked`, `raw`, `unlock`, `about`, `appearance`, `upload-preflight`, `upload-progress` |
| `--locale` | `en-US`, `zh-CN`, `ja-JP` |
| `--window-size` | Minimum 900×600; review 960×640, 1360×760, 1920×1080 |

Oversized windows fit the active display; record actual size separately. The legacy `--skip-telegram-api-id-prompt` flag remains accepted, but API setup is now opt-in. Non-preview startup opens real local state and can resume eligible downloads after account entry.

Inspect actual windows, not only process startup: navigation after refresh/long scroll; login and returning session; storage setup/Files/Raw/guide; locked upload → unlock; modal focus/Tab/Return/Escape; transfer bulk actions/details, batch membership, scroll isolation at both boundaries, expanded/collapsed navigation, multi-file picker/removal and local-file states; Settings/About; light/dark and all locales. Never capture a real QR token or recovery secret. Record blocked or unperformed checks honestly in status. CUA/AppKit inspection requires an unlocked Mac.

## CI and releases

`ci.yml` runs legal checks, cargo-deny, formatting, check, Clippy, workspace tests and explicit Core/i18n checks. `fuzz.yml` runs bounded daily parser campaigns. `release.yml` is an unsigned macOS bootstrap with checksums; a version tag is not evidence of signing, security audit or credentialed testing. Keep secrets in protected release environments and use least-privilege permissions.

For substantial changes, review the full diff, update affected contracts and record justified gate exceptions. Commit only when authorized. Use [implementation status](IMPLEMENTATION_STATUS.md) for unfinished work and precise next actions, not a growing chronology of every command.

## Documentation recovery checkpoints

The v0.4.0 consolidation preserves the complete prior document tree at `checkpoint/pre-doc-consolidation-20260907` and the compact result at `checkpoint/post-doc-consolidation-20260907`. To inspect an old file without changing the worktree:

```bash
git show checkpoint/pre-doc-consolidation-20260907:docs/UI_REVIEW.md
```

To restore only the documentation after reviewing local changes:

```bash
git restore --source=checkpoint/pre-doc-consolidation-20260907 -- AGENTS.md docs
```

`checkpoint/pre-gpui-kit-redesign-20260907` also preserves the pre-rewrite repository. Restoring source does not downgrade an already migrated user database; never replace or remove user data for a code rollback.
