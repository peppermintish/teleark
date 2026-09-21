# Desktop release packages

The [release workflow](../.github/workflows/release.yml) builds three explicit native targets and publishes nine files for a matching `vX.Y.Z` tag. A manual run uploads the same files as workflow artifacts with a `-manual-<commit>` filename suffix. The installer version always comes from the `teleark-gui` Cargo package (`X.Y.Z`); branch names and the `v` tag prefix never become installer versions. A tag whose version differs from Cargo fails before packaging.

| Target | Standalone executable | Portable archive | Native installer |
| --- | --- | --- | --- |
| Windows x86_64 | `teleark-<label>-windows-x86_64.exe` | `teleark-<label>-windows-x86_64.zip` | `TeleArk-Setup-<label>-windows-x86_64.exe` |
| macOS arm64 | `teleark-<label>-macos-arm64.bin` (Mach-O executable) | `teleark-<label>-macos-arm64.tar.gz` (`TeleArk.app`) | `TeleArk-<label>-macos-arm64.pkg` |
| Linux x86_64 | `teleark-<label>-linux-x86_64` | `teleark-<label>-linux-x86_64.tar.gz` | `teleark_<label>_amd64.deb` |

Each platform job checks that all three files exist, verifies the archive and native package metadata, and uploads checksums. Tagged publication checks the exact nine-file manifest before creating a release. The workflow fails if an installer cannot be built or an existing release would be overwritten. Published assets also include both licenses, third-party notices and a unified `SHA256SUMS` file.

Native installers upgrade in place. Windows Inno Setup keeps the existing per-user installation directory and refuses an older setup before copying files; it shows an error dialog, or logs the same reason in silent mode. The macOS package installs at `/Applications/TeleArk.app`; Installer.app shows a fatal version message and the component preinstall script also refuses downgrades from the command line. Debian's package manager replaces the installed `teleark` package in place, and the new package's `preinst` rejects a version older than the installed package before unpacking. Unknown installed versions on Windows and macOS also stop the installer. The portable archives and standalone executables do not enforce a version guard when copied manually.

## macOS

Build on an arm64 Mac and package with:

```bash
scripts/build-local.sh && scripts/package-macos.sh
```

This creates `dist/TeleArk.app`, the raw executable and the `.pkg`. `pkgbuild`, `productbuild` and other Apple packaging tools are required. The script stamps both bundle version fields from Cargo and rejects a binary that lacks the selected architecture. To install, open the `.pkg` in Installer.app, or run `sudo installer -pkg dist/TeleArk-<label>-macos-arm64.pkg -target /`.

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

The workflow does not publish a `.dmg`. The `.app` and `.pkg` are unsigned and not notarized; public distribution needs signing, notarization and clean-machine checks.

## Windows

Build the x86_64 MSVC target with `+crt-static`, then run:

```powershell
.\scripts\package-windows.ps1
```

[Inno Setup 6](https://jrsoftware.org/isdl.php) (`ISCC.exe`) is required. The script creates the standalone executable, portable ZIP, installer and checksums; missing Inno Setup is an error. The installer defaults to `%LOCALAPPDATA%\Programs\TeleArk` and retains an existing TeleArk install path. Run the setup executable to install or upgrade.

## Linux

Build on the Ubuntu 22.04 x86_64 baseline and run:

```bash
scripts/package-linux.sh
```

`cargo-deb` is required. The script creates the standalone ELF executable, portable tarball, `.deb` and checksums. Install or upgrade with:

```bash
sudo apt install ./dist/teleark_<label>_amd64.deb
```

The `.deb` contains application shortcuts, icons, licenses and notices. Its `preinst` compares Debian package versions using `dpkg`, including distro revision suffixes.
