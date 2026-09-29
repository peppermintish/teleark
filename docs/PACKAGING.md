# Desktop release packages

Packaging builds native releases for Windows, macOS and Linux. The Windows job verifies the exact MSIX and its SHA-256 checksum after packaging and again after artifact download, before Store submission. Release assembly verifies the exact platform artifact groups and every downloaded checksum manifest before creating the GitHub Release set. Local artifacts go in `dist/`.

## Release assets

A tagged release publishes seven platform artifacts, both project licenses, third-party notices and a unified `SHA256SUMS`.

The seven platform artifacts are one Windows MSIX, three universal macOS packages and three Linux packages. Each platform artifact group includes its checksum manifest; final assembly rejects missing, extra, duplicate or mismatched entries, then emits checksums for the seven packages and all three legal notices.

| Target | Release artifacts |
| --- | --- |
| Windows x64 | `TeleArk-<version>-windows-x86_64.msix` |
| macOS universal | `teleark-<version>-macos-universal.bin`, `teleark-<version>-macos-universal.tar.gz`, `TeleArk-<version>-macos-universal.pkg` |
| Linux x64 | `teleark-<version>-linux-x86_64.AppImage`, `teleark-<version>-linux-x86_64.tar.gz`, `teleark_<version>_amd64.deb` |

The Windows Store package is unsigned before submission; Microsoft signs it after certification. It uses a separate data set under `%USERPROFILE%\TeleArk` and does not import data from earlier unpackaged Windows builds. macOS and Linux package details are below.

## Windows

Windows release builds produce one artifact: an x64 MSIX for Microsoft Store distribution. The build uses the `msix` feature and statically links the Microsoft C runtime. Its manifest declares the nine UI languages bundled under `crates/teleark-i18n/resources` (`en-US`, `de-DE`, `es-ES`, `fr-FR`, `ja-JP`, `ko-KR`, `pt-BR`, `ru-RU`, and `zh-CN`); keep the manifest list in sync with those catalogs. The manifest identity comes from the three repository variables `TELEARK_MSIX_IDENTITY_NAME`, `TELEARK_MSIX_PUBLISHER`, and `TELEARK_MSIX_PUBLISHER_DISPLAY_NAME`; keep them identical to the values assigned in Partner Center.

On every matching `vX.Y.Z` tag, CI builds and checks all platform packages, submits the verified Windows `.msix` to Partner Center, then publishes the GitHub Release. Pull requests and manual package previews do not submit to the Store. A release tag must point to a commit on `main` and match the `teleark-gui` Cargo version. Package identity, Telegram distribution and macOS signing configuration are checked in preflight before package work begins. Store tenant/client/secret and Product ID values are checked inside the protected `store-submission` GitHub Environment after the verified Windows package is ready; those credentials are not exposed to preflight or package jobs. Missing Store credentials fail at the Store gate after the package jobs have run.

The workflow uses Microsoft's legacy [Store submission API](https://learn.microsoft.com/en-us/windows/uwp/monetize/create-and-manage-submissions-using-windows-store-services) for MSIX. It creates a draft, updates its full writable record, places the raw `.msix` inside a ZIP for the API's Azure Blob upload, commits the draft and polls the API status. The Store configuration check runs in the protected `store-submission` job before any API request and reports missing settings without printing values. The client secret is sent only in the HTTPS OAuth form body and is cleared from the process environment after token acquisition. Restrict the `store-submission` GitHub Environment to version-tag releases. Microsoft's API requires at least one completed submission and a configured age rating; upload the first package manually through Partner Center. Microsoft still reviews and certifies each update before publication.

| GitHub setting | Store value |
| --- | --- |
| Secret `AZURE_AD_TENANT_ID` | Microsoft Entra tenant ID associated with Partner Center |
| Secret `AZURE_AD_APPLICATION_CLIENT_ID` | Client ID of the Entra application |
| Secret `AZURE_AD_APPLICATION_SECRET` | Client secret of that Entra application |
| Variable `TELEARK_MSSTORE_PRODUCT_ID` | TeleArk Store Product ID |

