# ADR 0060: Windows MSIX-only Store release

Status: Accepted — 2026-09-25. This supersedes the Windows release artifacts and optional Store submission described by ADRs 0043, 0047, 0052, 0058 and 0059.

## Decision

Windows release builds produce one x64 Microsoft Store `.msix`. The release workflow no longer builds or publishes the standalone executable, ZIP, setup EXE, MSI or `.msixupload` bundle. It uses the `msix` feature binary and the exact Partner Center identity supplied through `TELEARK_MSIX_IDENTITY_NAME`, `TELEARK_MSIX_PUBLISHER` and `TELEARK_MSIX_PUBLISHER_DISPLAY_NAME`. The package script validates the MSIX manifest and required package contents, then records its SHA256 checksum.

Every matching `vX.Y.Z` tag must point to a commit on `main` and match the Cargo version. CI validates the package identity and required configuration, then starts quality, platform tests and Linux/Windows/macOS package jobs concurrently. The Store submission uses Microsoft's legacy MSIX API and waits for quality, all OS tests and the verified Windows package; GitHub Release assembly waits for all packages and successful Store submission. Authentication and draft handling are specified in [ADR 0062](0062-msstore-submission-api-authentication.md). Pull requests and manual package previews never submit.

The legacy Store Submission API requires the app to have at least one completed submission with age ratings information. Create the app and complete its first submission manually through Partner Center; later version-tag submissions use the API. Certification is still required for each submission. The Store package retains its distinct data directory under `%USERPROFILE%\TeleArk`; it does not import data from earlier unpackaged Windows installs.

Use `scripts/build-local-msix.ps1` on Windows to build the first upload from the private `.env.local` Telegram application credentials and the exact non-secret Partner Center identity values. The output is `dist/TeleArk-<version>-windows-x86_64.msix` plus a checksum manifest.

## Consequences

Windows users install and update through the Microsoft Store. Old Windows EXE/MSI packaging and its Inno Setup/WiX scripts and installer tests are removed. Existing unpackaged data remains separate from the Store package's user-profile data set.

Missing package identity, Telegram distribution or macOS signing configuration fails during preflight before package work begins. Store credentials and Product ID are validated only inside the protected `store-submission` environment after the verified Windows package is ready, so those settings remain unavailable to preflight and package jobs while the platform jobs run in parallel. A Store submission failure prevents the corresponding GitHub Release from publishing, so the two release destinations do not report different outcomes. Seller ID is not an input to the legacy API. Microsoft controls certification and live publication.

## Verification

The release job checks the tag's ancestry and version, and uses synthetic configuration cases for complete preview/release settings, missing identity, partial credentials and missing Store Product ID. Deterministic mocked HTTP tests cover token form construction and redaction, submission metadata, ZIP upload, block ordering, commit, polling and failures; workflow graph checks verify job dependencies and tag-only publishing. The Windows package build runs the MSIX data-root test, creates one MSIX, and validates its manifest, x64 binary, assets, notices and checksum. Partner Center configuration, the first submission, live API behavior and certification are not verified locally.

## References

- [Create and manage submissions using Windows Store services](https://learn.microsoft.com/en-us/windows/uwp/monetize/create-and-manage-submissions-using-windows-store-services)
- [Manage app submissions](https://learn.microsoft.com/en-us/windows/uwp/monetize/manage-app-submissions)
- [Update an app submission](https://learn.microsoft.com/en-us/windows/uwp/monetize/update-an-app-submission)
- [MSIX app package requirements](https://learn.microsoft.com/en-us/windows/apps/publish/publish-your-app/msix/app-package-requirements)
