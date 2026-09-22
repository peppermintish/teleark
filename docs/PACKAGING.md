# Desktop release packages

Packaging turns a compiled TeleArk executable into a portable app and a native installer, adding icons, version information and license files. macOS also signs the app with the persistent release identity. Local outputs go into the repository's `dist/` directory by default; CI verifies its outputs and attaches the finished files to the matching GitHub Release.

The [single CI/CD workflow](../.github/workflows/ci.yml) runs source checks for pull requests, version tags and manual dispatches. Branch pushes alone do not start a run, so a combined branch and version-tag push starts only the release run. Installer jobs run only for a matching `vX.Y.Z` tag or an explicitly selected manual package preview. A preview uploads artifacts but cannot publish a GitHub Release. A tag must match the `teleark-gui` Cargo version; the numeric Cargo version is stamped into every native installer.

The tagged workflow checks and publishes these nine files, plus both project licenses, third-party notices and a unified `SHA256SUMS`:

| Target | Standalone executable | Portable archive | Native installer |
| --- | --- | --- | --- |
| Windows x64 | `teleark-<label>-windows-x86_64.exe` | `teleark-<label>-windows-x86_64.zip` | `TeleArk-Setup-<label>-windows-x86_64.exe` |
| macOS universal | `teleark-<label>-macos-universal.bin` | `teleark-<label>-macos-universal.tar.gz` containing `TeleArk.app` | `TeleArk-<label>-macos-universal.pkg` |
| Linux x64 | `teleark-<label>-linux-x86_64.AppImage` | `teleark-<label>-linux-x86_64.tar.gz` containing `AppRun` | `teleark_<label>_amd64.deb` |

Windows binaries statically link the Microsoft C runtime. The macOS universal executable contains both `arm64` and `x86_64` slices and its package check rejects references to non-system dynamic libraries. Linux portable outputs bundle linked runtime libraries in an AppDir, along with available distribution copyright notices. The Debian installer declares runtime package dependencies and `apt` resolves them automatically. Users do not need Rust, a compiler, an SDK or a separate language runtime. The host still supplies its operating system, graphics drivers and desktop facilities.

Native installers upgrade in place. The Windows EXE installer retains the existing per-user directory and shows an error before changing files if the installer is older; silent installation logs the same reason. The macOS package installs at `/Applications/TeleArk.app`; Installer.app and its command-line pre-install guard reject downgrades. Debian `preinst` rejects a version older than the installed package before unpacking. Copying a portable archive or standalone executable manually does not enforce a version guard.

Each package job checks native architecture, files, checksums and installer metadata. Windows and macOS jobs exercise a real install and in-place upgrade, then prove an older installer fails without replacing the executable. Linux jobs extract both portable formats, check the Debian guard and install the package. Publication refuses to overwrite an existing release or accept a missing asset. The GitHub Actions summary shows the stages, LF counts, cache hits and SHA256 manifest.

## Windows

Build the `x86_64-pc-windows-msvc` target with `RUSTFLAGS="-C target-feature=+crt-static"`. Install the pinned Inno Setup 6.7.3 compiler and the .NET 8 SDK on the build machine, run `dotnet tool restore` for WiX 5.0.2, then run `scripts/package-windows.ps1` with the `x86_64` architecture argument. CI installs and verifies the pinned compiler with `scripts/install-innosetup.ps1`; local Windows builds can use the same helper or the official Inno Setup 6.7.3 installer. The build helper rejects a different `ISCC.exe` version. Release assets contain the standalone app EXE, portable ZIP and **TeleArk-Setup EXE**. Double-click the Setup EXE for Welcome, destination, confirmation, installation progress and completion screens, with an optional launch action. The wizard uses English, as the previous EXE installer did; the application supports ten locales with English fallback.

The EXE embeds the transactional MSI as a private payload. Users do not open or download an MSI separately, and no .NET, WiX, Inno Setup or Visual C++ redistributable is needed on their computer. The internal MSI stays under `dist/.windows-installer/`, outside the portable ZIP and published asset set. Inno creates no second uninstall entry and does not own application files. Windows Installer retains ownership so existing MSI installations upgrade normally, rollback remains transactional, and Settings offers one installed product. [ADR 0052](adr/0052-visible-windows-exe-installer.md) records this choice.

