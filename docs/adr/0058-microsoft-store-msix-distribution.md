# ADR 0058: Microsoft Store MSIX distribution

Status: Accepted — 2026-09-24. This adds a separate opt-in Windows distribution; it does not replace the existing EXE setup wizard or portable release.

## Decision

Build an MSIX upload bundle from the Windows release executable using Microsoft's MakeAppx tooling. The package is a full-trust desktop app targeting Windows 10 version 1809 or later and declares `runFullTrust` to launch the native desktop process. Explain that requirement in Partner Center's restricted-capability submission options. Its `Identity Name`, `Publisher`, and `PublisherDisplayName` come from the exact Partner Center values supplied to CI as `TELEARK_MSIX_IDENTITY_NAME`, `TELEARK_MSIX_PUBLISHER`, and `TELEARK_MSIX_PUBLISHER_DISPLAY_NAME`. All three repository variables must be present to enable Store packaging. The workflow attaches a `.msixupload` to Windows artifacts and, for a version tag, to the GitHub Release with the other checksummed assets. It never submits to or publishes through Partner Center.

Compile a separate `msix` feature variant of `teleark-gui`. The existing unpackaged Windows build continues to use `%LOCALAPPDATA%\TeleArk`. The MSIX variant instead uses `%USERPROFILE%\TeleArk` for the database, Telegram session, recovery data and default managed-file directories. This path is outside AppData, so MSIX AppData virtualization does not split the database from its SQLite WAL sidecars, and files remain after package uninstall. The Store build starts an independent data set and does not import from an existing unpackaged installation; this is the clean-slate Store implementation requested for this distribution path. The folder uses normal Windows profile ACLs and is accessible to processes running as the same user.

Do not declare `unvirtualizedResources`. That restricted capability disables AppData virtualization broadly, is not intended for general Store apps and risks Store rejection. Use the durable user-profile data location instead. Store package versions map monotonically from Cargo `major.minor.patch` to `(major + 1).minor.patch.0`; the package script rejects components that cannot be represented. Keep the Partner Center identity stable for all future package updates. The Store signs the package after certification; CI keeps no production signing key.

## Consequences

Users of the Store package maintain a separate data set from the existing Windows installer and portable executable. Default files persist after uninstall in `%USERPROFILE%\TeleArk`; users may remove that folder when they want its database, Telegram session and managed files deleted. This is outside package-private LocalState and therefore is not removed automatically by Windows. Store distribution stays optional until the Partner Center identity variables are configured. First Store submission and certification remain manual.

## Verification

The packager validates the supplied identity/version inputs, manifest and package file set before producing `.msixupload`. Windows CI builds the Store feature variant separately, checks its data-root selection, and requires exactly one upload bundle when the Store variables are configured. Tagged publication counts and hashes that bundle. Partner Center certification and post-certification Store signing are external checks and do not run in GitHub Actions.

## References

- [MSIX package requirements](https://learn.microsoft.com/en-us/windows/apps/publish/publish-your-app/msix/app-package-requirements)
- [MSIX desktop app packaging](https://learn.microsoft.com/en-us/windows/msix/desktop/desktop-to-uwp-manual-conversion)
- [Flexible filesystem virtualization](https://learn.microsoft.com/en-us/windows/msix/desktop/flexible-virtualization)
- [Restricted app capabilities](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/app-capability-declarations)
