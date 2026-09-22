# Development

Commands and preview fixtures for the repository's `rust-toolchain.toml` and checked-in lockfile. Contributor rules live in [AGENTS.md](../AGENTS.md).

## Quality gates

Plain-text source files use UTF-8 and LF on every platform (`.editorconfig` and
`.gitattributes`). The CI/CD workflow runs
`pwsh ./scripts/check-line-endings.ps1` against all tracked Git-index files,
rejecting CRLF text and all `.cmd`/`.bat` scripts. Each check writes counts and
violations to the GitHub Actions job summary. Run the same command before a commit;
Git-index inspection avoids false results from an older local Windows checkout.

Choose local validation by the changed behavior; CI remains the full workspace gate.

| Change | Local validation |
| --- | --- |
| Documentation only | Review the diff, local links/anchors and stale references; `git diff --check`. No Rust rebuild unless executable examples or build instructions changed. |
| One module | `cargo fmt --all --check`; affected crate tests and strict Clippy with `--all-targets --locked`. Include callers when the API changes. |
| UI / messages | Affected GUI tests and non-visual i18n/catalog checks. Interface testing and layout reviews use English (`en-US`) only, covering both 900×600 and actual full-screen mode. Visual previews and layout reviews use light mode only; existing automated dark-theme coverage may remain. Default-size or large-window previews do not replace full-screen checks. For long operations, include slow/blocked, phase-change and terminal feedback. |
| Storage / crypto / manifests | Supported-version and skipped-upgrade fixtures; preserved data/keys, restart/rollback, insufficient-space and visible migration-phase tests; affected canonical-vector, tamper and recovery tests. See [migration safety](SECURITY.md#validation-and-migration-safety). |
| Dependencies | Review the graph and notices; `cargo deny check` plus affected compilation/tests. |
| Release / shared contracts / broad refactor | Full commands below and any applicable protected release qualification. |

Full source gates (explicit Core/i18n checks also run in CI):

```bash
pwsh ./scripts/check-line-endings.ps1
pwsh ./scripts/test-line-endings.ps1
pwsh ./scripts/test-distribution-credentials.ps1
bash scripts/test-macos-libraries.sh
bash scripts/test-linux-payloads.sh
cargo fmt --all --check
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked -- --test-threads=1
cargo test -p teleark-core --locked
cargo test -p teleark-i18n --locked
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
cargo deny check
```

Long fuzzing, million-row benchmarks and credentialed Telegram tests run in separate protected/manual workflows. GPUI dependency provenance is recorded in [ADR 0012](adr/0012-gpui-kit-and-private-storage-channel.md).

GPUI Kit's development-only `test-support` feature provides real event/focus regression tests. Its reviewed test-only graph adds `convert_case 0.11.0` (MIT), plus `proptest 1.11.0`, `proptest-macro 0.5.0`, `quick-error 1.2.3`, `rand_xorshift 0.4.0`, `rusty-fork 0.3.1`, `unarray 0.1.4` and `wait-timeout 0.2.1` (MIT OR Apache-2.0). These helpers are excluded from release features.

## Native macOS build

```bash
cargo run -p teleark-gui --bin teleark
scripts/build-local.sh &&
  scripts/package-macos.sh target/release/teleark dist/TeleArk.app
```

The packaging script creates a native `.app` with Info.plist, the original application icon at standard/Retina sizes and license resources. The release archive contains this unsigned bundle.

GPUI Kit enables the macOS runtime-shader path, allowing development with Apple Command Line Tools without the standalone Metal compiler. Preserve that feature unless a replacement is validated. The tagged workflow builds both Apple Silicon and Intel slices; signing/notarization and clean-machine validation of the macOS 11 floor remain separate qualification.

Source builds use their own Telegram API ID/Hash configured from the login/settings UI. Release and package-preview builds require the repository secrets `TELEARK_DISTRIBUTION_TELEGRAM_API_ID` and `TELEARK_DISTRIBUTION_TELEGRAM_API_HASH`; CI validates them and passes them to every native release compilation. Both must be valid; personal saved credentials override them. Embedded identifiers are extractable and do not authorize a Telegram user. For local testing, `.env.example` provides the [officially published TEST ONLY pair](https://github.com/telegramdesktop/tdesktop/blob/dev/docs/api_credentials.md). These identifiers are server-limited and must not be used for distribution; obtain your own pair before publishing. Never log personal pairs or commit them to fixtures.

## Local development environment

Use `.env.local` for private development environment values. To initialize a new checkout, copy `.env.example` to `.env.local` only if the local file does not already exist. Use mode `0600` for local environment files. Fill values locally without putting them in shell command arguments or history. The root `.gitignore` excludes `.env` and `.env.*`, with `.env.example` explicitly allowed. Do not force-add local files; ignore rules do not untrack previously committed files.

Neither Cargo nor the application automatically loads dotenv files. A plain `cargo run -r` does not read `.env.example` or `.env.local`. `.env.example` is a template only: replace its sample values in `.env.local`, and never load the example for builds or packaging. `scripts/build-local.sh` and `scripts/build-local.ps1` enforce this local packaging workflow. For an authorized development run using the existing build-time Telegram credential variables, load the trusted, locally maintained file in a subshell from the repository root:

```bash
(
  set +x
  set -a
  . ./.env.local || exit 1
  set +a
  cargo run -r -p teleark-gui --bin teleark --locked
)
```

Or in Windows PowerShell:

```powershell
.\scripts\run.ps1
```

The variables apply to that subshell and its child processes. They are read at compile time by `option_env!`; changing them requires rebuilding through Cargo, not merely launching an already built binary. Supply the variables on every Cargo build/run that should embed them: a later plain `cargo run -r` uses its current environment and may rebuild without the defaults. The local packaging helper requires both values; personal credentials saved in the app still override the embedded pair. Do not print the file, dump the environment or enable shell tracing. Shell sourcing executes file contents, so source only your trusted local configuration. Use synthetic fixtures for ordinary tests and `--preview-ui` reviews; real startup can open existing state and resume eligible work.

The embedded pair identifies the application and is extractable from the resulting binary. This workflow keeps development values out of Git and logs; it does not prevent others from reusing identifiers in a distributed build. Session credentials and Vault keys have separate protections described in [Security](SECURITY.md#local-data-api-configuration-and-logs).

## Isolated UI review

Always include `--preview-ui` for layout work:

Use English (`--locale=en-US`) only for interface testing, visual previews, screenshots and layout reviews. Do not open other-language interfaces or add other-language UI passes for localization checks. Every interface test/layout review must cover both 900×600 and actual full-screen mode. Visual previews and layout reviews use light mode only; existing automated dark-theme coverage may remain. Enter full-screen mode through the native window control; a large `--window-size` or the default window is not a substitute. At both sizes, verify primary actions remain reachable and inspectors scroll independently.

```bash
cargo run -p teleark-gui -- --preview-ui --screen=transfers --locale=en-US --window-size=1360x760
cargo run -p teleark-gui -- --preview-ui --preview-state=unlock --locale=en-US --window-size=900x600
cargo run -p teleark-gui -- --preview-ui --preview-state=about --locale=en-US
```

`--preview-state=quit-confirm`, `quit-pausing` and `quit-failed` provide static exit-dialog fixtures. Native Close, Close Window and Quit always exit preview immediately, including these fixtures. Window shortcuts are listed in [ADR 0038](adr/0038-native-window-close-and-paused-exit.md).

Preview disables Library, Telegram, diagnostics, native-transfer and Vault runtime constructors. Fixtures contain a synthetic account, 200 channel titles, 5,000 raw rows, Unicode managed files and native/upload batches with local-file states; a missing real runtime never produces fake transfer success. Preview actions cannot authenticate or move real Telegram data. `--preview-state` and `--preview-dark` are interpreted only in preview mode.

| Option | Values |
| --- | --- |
| `--screen` | `account`, `storage`, `channel`, `transfers`, `library`, `file`, `settings`, `upload` |
| `--preview-state` | `login`, `returning`, `setup`, `locked`, `raw`, `channel-selected`, `local-availability`, `batch-groups`, `batch-large`, `batch-deleted`, `upload-history`, `native-failure`, `transfer-rate-limited`, `managed-key-loading`, `upload-folder`, `unlock`, `about`, `appearance`, `upload-preflight`, `upload-progress`, `channel-sync`, `channel-sync-wait`, `dialogs-failed`, `dialogs-waiting`, `proxy-ready`, `proxy-failed`, `proxy-testing` |
| `--locale` | `en-US` only for interface tests and previews |
| `--window-size` | 900×600 is required; also review actual native full-screen mode. Other window sizes are supplemental |

The default content size is 1120×680. Startup centers the native frame inside the primary display’s OS-reported work area, excluding the menu bar and Dock/taskbar, with a 16-point margin and separate 36-point native-titlebar allowance. Oversized requests shrink to fit; on unusually small work areas the window minimum also shrinks instead of forcing overlap. Record actual size separately. The legacy `--skip-telegram-api-id-prompt` flag remains accepted, but API setup is now opt-in. Non-preview startup opens real local state and can resume eligible downloads after account entry.

Channel sync fixtures show an active difference or a server wait with queue/timing and an independently scrolling timeline; they never create the real synchronization owner. Startup now shows database opening/migration before initializing runtime owners in the background.

`--preview-state=upload-folder` shows the explicit folder/application-bundle rejection above the upload composer’s scroll area. Check the message and wrapping in English only.

`--preview-state=channel-selected` opens a synthetic channel with the second row focused and checked, so selection overlays, text and checkbox visibility can be reviewed together.

`--screen=channel --preview-state=local-availability --locale=en-US` shows Checking, Deleted from disk, Local file changed, Local file unavailable and Available locally on consecutive synthetic channel rows, plus the bounded-observation notice. Inspect the row details at 900×600 and actual native full-screen in light mode. This fixture does not inspect or modify real files.

`--preview-state=batch-groups` shows adjacent expanded download/upload groups between ordinary tasks. Use `--screen=transfers --preview-state=batch-large` and activate the 48-file upload header to review the same hierarchy, selection and scrollbar in its auxiliary window.

`--screen=transfers --preview-state=batch-deleted --locale=en-US` shows a selected completed native batch with two deleted outputs and one present output. Review the enabled toolbar and batch-header Retry controls at 900×600 and actual native full-screen in light mode. The preview never starts real downloads.

Inspect actual affected windows, including keyboard/focus, wrapping, scrolling and loading/error states. The full release matrix also covers login/returning sessions, storage setup/Files/Raw, unlock, transfer and Library bulk actions/details, batch membership, sidebar states, file picker/removal, local-file states and Settings/About. Process survival is not visual verification. Never capture real QR tokens or recovery secrets. CUA/AppKit inspection requires an unlocked Mac.

## CI and releases

[`ci.yml`](../.github/workflows/ci.yml) is the only CI/CD workflow. Its first job runs LF checks, the legal baseline and the full locked Linux source gates. Pull requests and ordinary manual runs then run Windows and macOS tests. A matching `vX.Y.Z` tag instead resolves the Cargo version, builds Windows x64, universal macOS and Linux x64 packages, and publishes one exact nine-file release manifest after every package verifies. Branch pushes do not trigger a run: pushing a branch and its version tag together starts only the tag run, which validates the published commit before packaging. Direct pushes to `main` require a pull request or manual dispatch to receive CI checks.

Optional manual dispatch can preview packages with a commit-suffixed filename; it never publishes. The stage summary reports job results, and the individual job summaries show LF counts, exact Rust cache hits and SHA256 checksums. CI installs pinned `cargo-deny 0.20.2` as a native tool. `fuzz.yml` remains a separate scheduled parser campaign and produces no desktop package. Mac packages remain unsigned and unnotarized; a tag is not evidence of signing, a security audit or credentialed testing. See [packaging](PACKAGING.md), [ADR 0043](adr/0043-release-artifact-and-installer-version-contract.md), [ADR 0044](adr/0044-single-workflow-native-release-matrix.md) and [ADR 0045](adr/0045-focused-release-targets.md).

After workflow edits, run `actionlint` and the affected commands locally. To smoke-test the parser campaigns on a supported local host:

```bash
cargo +nightly install cargo-fuzz --version 0.13.2 --locked
cargo +nightly fuzz run part_decode -- -max_total_time=30 -rss_limit_mb=2048
cargo +nightly fuzz run manifest_decode -- -max_total_time=30 -rss_limit_mb=2048
```

These short local campaigns do not substitute for the scheduled ten-minute campaigns or a successful hosted workflow run.

Protected system, security and platform qualification remains tracked in [implementation status](IMPLEMENTATION_STATUS.md#next-actions-and-release-gates).

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

## Proxy validation

`cargo test -p teleark-telegram --locked network::tests` runs actual loopback socket routing and negative direct-destination traps, including MTProto bootstrap traffic and environment bypass variables. Runtime owner tests check shutdown barriers, restart persistence and probe cancellation/admission; GUI network policy tests block a real framework HTTP request and guard alternate network exits. Run full workspace gates when changing this shared route contract. No real session or external proxy is needed.

Use `--preview-ui --preview-state=proxy-failed --locale=en-US --window-size=900x600` for an isolated persistent-error settings preview, or `proxy-testing`/`proxy-ready` for waiting/configured states. These are synthetic presentation states and do not dial a proxy or measure latency. A successful real settings test means a TCP tunnel to a Telegram DC was accepted, not authenticated API health.


`--screen=transfers --preview-state=upload-history --locale=en-US` shows an expanded restored upload batch with completed/interrupted members, saved totals, omitted-history count and interruption guidance. No real history or account is accessed.

`--screen=transfers --preview-state=native-failure --locale=en-US` shows a synthetic 60-second native part timeout, its connection slot and the right-hand failure inspector. Use it at 900×600 and actual native full-screen in light mode.

`--screen=transfers --preview-state=transfer-rate-limited --locale=en-US` shows a synthetic Telegram cooldown with a long ETA at 900×600 and actual native full-screen. `--preview-state=managed-key-loading` shows the key activity button in the bottom status bar; open it to check the lateral phase inspector and its independent scroll area.


## Task completion checkpoints

After completing and validating each repository task, commit only its changes and create an annotated Git tag (for example `fix/YYYYMMDD-short-description`). Preserve unrelated pending work; do not push commits or tags without a request. Use English/light-only previews at 900×600 and actual native full-screen for visual changes.


### Application PIN preview

`--preview-ui --preview-state=locked --locale=en-US --window-size=900x600` shows
the whole-application PIN surface (`--preview-state=locked-transfers` adds a
synthetic active upload for lifecycle prompt checks) while background projections remain
available. `--preview-state=unlock` continues to exercise file-key setup/recovery,
which is independent of the application PIN. Review the locked proxy editor and
account-switch prompt at 900×600 and native full-screen, then review General →
Application lock and Network proxy → Sync activity and logs. Sync footer clicks
must not open an inspector. Previews never save a PIN to real application state.
