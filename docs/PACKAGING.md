## macOS: `.app`, `.dmg` and `.pkg`

After building on macOS, create the application bundle and `.pkg` installer with the repository script:

```bash
scripts/build-local.sh &&
  scripts/package-macos.sh
```

This script:
1. Assembles `dist/TeleArk.app` with icons, `Info.plist`, and licenses.
2. If `pkgbuild` is installed, creates `dist/TeleArk-<version>-macos-<arch>.pkg` targeting `/Applications/TeleArk.app` with an embedded preinstall script that performs an in-place upgrade and strictly aborts if a newer version is already installed.

To produce a drag-to-Applications disk image (`.dmg`), use a fresh staging directory and an unused output filename:

```bash
(
  set -eu
  mkdir -p dist/dmg
  ditto dist/TeleArk.app dist/dmg/TeleArk.app
  ln -s /Applications dist/dmg/Applications
  hdiutil create -volname TeleArk -srcfolder dist/dmg -ov -format UDZO dist/TeleArk.dmg
)
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

If [Inno Setup](https://jrsoftware.org/isinfo.php) (`ISCC.exe`) is installed and available, the installer is built automatically with lowest privilege requirements (`{localappdata}\Programs\TeleArk`), in-place upgrade by default (`UsePreviousAppDir=yes`), and a pre-install guard that blocks and aborts downgrades if a newer version is already detected.

## Linux: Debian/Ubuntu `.deb` and portable tarball

After building on Linux (targeting Ubuntu 22.04 / glibc 2.35 baseline), package the application using the repository script:

```bash
scripts/package-linux.sh
```

This generates:
1. `dist/teleark-<version>-linux-<arch>.tar.gz` (portable distribution with desktop entry and icons).
2. `dist/teleark_<version>_amd64.deb` (Debian/Ubuntu package using `cargo-deb`).
3. `dist/SHA256SUMS` (checksums).

The `.deb` package installs via:
```bash
sudo apt install ./dist/teleark_<version>_amd64.deb
```
The package automatically performs in-place upgrades via `dpkg`, and contains a `preinst` script that aborts the install if the system already has a newer package version installed.
