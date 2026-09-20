# Building and packaging TeleArk

[Back to README](../README.md) · [Portable archives](#standalone-executable-and-portable-archives) · [macOS](#macos-app-dmg-and-pkg) · [Windows](#windows-installation-executable-and-portable-zip) · [Linux](#linux-debianubuntu-deb-and-portable-tarball)

## Prerequisites and release build

Run commands from the repository root. Install Rust through rustup; this checkout uses `rust-toolchain.toml` and the checked-in `Cargo.lock`. Build on the target operating system and CPU architecture. A macOS executable does not run on Windows or Linux, and adding a Rust target alone does not install native SDKs or linkers.

| Platform | Build prerequisites | Executable | Packaging status |
| --- | --- | --- | --- |
| macOS | Rust and Apple Command Line Tools (`xcode-select --install`) | `target/release/teleark` | `scripts/package-macos.sh` produces unsigned `.app` archive; manual `.dmg` / `.pkg` commands below |
| Windows | Native MSVC Rust toolchain, Visual Studio Build Tools with Desktop development with C++ and Windows SDK | `target/release/teleark.exe` | `scripts/package-windows.ps1` produces portable ZIP and Inno Setup installer (`.exe`) |
| Linux | Rust, C/C++ toolchain, CMake, pkg-config, development libraries required by GPUI (`libxkbcommon-dev`, `libwayland-dev`, etc.) | `target/release/teleark` | `scripts/package-linux.sh` produces portable tarball and Debian `.deb` package |

For platform dependency troubleshooting, check the native build requirements of the locked GPUI backend. macOS targets 11.0+ (Big Sur), Linux targets glibc 2.35+, and Windows targets Windows 10 (1809+).

Build without launching:

```bash
scripts/build-local.sh
# On Windows (PowerShell):
.\scripts\build-local.ps1
```

First configure your own application credentials in `.env.local` using [README environment setup](../README.md#load-env-values-before-building). The helper always loads that file and stops if it is missing, invalid or still uses the public sample API ID. `.env.example` is a template only and is never sourced as a fallback. Use bash on macOS/Linux or PowerShell / Git Bash with native Rust on Windows. All commands below run from the repository root.

The packaging scripts only copy the supplied binary; they cannot change credentials embedded by a previous build. Always rebuild with `.env.local` first. Neither local environment file belongs in the bundle. CI has no private `.env.local`; its current workflow builds without embedded credentials unless separately configured through protected build secrets.

## Standalone executable and portable archives

Build with the command above, loading credentials as described in the linked environment instructions when needed. Launch with `./target/release/teleark` on macOS/Linux or `.\target\release\teleark.exe` in Windows PowerShell. This is a desktop GUI executable, not a headless CLI.

### Windows Static Runtime Linking

To ensure `teleark.exe` runs on freshly installed Windows systems without requiring the Microsoft Visual C++ Redistributable (`VCRUNTIME140.dll`), compile with static C runtime (`/MT`):

```powershell
$env:RUSTFLAGS = "-C target-feature=+crt-static"
cargo build --release -p teleark-gui --bin teleark --locked
```

## macOS: `.app`, `.dmg` and `.pkg`

After building on macOS, create the application bundle with the repository script:

```bash
scripts/build-local.sh &&
  scripts/package-macos.sh target/release/teleark dist/TeleArk.app
```

This adds Info.plist, the app icon and license resources. Copy `dist/TeleArk.app` to Applications to install it. To produce a drag-to-Applications disk image, use a fresh staging directory and an unused output filename:

```bash
(
  set -eu
  mkdir -p dist/dmg
  ditto dist/TeleArk.app dist/dmg/TeleArk.app
  ln -s /Applications dist/dmg/Applications
  hdiutil create -volname TeleArk -srcfolder dist/dmg -ov -format UDZO dist/TeleArk.dmg
)
```

Alternatively, create an Installer package that installs the bundle under `/Applications`:

```bash
pkgbuild --component dist/TeleArk.app \
  --install-location /Applications \
  --identifier app.teleark.desktop \
  --version "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' dist/TeleArk.app/Contents/Info.plist)" \
  dist/TeleArk.pkg
```

These commands do not sign or notarize the output. Public distribution still needs signing/notarization and clean-machine validation.

## Windows: installation executable and portable ZIP

After building on Windows (ideally with `+crt-static`), package the application using the repository script:

```powershell
.\scripts\package-windows.ps1
```

This generates:
1. `dist/teleark-<version>-windows-x86_64.zip` (portable distribution with licenses).
2. `dist/TeleArk-Setup-<version>-windows-x86_64.exe` (Inno Setup non-admin installer using `scripts/teleark.iss`).
3. `dist/SHA256SUMS` (checksums).

If [Inno Setup](https://jrsoftware.org/isinfo.php) (`ISCC.exe`) is installed and available, the installer is built automatically with lowest privilege requirements (`{localappdata}\Programs\TeleArk`) and desktop/start menu shortcuts.

## Linux: Debian/Ubuntu `.deb` and portable tarball

After building on Linux (targeting Ubuntu 22.04 / glibc 2.35 baseline), package the application using the repository script:

```bash
scripts/package-linux.sh
```

This generates:
1. `dist/teleark-<version>-linux-<arch>.tar.gz` (portable distribution with desktop entry and icons).
2. `dist/teleark_<version>_amd64.deb` (Debian/Ubuntu package using `cargo-deb`).
3. `dist/SHA256SUMS` (checksums).

Install the `.deb` package on Debian/Ubuntu systems:
```bash
sudo apt install ./dist/teleark_<version>_amd64.deb
```
The package dependencies are automatically resolved by `dpkg`/`apt`, requiring only runtime libraries without build tools or `-dev` headers.


## Telegram session identity

Telegram associates the application name with the API ID registered in its application panel. The client sends TeleArk’s workspace version as `app_version`. Packaging with the public example ID identifies the session as Telegram Desktop. Saved credentials in Settings override the embedded pair; update an old override and establish a new login session when validating a changed application identity.
