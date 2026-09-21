# ADR 0044: Single-workflow native release matrix

Status: Partially superseded by [ADR 0045](0045-focused-release-targets.md) for target list and asset count. The single-workflow structure remains in force. This ADR superseded ADR 0043's workflow structure, target list and asset count; ADR 0043's numeric version and installer upgrade/downgrade contract remains in force.

## Decision

One `ci.yml` owns both CI and tagged publication. Linux quality runs for every trigger. Branches and pull requests then test Windows and macOS; a matching `vX.Y.Z` push instead builds packages and publishes one GitHub Release. An optional manual package preview uploads artifacts with a commit-suffixed label but cannot publish. The release jobs use ordinary `if` conditions and `needs` edges; there is no manual promotion or second release workflow. Branch and tag pushes can create two runs for one commit, but only the tag run starts installer runners. The tag reruns Linux source gates so the published ref is directly checked.

The package matrix covers native Windows x64 and ARM64, one universal macOS binary containing Apple Silicon and Intel slices, and native Linux x64 and ARM64. Windows builds link the CRT statically. The macOS package audit rejects non-system dynamic libraries. Linux AppImage and portable archive outputs are made from the same AppDir with linked runtime libraries and distribution copyright notices; Debian packages declare ordinary OS runtime dependencies for automatic package-manager resolution. The host still provides its kernel, graphics driver and platform libraries. No end user installs a compiler, SDK or language runtime.

Each package job verifies architecture, archive contents, installer metadata, checksums and downgrade refusal. Windows and macOS jobs perform an actual in-place upgrade. Linux verifies its Debian pre-install guard and installs the package on the native runner. Tagged publication requires all five package jobs and copies exactly 15 expected package files before generating unified checksums and creating the release. Stage, LF, cache and asset reports appear in GitHub Actions job summaries.

## Limits

Ubuntu 22.04 is the Linux build baseline; older distributions are not promised. Linux AppImage execution may need FUSE, and the portable archive offers an `AppRun` path without it. macOS output is unsigned and unnotarized until distribution signing is configured. The Windows Inno compiler has separate commercial-use license terms; its runtime installer does not change TeleArk's `MIT OR Apache-2.0` license. Native runner builds and package tests establish the listed architecture and installer behavior only after a successful hosted run.
