# ADR 0047: Windows MSI and distribution build inputs

Status: Windows delivery format superseded by [ADR 0052](0052-visible-windows-exe-installer.md). The transactional engine and distribution build-input decisions remain accepted. Supersedes the Windows Inno installer choice in ADRs 0043 and 0044. Retains ADR 0045's three targets and ADR 0046's tag-only push trigger.

## Decision

Windows x64 releases contain the standalone EXE, portable ZIP and a genuine MSI with an embedded cabinet. WiX 5.0.2 is pinned in the .NET tool manifest and used only while building. Its MS-RL license applies to the compiler; no WiX implementation, extension DLLs or UI library is copied into TeleArk. The package uses Windows Installer's built-in actions and native UI, without a bootstrapper or an end-user .NET dependency. TeleArk remains MIT OR Apache-2.0.

The MSI keeps a stable UpgradeCode and component identities, uses per-user installation, and remembers the install directory. Numeric three-part versions fit MSI limits. Major upgrades reject newer installed versions and remove the old product after InstallExecute, within the rollback transaction and only after the new payload has been installed. This avoids removing the working product before discovering a broken payload. Component identities and file membership must remain stable; a changed resource must follow MSI component rules. Same-version packages explicitly replace their payload. Existing Inno installations are detected by their original per-user registration and versioned executable, adopted in place, and have only their obsolete uninstall registration and installer-owned files removed transactionally. User data is outside the installer payload and unrelated files survive installation and uninstallation.

Native verification uses a clean user account and temporary files. It exercises installation, repair, two MSI versions, downgrade refusal and an unavailable cabinet during native file installation, plus legacy registration/version fixtures and migration rollback. No production failure hooks are added. Failed installer logs are retained in CI and successful checks appear in the job summary.

The existing Telegram distribution secret names are explicitly mapped into all three native build steps; both macOS compilations share the mapping. A preceding validation step rejects an absent or malformed pair without logging it. The quality stage tests validation with synthetic values. Credentials are not available to ordinary source checks or packaging-tool installation steps. GitHub settings are managed by the repository owner.

## References

- [WiX 5.0.2 package and license](https://www.nuget.org/packages/wix/5.0.2)
- [Windows Installer major upgrades](https://docs.firegiant.com/wix/schema/wxs/majorupgrade/)
- [Windows Installer file-version searches](https://learn.microsoft.com/en-us/windows/win32/msi/signature-table)
- [GitHub Actions secret environment mappings](https://docs.github.com/en/actions/how-tos/write-workflows/choose-what-workflows-do/use-secrets)
