# Build, packaging and verification scripts

Start with [Development](../docs/DEVELOPMENT.md) for source builds and checks, or
[Packaging](../docs/PACKAGING.md) for native packages. Run the examples from the
repository root. The [CI workflow](../.github/workflows/ci.yml) shows the complete
release sequence and pinned build tools.

These scripts have different responsibilities. A build helper compiles the app;
a packager assembles an existing executable; a release verifier exercises the
resulting package. Tests below cover those helpers with synthetic data or native
installation fixtures. They are kept separate so ordinary source checks do not
install applications or need release credentials.

## Local build and launch

| Script | Purpose |
| --- | --- |
| [`build-local.sh`](build-local.sh), [`build-local.ps1`](build-local.ps1) | Load the repository's private `.env.local`, validate its application identifiers, and run the locked release build. `--run` builds and launches instead. Restrict the file's permissions before reading it. |
| [`run.sh`](run.sh), [`run.ps1`](run.ps1) | Short entrypoints for the corresponding build helper's `--run` mode. |

These helpers deliberately require a non-sample API pair. For a source build that
configures its pair in the application, use the plain Cargo command in Development.
Bash sources trusted shell assignments; PowerShell reads simple `NAME=value`
assignments, with optional `export` and matching quotes, without executing them.
Use that simple format when sharing a local configuration between the helpers.
Bash runs in its own process; PowerShell restores the caller's environment and
working directory when it returns.

Only the following scripts support an operation preview:

```bash
scripts/build-local.sh --dry-run
scripts/run.sh --dry-run
scripts/package-macos.sh --dry-run
scripts/package-linux.sh --dry-run
python3 scripts/test-build-local.py --dry-run
```

```powershell
pwsh -NoProfile -File scripts/build-local.ps1 --dry-run
pwsh -NoProfile -File scripts/run.ps1 --dry-run
pwsh -NoProfile -File scripts/package-windows.ps1 --dry-run
```

Dry runs do not load private configuration, build, sign, install or publish.
Packaging dry runs still validate supplied version, label and architecture
arguments; the Linux helper requires an x86_64 host/target combination.

## Packaging entrypoints and supporting files

| File | Purpose |
| --- | --- |
| [`package-linux.sh`](package-linux.sh) | Create AppImage, portable archive, Debian package and checksums from a compiled executable. Stage that same input for cargo-deb, and stamp the requested version. Requires Linux packaging tools. |
| [`package-macos.sh`](package-macos.sh) | Assemble the universal app, signed standalone executable and `.pkg` installer. Requires both Mach-O architectures and the persistent private signing identity. CI adds the portable archive and checksums. |
| [`package-windows.ps1`](package-windows.ps1) | Assemble the standalone executable, portable ZIP, setup EXE and checksums. |
| [`build-windows-msi.ps1`](build-windows-msi.ps1), [`teleark.wxs`](teleark.wxs) | Build the internal transactional Windows Installer payload and define its files, upgrade policy and rollback behavior. The MSI is embedded in the setup EXE. |
| [`build-windows-exe.ps1`](build-windows-exe.ps1), [`teleark.iss`](teleark.iss) | Build the visible Inno Setup wizard around that MSI. |
| [`install-innosetup.ps1`](install-innosetup.ps1) | Download, checksum-verify, install and select the pinned Windows compiler. This installs a build tool on the host. |
| [`verify-innosetup-compiler.ps1`](verify-innosetup-compiler.ps1) | Shared compiler-version probe used by the bootstrap, EXE builder and regression test. |
| [`release-version.ps1`](release-version.ps1) | Resolve the Cargo version, enforce installer version limits and tag agreement, and produce the CI artifact label. |
| [`macos/Distribution.xml`](macos/Distribution.xml), [`macos/preinstall`](macos/preinstall) | Reject downgrades in Installer.app and command-line installations. The packager substitutes the release version. |
| [`macos/release-certificate.pem`](macos/release-certificate.pem) | Public certificate that pins the persistent release signing identity. It contains no private key. |
| [`macos/sign-app.sh`](macos/sign-app.sh) | Sign and verify the app and standalone executable through an isolated temporary Keychain; restore the prior Keychain search list afterward. |
| [`macos/create-signing-identity.sh`](macos/create-signing-identity.sh) | One-time creation of a new private identity outside the checkout. Existing releases reuse their identity; replacement requires the migration decision documented in Packaging. |

