# ADR 0043: Release artifact and installer version contract

Status: Partially superseded by [ADR 0044](0044-single-workflow-native-release-matrix.md). The version and installer downgrade contracts remain in force; the workflow, target matrix and asset count below describe the 0.4.5 release.

## Decision

The release workflow builds one native desktop target per supported platform: `x86_64-pc-windows-msvc`, `aarch64-apple-darwin`, and `x86_64-unknown-linux-gnu`. Every platform must produce a directly runnable executable, a portable archive and a native installer. Missing outputs fail the matrix job; publication copies an exact nine-file manifest rather than whatever happens to be present. Checksums and legal files accompany the assets. Manual dispatch uploads artifacts without publishing a GitHub release. Tagged publication refuses to replace an existing release's assets.

The `teleark-gui` Cargo package supplies the numeric `X.Y.Z` version for binary and installer metadata. A publishing tag must be exactly `vX.Y.Z`; a mismatch fails before building. The `v` prefix and branch names do not enter installer version fields. Manual runs append the source commit to artifact filenames, while the native installer version remains numeric. Schema and codec versions are independent of this application release version.

The Windows installer retains the existing Inno Setup AppId and per-user install directory. Its setup initialization compares the version in the existing uninstall registration before writing files. The macOS product package installs at a fixed `/Applications/TeleArk.app` path and checks the installed app's bundle version in both its distribution check and component preinstall script. Debian's `preinst` compares the installed package version with the incoming package version before unpacking. These installers accept same-version repair and newer-version in-place upgrades, and reject older or uncomparable versions with a stated reason. Copying a portable archive or standalone executable is a manual operation outside the installer guards.

## Evidence and limits

The Windows installer compiles locally with Inno Setup 6.7.3. A local silent install and a reinstall with an older registered version succeeded in the same directory, retained an unrelated file and restored the installed version. A setup with a newer registered version returned a failure, logged the refusal and left the executable hash unchanged. The workflow adds equivalent Windows runner checks, macOS Installer upgrade/downgrade checks, and an extracted Debian `preinst` downgrade test. Those hosted checks require a workflow run; local syntax and structure checks alone do not establish macOS or Linux installer behavior. Unsigned macOS output still requires signing and notarization for a public release.