Installation defaults to `%LOCALAPPDATA%\Programs\TeleArk` without elevation. Upgrades display and retain the registered location; only a fresh installation can select another location. Re-running the same EXE repairs missing files. Older versions are refused before changing installed files. Existing per-user Inno installations retain the automatic migration described in ADR 0047. User data and unrelated files are retained. Numeric versions still fit the embedded MSI limits (`255.255.65535`). Windows 10 version 1809 or later is required; Windows 11 ARM can use x64 emulation.

For unattended installation use `TeleArk-Setup-<label>-windows-x86_64.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /LOG="install.log"`. Add `/DIR="C:\chosen\path"` for a fresh installation. Normal launches always show the wizard and native progress/cancellation UI; only explicit unattended flags suppress it. Cancellation and failures stop before the completion/launch page and return a nonzero EXE exit code. Both the wizard log and adjacent `install.log.msi.log` record the outcome. The MSI log contains the detailed refusal or rollback reason. Restart-required success is handed back to the wizard without automatically restarting Windows.

The release verifier checks EXE metadata, archive contents and all three checksums, then runs engine and EXE installation/repair/upgrade/downgrade/rollback tests. Native tests use temporary files and refuse a real installed product. Local artifact and EXE verification can use `scripts/verify-release-windows.ps1 0.4.13 0.4.13 x86_64 -Isolated`. This skips the clean-account-only legacy MSI suite. Direct EXE testing may use `scripts/test-windows-exe.ps1 -InstallerPath <setup.exe> -MsiPath <internal.msi> -Isolated`: it assigns private product, upgrade, component, registry and shortcut identities, validates every upgrade lookup before execution, and retains logs. CI tests the actual release identity on a clean account. These checks do not require Telegram credentials or user data.

## Telegram distribution configuration

The workflow validates the repository secrets `TELEARK_DISTRIBUTION_TELEGRAM_API_ID` and `TELEARK_DISTRIBUTION_TELEGRAM_API_HASH` once before starting the packaging matrix. It then maps both secrets into the environment of each native compilation step, including both slices of universal macOS. Rust embeds them through the runtime's existing `option_env!` constants, so standalone executables and installed apps use the same configured pair. Missing or malformed secrets stop packaging; summaries report only validation status, never values. These identifiers are extractable from binaries and do not authorize a Telegram user. Saved personal credentials continue to override the embedded pair.

## macOS

The release job builds `aarch64-apple-darwin` and `x86_64-apple-darwin` on an Apple Silicon runner with `MACOSX_DEPLOYMENT_TARGET=11.0`, joins them with `lipo`, and creates the app archive, raw executable and product package using `scripts/package-macos.sh`. macOS 11 is the declared minimum. The raw executable uses Apple system frameworks only; the `.app` includes the icon and legal files. Install through Finder or `sudo installer -pkg TeleArk-<label>-macos-universal.pkg -target /`.

The `.app` and standalone executable are signed with the pinned self-signed certificate in `scripts/macos/release-certificate.pem`. `scripts/macos/sign-app.sh` supplies the same explicit designated requirement (bundle identifier plus certificate fingerprint) to every release, so Keychain can recognize an updated executable. The installer remains unsigned; this identity does not provide Developer ID, notarization, or Gatekeeper trust. macOS can still refuse access to locked Keychains or legacy items created by a different identity. Runtime requests fail without opening authentication popups and preserve recovery material.

### Build and package locally

