# Building and packaging TeleArk

[Back to README](../README.md) · [Portable archives](#standalone-executable-and-portable-archives) · [macOS](#macos-app-dmg-and-pkg) · [Windows](#windows-installation-executable-manual-unverified) · [Linux](#linux-debianubuntu-deb-manual-unverified)

## Prerequisites and release build

Run commands from the repository root. Install Rust through rustup; this checkout uses `rust-toolchain.toml` and the checked-in `Cargo.lock`. Build on the target operating system and CPU architecture. A macOS executable does not run on Windows or Linux, and adding a Rust target alone does not install native SDKs or linkers.

| Platform | Build prerequisites | Executable | Packaging status |
| --- | --- | --- | --- |
| macOS | Rust and Apple Command Line Tools (`xcode-select --install`) | `target/release/teleark` | Repository script and CI produce an unsigned `.app` archive; manual `.dmg` / `.pkg` commands below |
| Windows | Native MSVC Rust toolchain, Visual Studio Build Tools with Desktop development with C++ and Windows SDK; Git Bash for the environment examples | `target/release/teleark.exe` | Manual ZIP / Inno Setup recipe; native build and installer not yet qualified by this project |
| Linux | Rust, C/C++ toolchain, CMake, pkg-config, Clang and development libraries required by the locked GPUI backend; a working graphical session/GPU driver | `target/release/teleark` | Manual tar archive / Debian package recipe; native build and installer not yet qualified by this project |

For platform dependency troubleshooting, check the native build requirements of the locked GPUI backend. The Windows and Linux prerequisites above are starting points, not a verified dependency list for this checkout. macOS is the current build/release baseline. There is no signed installer yet.

Build without launching:

```bash
scripts/build-local.sh
```

First configure your own application credentials in `.env.local` using [README environment setup](../README.md#load-env-values-before-building). The helper always loads that file and stops if it is missing, invalid or still uses the public sample API ID. `.env.example` is a template only and is never sourced as a fallback. Use bash on macOS/Linux or Git Bash with native Rust on Windows. All commands below run from the repository root.

The macOS packaging script only copies the supplied binary; it cannot change credentials embedded by a previous build. Always rebuild with `.env.local` first. Neither local environment file belongs in the bundle. CI has no private `.env.local`; its current workflow builds without embedded credentials unless separately configured through protected build secrets.

## Standalone executable and portable archives

Build with the command above, loading credentials as described in the linked environment instructions when needed. Launch with `./target/release/teleark` on macOS/Linux or `.\target\release\teleark.exe` in Windows PowerShell. This is a desktop GUI executable, not a headless CLI. A single executable may still require platform runtime libraries; test on a clean target machine before distributing it.

For a macOS/Linux archive containing the executable and required notices (bash/zsh):

```bash
(
  set -eu
  bundle="dist/teleark-$(uname -s)-$(uname -m)"
  mkdir -p "$bundle"
  cp target/release/teleark README.md LICENSE-MIT LICENSE-APACHE THIRD_PARTY_NOTICES.md "$bundle/"
  tar -C dist -czf "${bundle}.tar.gz" "$(basename "$bundle")"
)
```

For Windows, after the native build, use PowerShell:

```powershell
New-Item -ItemType Directory -Force dist/teleark-windows | Out-Null
Copy-Item target/release/teleark.exe, README.md, LICENSE-MIT, LICENSE-APACHE, THIRD_PARTY_NOTICES.md dist/teleark-windows/
Compress-Archive -Path dist/teleark-windows -DestinationPath dist/teleark-windows.zip -Force
```

Use a fresh staging directory for each release. These archives are unpack-and-run distributions; they do not register an application or create an uninstaller. Include any required redistributable runtime dependencies after checking their licenses.

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

Open the `.dmg` and drag the app into Applications, or open the `.pkg` and follow Installer. These commands do not sign or notarize the output. Public distribution still needs signing/notarization and clean-machine validation; see Apple's [distribution guidance](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution). Build separately for Apple Silicon and Intel; the commands do not create a universal binary. The existing [release workflow](../.github/workflows/release.yml) packages an unsigned `.app` in `.tar.gz` with a checksum, not a `.dmg` or `.pkg`.

## Windows: installation executable (manual, unverified)

First complete the native Windows build and portable folder above. Install [Inno Setup](https://jrsoftware.org/isinfo.php). Save the following as `dist/teleark.iss`, replacing `AppVersion` with the workspace version from `Cargo.toml`:

```ini
[Setup]
AppId=app.teleark.desktop
AppName=TeleArk
AppVersion=0.4.4
DefaultDirName={localappdata}\Programs\TeleArk
PrivilegesRequired=lowest
OutputDir=.
OutputBaseFilename=TeleArk-Setup
UninstallDisplayIcon={app}\teleark.exe

[Files]
Source: "teleark-windows\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{userprograms}\TeleArk"; Filename: "{app}\teleark.exe"
```

Compile using Inno Setup's IDE, or run `ISCC.exe dist\teleark.iss` from a shell where `ISCC.exe` is on PATH. The result is `dist/TeleArk-Setup.exe`, which installs for the current user and creates an uninstall entry. See the [compiler command-line documentation](https://jrsoftware.org/ishelp/topic_compilercmdline.htm). This recipe does not produce MSI/MSIX, bundle the MSVC runtime or sign the installer. Check native dependencies and test install, launch, upgrade and uninstall on Windows before publishing.

## Linux: Debian/Ubuntu `.deb` (manual, unverified)

First complete and test a native release build on the intended Debian/Ubuntu baseline. Install `dpkg-dev` and the packaging-only [cargo-deb tool](https://github.com/kornelski/cargo-deb):

```bash
sudo apt-get install dpkg-dev
cargo install cargo-deb --locked
```

The repository does not currently define Debian metadata. To try this packaging recipe, add the following table to `crates/teleark-gui/Cargo.toml` in your packaging checkout. Replace the maintainer placeholder with the distributor's actual name/email:

```toml
[package.metadata.deb]
name = "teleark"
maintainer = "Your Name <you@example.com>"
depends = "$auto"
section = "utils"
priority = "optional"
assets = [
    ["target/release/teleark", "usr/bin/teleark", "755"],
    ["../../README.md", "usr/share/doc/teleark/README.md", "644"],
    ["../../LICENSE-MIT", "usr/share/doc/teleark/LICENSE-MIT", "644"],
    ["../../LICENSE-APACHE", "usr/share/doc/teleark/LICENSE-APACHE", "644"],
    ["../../THIRD_PARTY_NOTICES.md", "usr/share/doc/teleark/THIRD_PARTY_NOTICES.md", "644"],
]
```

Then package the already-built executable without rebuilding outside its credential environment:

```bash
mkdir -p dist
cargo deb -p teleark-gui --no-build --output dist/teleark.deb
dpkg-deb --info dist/teleark.deb
dpkg-deb --contents dist/teleark.deb
sudo apt install ./dist/teleark.deb
```

Run `teleark` from a terminal in the desktop session; remove it with `sudo apt remove teleark`. `$auto` resolves linked library dependencies, but cannot prove dynamically loaded graphics/runtime dependencies are complete. Inspect the package and validate install/launch/upgrade/removal on a clean target distribution. This minimal recipe has no application-menu entry. RPM, AppImage and Flatpak packaging are not configured; do not rename a `.deb` or tar archive to those formats.


## Telegram session identity

Telegram associates the application name with the API ID registered in its application panel. The client sends TeleArk’s workspace version as `app_version`. Packaging with the public example ID identifies the session as Telegram Desktop. Saved credentials in Settings override the embedded pair; update an old override and establish a new login session when validating a changed application identity.