The legacy MSIX API does not document a Seller ID input. An existing GitHub `SELLER_ID` secret may remain configured, but this workflow does not require or read it; only the tenant, client ID, client secret and Store Product ID are used for submission. Associate the Entra app registration with Partner Center and assign it the Manager role. The API's [submission workflow](https://learn.microsoft.com/en-us/windows/uwp/monetize/manage-app-submissions) and [update schema](https://learn.microsoft.com/en-us/windows/uwp/monetize/update-an-app-submission) describe the draft, package upload, commit and polling contract. The package manifest declares `runFullTrust` so it can launch the native desktop process; Partner Center may request a justification during initial submission. Explain that TeleArk is a native Rust desktop app that uses a local database, encryption, Telegram networking and user-selected files. See Microsoft's [desktop packaging guidance](https://learn.microsoft.com/en-us/windows/msix/desktop/desktop-to-uwp-manual-conversion) and [restricted capability guidance](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/app-capability-declarations).

The manifest targets Windows 10 version 1809 or later. General Windows 10 support ended on October 14, 2025, with later support limited to specific LTSC or Extended Security Update editions; confirm that the declared floor fits the intended audience before submission. See [MSIX platform support](https://learn.microsoft.com/en-us/windows/msix/supported-platforms) and the [Windows 10 lifecycle notice](https://learn.microsoft.com/en-us/lifecycle/announcements/windows-10-end-of-support).

The package version maps Cargo `major.minor.patch` to `(major + 1).minor.patch.0`; for example, Cargo `0.5.11` becomes Store package version `1.5.11.0`. Partner Center identity values must remain stable across updates. The package stores the database, session and default managed files under `%USERPROFILE%\TeleArk`; this location persists after uninstall and follows the user's Windows profile permissions.

### Build the first package locally

On Windows, prepare `.env.local` with your private Telegram distribution application ID and hash as described in [Development](DEVELOPMENT.md#local-development-environment). The build helper reads those values without displaying them. Pass the exact, non-secret Partner Center identity from the GitHub repository variables:

```powershell
pwsh -NoProfile -File .\scripts\build-local-msix.ps1 `
  -IdentityName "<TELEARK_MSIX_IDENTITY_NAME>" `
  -Publisher "<TELEARK_MSIX_PUBLISHER>" `
  -PublisherDisplayName "<TELEARK_MSIX_PUBLISHER_DISPLAY_NAME>"
```

The script builds the x64 `msix` feature binary, creates the unsigned Store package and records its SHA256 checksum. It writes `dist\TeleArk-<version>-windows-x86_64.msix`. Use `-DryRun` to validate inputs and output paths without compiling; `-Force` replaces an existing package with the same version. Upload the resulting `.msix` to Partner Center for the initial submission. Microsoft supports raw `.msix` packages in the [Store package requirements](https://learn.microsoft.com/en-us/windows/apps/publish/publish-your-app/msix/app-package-requirements) and the [Store CLI](https://learn.microsoft.com/en-us/windows/apps/publish/msstore-dev-cli/commands).

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

For version 0.5.11, the installer is `dist/TeleArk-0.5.11-macos-universal.pkg`. CI additionally creates the portable `.tar.gz` archive and release checksums. To preview the local output paths without building or accessing credentials, run `scripts/package-macos.sh --dry-run target/universal-apple-darwin/release/teleark dist/TeleArk.app`.

### Persistent self-signed release identity

The original identity was generated once using `scripts/macos/create-signing-identity.sh` into a private directory outside the checkout. Never regenerate it per build. The public certificate is committed; `private-key.pem` and `identity.p12` are private, mode 0600 inside a 0700 directory, and must be backed up securely. The certificate is valid for twenty years. Rotation requires an explicit compatible identity/recovery migration; simply replacing the certificate breaks Keychain continuity.

Set GitHub Actions repository secret `TELEARK_MACOS_SIGNING_P12_BASE64` from the base64-encoded private `identity.p12`. Feed it through standard input or a private file, never command arguments or logs. Before starting the packaging matrix, `scripts/check-macos-signing.sh` verifies that the secret matches the pinned public certificate, contains its corresponding private key, and has at least thirty days of certificate validity. The macOS package step also requires and verifies this identity. Local packaging uses `TELEARK_MACOS_SIGNING_P12` pointing to the private file. OpenSSL 3 and Apple command-line tools are required. The PKCS#12 envelope has an empty passphrase because the private directory and GitHub encrypted secret store protect it; no passphrase or key is passed in process arguments.

Signing imports the PKCS#12 identity into an isolated temporary Keychain as one signing identity, checks the pinned fingerprint against all certificate/private-key identities (including the intentionally untrusted self-signed identity), grants only Apple signing tools access, and temporarily adds that Keychain to the user search list so `codesign` can resolve its private key on macOS 15. The helper restores the original search list and removes temporary material on exit. If a runner rejects the PKCS#12 envelope, the helper reports its exit status, extracts explicitly typed PEM files and uses Apple's `certtool` to import the certificate and private key together. A mismatched identity fails before signing. It never changes the system certificate trust store. Bundle and standalone resource signatures differ; the release verifier compares executable code after removing signatures from temporary copies. `scripts/test-macos-signing.sh` verifies identity continuity across an update, code equivalence, and rejection of altered resources using synthetic files. CI runs it and `scripts/macos/verify-keychain-signing.sh` before packaging. The latter uses only a temporary Keychain and synthetic data to confirm prompt-free access across a signed update and denial for a changed signing identity. `scripts/test-macos-signing-import.sh` additionally exercises signing, Keychain continuity, forced PEM fallback, identity mismatch and search-list restoration with a fresh untrusted synthetic certificate; it requires no release secret. See [ADR 0055](adr/0055-stable-macos-signing-identity.md).

## Linux

Linux packages are built natively on an Ubuntu 22.04 x64 runner. `scripts/package-linux.sh` uses `cargo-deb` and a checksum-pinned MIT-licensed `linuxdeploy` build to create the Debian package and portable AppDir outputs. The AppImage is a one-file runnable application; the archive exposes `AppRun` for systems without AppImage FUSE support. To install or upgrade the Debian package, use `sudo apt install ./teleark_<label>_amd64.deb`; `apt` installs ordinary runtime libraries if the host lacks them. Neither method asks users to install development headers or SDKs.

Ubuntu 22.04 is the build baseline, so older distributions are not promised. Portable outputs include linked libraries but still use the host kernel, display server and graphics drivers. The Debian package installs desktop shortcuts, icons, licenses and notices; its `preinst` compares Debian version strings including distribution revisions.

Linux verification compares the executables extracted from AppImage and the portable archive, which share the same final AppDir. It checks the Debian executable independently and compares the installed file with the Debian payload. The portable executable's RPATH and stripping differ from the Debian executable, so they are not expected to be byte-identical. An early payload regression covers this distinction, corrupt portable output and missing required files before release compilation.
