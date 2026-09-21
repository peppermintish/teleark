# ADR 0045: Focused release targets

Status: Accepted. Supersedes ADR 0044's package target list and asset count; retains its single-workflow CI/CD structure and ADR 0043's version and installer contracts.

## Decision

Tagged releases build three package jobs: Windows x64, one macOS universal executable containing Intel x86_64 and Apple Silicon arm64 slices, and Linux x64. Each job produces a standalone executable, portable archive and native installer. Publication requires exactly nine package files, legal notices and checksums. Windows and Linux ARM64-only packages are removed. Windows ARM devices may run the x64 package through OS emulation, but this is not a native ARM64 release target.

The macOS verifier inspects linked libraries separately for both slices, so universal-binary headings cannot be mistaken for dependencies. The Debian package declares copyright metadata required by `cargo-deb`. Each package job retains native artifact, installation and downgrade checks before publication.

## Limits

The macOS package is unsigned and unnotarized. Linux remains built against Ubuntu 22.04 and needs host kernel, desktop and graphics facilities; Debian may install ordinary runtime packages. A successful hosted tag run, rather than the local version tag alone, establishes package verification and publication.
