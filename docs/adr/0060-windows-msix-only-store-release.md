# ADR 0060: Windows MSIX-only Store release

Status: Accepted — 2026-09-25. This supersedes the Windows release artifacts and optional Store submission described by ADRs 0043, 0047, 0052, 0058 and 0059.

## Decision

Windows release builds produce one x64 Microsoft Store `.msix`. The release workflow no longer builds or publishes the standalone executable, ZIP, setup EXE, MSI or `.msixupload` bundle. It uses the `msix` feature binary and the exact Partner Center identity supplied through `TELEARK_MSIX_IDENTITY_NAME`, `TELEARK_MSIX_PUBLISHER` and `TELEARK_MSIX_PUBLISHER_DISPLAY_NAME`. The package script validates the MSIX manifest and required package contents, then records its SHA256 checksum.

Every matching `vX.Y.Z` tag must point to a commit on `main`, match the Cargo version, and have all three package identity variables, the four Partner Center authentication secrets and `TELEARK_MSSTORE_PRODUCT_ID`. After all platform package jobs pass, CI submits the raw `.msix` with Microsoft's Store Developer CLI. The GitHub Release publishes only after that submission succeeds. Pull requests and manual package previews never submit.

The first Store upload remains manual through Partner Center. Microsoft's current GitHub Actions update path is for free apps that are already published and live in the Store. Certification is still required for each submission. The Store package retains its distinct data directory under `%USERPROFILE%\TeleArk`; it does not import data from earlier unpackaged Windows installs.

Use `scripts/build-local-msix.ps1` on Windows to build the first upload from the private `.env.local` Telegram application credentials and the exact non-secret Partner Center identity values. The output is `dist/TeleArk-<version>-windows-x86_64.msix` plus a checksum manifest.

## Consequences

Windows users install and update through the Microsoft Store. Old Windows EXE/MSI packaging and its Inno Setup/WiX scripts and installer tests are removed. Existing unpackaged data remains separate from the Store package's user-profile data set.

A release tag with missing Store settings fails before native compilation. A Store submission failure prevents the corresponding GitHub Release from publishing, so the two release destinations do not report different outcomes. Microsoft controls certification and live publication.

## Verification

The release job checks the tag's ancestry and version, and uses synthetic configuration cases to cover complete preview/release settings, missing identity, partial credentials and missing Store Product ID. The Windows package build runs the MSIX data-root test, creates one MSIX, and validates its manifest, x64 binary, assets, notices and checksum. The first Partner Center upload and certification are external actions and are not verified locally.

## References

- [Publish app updates to Microsoft Store with GitHub Actions](https://learn.microsoft.com/en-us/windows/apps/publish/msstore-dev-cli/github-actions)
- [Microsoft Store Developer CLI commands](https://learn.microsoft.com/en-us/windows/apps/publish/msstore-dev-cli/commands)
- [MSIX app package requirements](https://learn.microsoft.com/en-us/windows/apps/publish/publish-your-app/msix/app-package-requirements)