Run from the repository root on macOS with Rust, Apple Command Line Tools and OpenSSL 3 available. If OpenSSL 3 was installed with Homebrew, add it to your shell with `export PATH="$(brew --prefix openssl@3)/bin:$PATH"`. Prepare your private `.env.local` as described in [Development](DEVELOPMENT.md#local-development-environment), and replace `/private/path/to/identity.p12` below with the existing release signing identity. Keep that file outside the repository and reuse it for subsequent builds.

The packager requires a universal executable containing both Apple Silicon and Intel code. Build both targets through the local helper, combine them, then package:

```bash
(
  set +x
  set -e
  export TELEARK_MACOS_SIGNING_P12="/private/path/to/identity.p12"
  unset TELEARK_MACOS_SIGNING_P12_BASE64
  export MACOSX_DEPLOYMENT_TARGET=11.0

  rustup target add aarch64-apple-darwin x86_64-apple-darwin
  CARGO_BUILD_TARGET=aarch64-apple-darwin scripts/build-local.sh
  CARGO_BUILD_TARGET=x86_64-apple-darwin scripts/build-local.sh
  mkdir -p target/universal-apple-darwin/release
  lipo -create \
    target/aarch64-apple-darwin/release/teleark \
    target/x86_64-apple-darwin/release/teleark \
    -output target/universal-apple-darwin/release/teleark
  scripts/package-macos.sh \
    target/universal-apple-darwin/release/teleark dist/TeleArk.app
)
```

Each build helper loads and validates `.env.local` privately. The packaging script assembles the app, adds its resources, signs the app and standalone executable using a temporary Keychain, and creates the installer. It reads the version from `Cargo.toml`; no installation or upload occurs during these commands. The example assumes Cargo's default `target/` output directory.

The finished files are:

| Local output | Purpose |
| --- | --- |
| `dist/TeleArk.app` | Signed app bundle; open it directly or copy it to Applications. |
| `dist/TeleArk-<version>-macos-universal.pkg` | Installer to share; installs the app at `/Applications/TeleArk.app`. |
| `dist/teleark-<version>-macos-universal.bin` | Signed standalone executable. |

For version 0.5.2, the installer is `dist/TeleArk-0.5.2-macos-universal.pkg`. CI additionally creates the portable `.tar.gz` archive and release checksums. To preview the local output paths without building or accessing credentials, run `scripts/package-macos.sh --dry-run target/universal-apple-darwin/release/teleark dist/TeleArk.app`.

### Persistent self-signed release identity

The original identity was generated once using `scripts/macos/create-signing-identity.sh` into a private directory outside the checkout. Never regenerate it per build. The public certificate is committed; `private-key.pem` and `identity.p12` are private, mode 0600 inside a 0700 directory, and must be backed up securely. The certificate is valid for twenty years. Rotation requires an explicit compatible identity/recovery migration; simply replacing the certificate breaks Keychain continuity.

Set GitHub Actions repository secret `TELEARK_MACOS_SIGNING_P12_BASE64` from the base64-encoded private `identity.p12`. Feed it through standard input or a private file, never command arguments or logs. Before starting the packaging matrix, `scripts/check-macos-signing.sh` verifies that the secret matches the pinned public certificate, contains its corresponding private key, and has at least thirty days of certificate validity. The macOS package step also requires and verifies this identity. Local packaging uses `TELEARK_MACOS_SIGNING_P12` pointing to the private file. OpenSSL 3 and Apple command-line tools are required. The PKCS#12 envelope has an empty passphrase because the private directory and GitHub encrypted secret store protect it; no passphrase or key is passed in process arguments.

Signing imports the PKCS#12 identity into an isolated temporary Keychain as one signing identity, verifies the pinned public fingerprint, grants only Apple signing tools access, signs both distribution forms, and removes temporary material on exit. If a runner rejects the PKCS#12 envelope, the helper uses explicitly typed PEM imports and repeats the identity check. It never changes the system certificate trust store. Bundle and standalone resource signatures differ; the release verifier compares executable code after removing signatures from temporary copies. `scripts/test-macos-signing.sh` verifies identity continuity across an update, code equivalence, and rejection of altered resources using synthetic files. CI runs it and `scripts/macos/verify-keychain-signing.sh` before packaging. The latter uses only a temporary Keychain and synthetic data to confirm prompt-free access across a signed update and denial for a changed signing identity. See [ADR 0055](adr/0055-stable-macos-signing-identity.md).

## Linux

Linux packages are built natively on an Ubuntu 22.04 x64 runner. `scripts/package-linux.sh` uses `cargo-deb` and a checksum-pinned MIT-licensed `linuxdeploy` build to create the Debian package and portable AppDir outputs. The AppImage is a one-file runnable application; the archive exposes `AppRun` for systems without AppImage FUSE support. To install or upgrade the Debian package, use `sudo apt install ./teleark_<label>_amd64.deb`; `apt` installs ordinary runtime libraries if the host lacks them. Neither method asks users to install development headers or SDKs.

Ubuntu 22.04 is the build baseline, so older distributions are not promised. Portable outputs include linked libraries but still use the host kernel, display server and graphics drivers. The Debian package installs desktop shortcuts, icons, licenses and notices; its `preinst` compares Debian version strings including distribution revisions.

Linux verification compares the executables extracted from AppImage and the portable archive, which share the same final AppDir. It checks the Debian executable independently and compares the installed file with the Debian payload. The portable executable's RPATH and stripping differ from the Debian executable, so they are not expected to be byte-identical. An early payload regression covers this distinction, corrupt portable output and missing required files before release compilation.