The Linux staging layout follows pinned cargo-deb 2.11.1's
[asset resolution](https://github.com/kornelski/cargo-deb/blob/v2.11.1/src/config.rs#L969):
`target/release/teleark` resolves under Cargo's configured target directory,
target triple and release profile. Its
[CLI](https://github.com/kornelski/cargo-deb/blob/v2.11.1/src/main.rs) accepts the
explicit manifest, target and Debian version used by the helper.

## Input and artifact checks

| Script | Purpose |
| --- | --- |
| [`check-line-endings.ps1`](check-line-endings.ps1) | Check tracked Git-index text for LF line endings and reject tracked `.cmd`/`.bat` files. Stage changes before using it as the final index check. |
| [`check-distribution-credentials.ps1`](check-distribution-credentials.ps1) | Validate protected release API inputs without logging their values. |
| [`check-macos-signing.sh`](check-macos-signing.sh) | Validate the private release PKCS#12 input against the public certificate, matching key and expiry before CI packaging starts. |
| [`check-release-checksums.ps1`](check-release-checksums.ps1) | Require a complete, unique expected artifact set and verify each SHA-256 digest. |
| [`verify-linux-payloads.sh`](verify-linux-payloads.sh) | Check required extracted files and notices, and compare the AppImage and archive executables. Debian may differ after independent stripping. |
| [`verify-macos-libraries.sh`](verify-macos-libraries.sh) | Inspect each universal Mach-O slice and reject dependencies outside Apple system libraries. |
| [`verify-release-linux.sh`](verify-release-linux.sh) | Verify Linux outputs and metadata, exercise the downgrade guard, install the Debian package and compare its installed executable. |
| [`verify-release-macos.sh`](verify-release-macos.sh) | Verify signatures, both architectures, payloads and metadata, then exercise a native install/upgrade and downgrade refusal. |
| [`verify-release-windows.ps1`](verify-release-windows.ps1) | Verify Windows outputs and metadata, then invoke native installer regressions. |

The release verifiers are for disposable native runners and use PowerShell 7
(`pwsh`) for the shared checksum check before unpacking or executing artifacts.
Linux and macOS checks
use administrator privileges and remove their test installation afterward;
they refuse an existing installed TeleArk. Windows supports an explicit
`-Isolated` mode with private installer identities; see Packaging for its scope.
They are not dry-run commands and do not publish releases.

## Regression tests

| Script | Scope and requirements |
| --- | --- |
| [`test-build-local.py`](test-build-local.py) | Temporary configuration, fake build/package tools, build/run failure propagation, supported dry runs, permission/path safety, and PowerShell environment restoration. Requires Python 3 and Bash; PowerShell cases run when `pwsh` or `powershell` is available. The Linux package-routing case requires distribution copyright notices and reports a skip elsewhere. |
| [`test-line-endings.ps1`](test-line-endings.ps1) | Synthetic Git repositories covering the tracked-file policy. Requires PowerShell and Git. |
| [`test-distribution-credentials.ps1`](test-distribution-credentials.ps1) | Synthetic valid, malformed, missing and public-sample API pairs. |
| [`test-release-checksums.ps1`](test-release-checksums.ps1) | Synthetic manifests covering complete, missing, duplicate, malformed and corrupted artifacts. |
| [`test-release-version.ps1`](test-release-version.ps1) | Synthetic Cargo metadata and CI references covering version limits, tag agreement and manual artifact labels. |
| [`test-linux-payloads.sh`](test-linux-payloads.sh) | Temporary byte fixtures covering portable equality, legitimate Debian differences and missing/corrupt files. No Linux installation required. |
| [`test-macos-libraries.sh`](test-macos-libraries.sh) | Fake `otool` output covering both architectures, external libraries and missing inspection. No macOS installation required. |
| [`test-macos-signing-import.sh`](test-macos-signing-import.sh) | Native macOS signing with a new synthetic identity; tests PKCS#12 import, forced PEM fallback, mismatched identities and search-list restoration. No release secret required. |
| [`test-macos-signing.sh`](test-macos-signing.sh) | Native signing continuity, standalone code equivalence and tamper rejection, using the identity supplied by its caller. Called with the synthetic identity by the import test and the release identity by packaging CI. |
| [`macos/verify-keychain-signing.sh`](macos/verify-keychain-signing.sh), [`macos/keychain-signing-probe.swift`](macos/keychain-signing-probe.swift) | Compile and sign two native probes, then check prompt-free access to an isolated synthetic Keychain across an update and denial after an identity change. Uses the caller-supplied signing identity. |
| [`test-install-innosetup.ps1`](test-install-innosetup.ps1) | Native Windows compiler probe plus synthetic download/bootstrap failures. Requires the pinned installed compiler. |
| [`test-windows-msi.ps1`](test-windows-msi.ps1) | Actual Windows Installer install, upgrade, downgrade and rollback transactions with temporary payloads; clean test account required. |
| [`test-windows-exe.ps1`](test-windows-exe.ps1) | Exercise the shipped setup wizard around Windows Installer; supports `-Isolated` for private test identities. |

Run the source-safe checks listed in Development for routine changes. Native
signing and installation tests belong on the corresponding operating system;
a fake-tool regression does not replace that qualification.
