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
scripts/build-local.sh &&
  scripts/package-macos.sh target/release/teleark dist/TeleArk.app
```

The packaging script creates a native `.app` with Info.plist, the original application icon at standard/Retina sizes and license resources. The release archive contains this unsigned bundle.

GPUI Kit enables the macOS runtime-shader path, allowing development with Apple Command Line Tools without the standalone Metal compiler. Preserve that feature unless a replacement is validated. A signed/notarized release and the macOS deployment floor, Apple Silicon/Intel matrix and other desktop platforms need separate qualification.

Source builds use their own Telegram API ID/Hash configured from the login/settings UI. Distributors may set `TELEARK_DISTRIBUTION_TELEGRAM_API_ID` and `TELEARK_DISTRIBUTION_TELEGRAM_API_HASH` through protected build secrets. Both must be valid; personal saved credentials override them. Embedded identifiers are extractable and do not authorize a Telegram user. For local testing, `.env.example` provides the [officially published TEST ONLY pair](https://github.com/telegramdesktop/tdesktop/blob/dev/docs/api_credentials.md). These identifiers are server-limited and must not be used for distribution; obtain your own pair before publishing. Never log personal pairs or commit them to fixtures.

## Local agent memory and development environment

Keep temporary coding-agent notes and handoffs in `.agent-memory/`, with a concise `README.md` recording current context, validation and next steps. These notes are local, disposable working context; keep shared contracts in tracked documentation and credential values out of notes. Agents should consult and refresh the notes during ongoing development, checking them against the current working tree.

Use `.env.local` for private development environment values. To initialize a new checkout, create `.agent-memory/` with mode `0700` and copy `.env.example` to `.env.local` only if the local file does not already exist. Use mode `0600` for local notes and environment files. Fill values locally without putting them in shell command arguments or history. The root `.gitignore` excludes the memory directory, `.env` and `.env.*`, with `.env.example` explicitly allowed. Do not force-add local files; ignore rules do not untrack previously committed files.

Neither Cargo nor the application automatically loads dotenv files. A plain `cargo run -r` does not read `.env.example` or `.env.local`. `.env.example` is a template only: replace its sample values in `.env.local`, and never load the example for builds or packaging. `scripts/build-local.sh` enforces this local packaging workflow. For an authorized development run using the existing build-time Telegram credential variables, load the trusted, locally maintained file in a subshell from the repository root:

```bash
(
  set +x
  set -a
  . ./.env.local || exit 1
  set +a
  cargo run -r -p teleark-gui --bin teleark --locked
)
```

The variables apply to that subshell and its child processes. They are read at compile time by `option_env!`; changing them requires rebuilding through Cargo, not merely launching an already built binary. Supply the variables on every Cargo build/run that should embed them: a later plain `cargo run -r` uses its current environment and may rebuild without the defaults. The local packaging helper requires both values; personal credentials saved in the app still override the embedded pair. Do not print the file, dump the environment or enable shell tracing. Shell sourcing executes file contents, so source only your trusted local configuration. Use synthetic fixtures for ordinary tests and `--preview-ui` reviews; real startup can open existing state and resume eligible work.

The embedded pair identifies the application and is extractable from the resulting binary. This workflow keeps development values out of Git and logs; it does not prevent others from reusing identifiers in a distributed build. Session credentials and Vault keys have separate protections described in [Security](SECURITY.md#local-data-api-configuration-and-logs).

## Isolated UI review

Visual previews and layout reviews use English (`en-US`) and light mode only. Cover 900×600 and actual native full-screen mode; a large window does not replace full-screen. Existing automated dark-theme coverage may remain.

Always include `--preview-ui` for layout work:

Use English (`--locale=en-US`) only for interface tests and visual previews.

```bash
cargo run -p teleark-gui -- --preview-ui --screen=transfers --locale=en-US --window-size=1360x760
cargo run -p teleark-gui -- --preview-ui --preview-state=unlock --locale=en-US --window-size=900x600
cargo run -p teleark-gui -- --preview-ui --preview-state=about --locale=en-US
```

Preview disables Library, Telegram, diagnostics, native-transfer and Vault runtime constructors. Fixtures contain a synthetic account, 200 channel titles, 5,000 raw rows, Unicode managed files and native/upload batches with local-file states; a missing real runtime never produces fake transfer success. Preview actions cannot authenticate or move real Telegram data. `--preview-state` and `--preview-dark` are interpreted only in preview mode.

| Option | Values |
| --- | --- |
| `--screen` | `account`, `storage`, `channel`, `transfers`, `library`, `file`, `settings`, `upload` |
| `--preview-state` | `upload-history`, `login`, `returning`, `setup`, `locked`, `raw`, `unlock`, `about`, `appearance`, `upload-preflight`, `upload-progress` |
| `--locale` | `en-US`, `zh-CN`, `ja-JP` |
| `--window-size` | Minimum 900×600; review 960×640, 1360×760, 1920×1080 |

The default content size is 1120×680. Startup centers the native frame inside the primary display’s OS-reported work area, excluding the menu bar and Dock/taskbar, with a 16-point margin and separate 36-point native-titlebar allowance. Oversized requests shrink to fit; on unusually small work areas the window minimum also shrinks instead of forcing overlap. Record actual size separately. The legacy `--skip-telegram-api-id-prompt` flag remains accepted, but API setup is now opt-in. Non-preview startup opens real local state and can resume eligible downloads after account entry.

`--preview-state=upload-folder` shows the explicit folder/application-bundle rejection above the upload composer’s scroll area. Check English by default; use other locales for the localized message and wrapping checks.

Inspect actual windows, not only process startup: navigation after refresh/long scroll; login and returning session; storage setup/Files/Raw/guide; locked upload → unlock; modal focus/Tab/Return/Escape; transfer bulk actions/details, batch membership, scroll isolation at both boundaries, expanded/collapsed navigation, multi-file picker/removal and local-file states; Settings/About; English light mode at 900×600 and actual native full-screen. Never capture a real QR token or recovery secret. Record blocked or unperformed checks honestly in status. CUA/AppKit inspection requires an unlocked Mac.

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


`--screen=transfers --preview-state=upload-history --locale=en-US` shows an expanded restored upload batch with completed/interrupted members, saved totals, omitted-history count and interruption guidance. No real history or account is accessed.


## Task completion checkpoints

After completing and validating each repository task, commit only its changes and create an annotated Git tag (for example `fix/YYYYMMDD-short-description`). Preserve unrelated pending work; do not push commits or tags without a request. Use English/light-only previews at 900×600 and actual native full-screen for visual changes.
