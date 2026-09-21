# Desktop release packages

The [single CI/CD workflow](../.github/workflows/ci.yml) runs source checks for every branch, pull request and tag. Installer jobs run only for a matching `vX.Y.Z` tag or an explicitly selected manual package preview. A preview uploads artifacts but cannot publish a GitHub Release. A tag must match the `teleark-gui` Cargo version; the numeric Cargo version is stamped into every native installer.

The tagged workflow checks and publishes these nine files, plus both project licenses, third-party notices and a unified `SHA256SUMS`:

| Target | Standalone executable | Portable archive | Native installer |
| --- | --- | --- | --- |
| Windows x64 | `teleark-<label>-windows-x86_64.exe` | `teleark-<label>-windows-x86_64.zip` | `TeleArk-Setup-<label>-windows-x86_64.exe` |
| macOS universal | `teleark-<label>-macos-universal.bin` | `teleark-<label>-macos-universal.tar.gz` containing `TeleArk.app` | `TeleArk-<label>-macos-universal.pkg` |
| Linux x64 | `teleark-<label>-linux-x86_64.AppImage` | `teleark-<label>-linux-x86_64.tar.gz` containing `AppRun` | `teleark_<label>_amd64.deb` |

Windows binaries statically link the Microsoft C runtime. The macOS universal executable contains both `arm64` and `x86_64` slices and its package check rejects references to non-system dynamic libraries. Linux portable outputs bundle linked runtime libraries in an AppDir, along with available distribution copyright notices. The Debian installer declares runtime package dependencies and `apt` resolves them automatically. Users do not need Rust, a compiler, an SDK or a separate language runtime. The host still supplies its operating system, graphics drivers and desktop facilities.

Native installers upgrade in place. Windows Inno Setup retains the existing per-user directory and shows an error before changing files if the installer is older; silent installation logs the same reason. The macOS package installs at `/Applications/TeleArk.app`; Installer.app and its command-line pre-install guard reject downgrades. Debian `preinst` rejects a version older than the installed package before unpacking. Copying a portable archive or standalone executable manually does not enforce a version guard.

Each package job checks native architecture, files, checksums and installer metadata. Windows and macOS jobs exercise a real install and in-place upgrade, then prove an older installer fails without replacing the executable. Linux jobs extract both portable formats, check the Debian guard and install the package. Publication refuses to overwrite an existing release or accept a missing asset. The GitHub Actions summary shows the stages, LF counts, cache hits and SHA256 manifest.

## Windows

Build the `x86_64-pc-windows-msvc` target with `RUSTFLAGS="-C target-feature=+crt-static"`, then run `scripts/package-windows.ps1` with the `x86_64` architecture argument. The Inno Setup 6 compiler is a build-time tool; the installer includes the app and legal files. The installer defaults to `%LOCALAPPDATA%\Programs\TeleArk`, preserves an existing install location and accepts a same-version repair. The x64 build targets Windows 10 version 1809 or later and can run under Windows 11 ARM x64 emulation. Inno Setup has separate [commercial-use license terms](https://jrsoftware.org/isorder.php) for distributors.

## macOS

The release job builds `aarch64-apple-darwin` and `x86_64-apple-darwin` on an Apple Silicon runner with `MACOSX_DEPLOYMENT_TARGET=11.0`, joins them with `lipo`, and creates the app archive, raw executable and product package using `scripts/package-macos.sh`. macOS 11 is the declared minimum. The raw executable uses Apple system frameworks only; the `.app` includes the icon and legal files. Install through Finder or `sudo installer -pkg TeleArk-<label>-macos-universal.pkg -target /`.

The `.app` and `.pkg` are currently unsigned and unnotarized. Public distribution through Gatekeeper needs Apple signing, notarization and clean-machine checks.

## Linux

Linux packages are built natively on an Ubuntu 22.04 x64 runner. `scripts/package-linux.sh` uses `cargo-deb` and a checksum-pinned MIT-licensed `linuxdeploy` build to create the Debian package and portable AppDir outputs. The AppImage is a one-file runnable application; the archive exposes `AppRun` for systems without AppImage FUSE support. To install or upgrade the Debian package, use `sudo apt install ./teleark_<label>_amd64.deb`; `apt` installs ordinary runtime libraries if the host lacks them. Neither method asks users to install development headers or SDKs.

Ubuntu 22.04 is the build baseline, so older distributions are not promised. Portable outputs include linked libraries but still use the host kernel, display server and graphics drivers. The Debian package installs desktop shortcuts, icons, licenses and notices; its `preinst` compares Debian version strings including distribution revisions.
