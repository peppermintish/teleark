# ADR 0052: Visible Windows EXE installer

Status: Accepted. Supersedes ADR 0047's user-facing MSI artifact choice. Retains its transactional installation engine and Telegram distribution build inputs.

## Decision

Ship `TeleArk-Setup-<label>-windows-x86_64.exe` again. The MSI had no authored setup dialog sequence, so launching it did not provide the expected setup wizard. Restore the Inno Setup presentation from the September 21 installer: Welcome, destination, confirmation, progress and completion, followed by an optional application launch. Normal launches are interactive; unattended installation requires explicit command-line flags.

The EXE embeds the existing MSI rather than transferring ownership between installer systems. Windows Installer alone owns files, shortcuts and uninstall registration. This keeps MSI-to-EXE upgrades, older Inno migration, repair, downgrade refusal and failed-upgrade rollback inside the existing transaction. Inno has no uninstaller or uninstall registration. No installed app is removed before its replacement is ready. The MSI is a private build artifact and is excluded from publication and the portable ZIP.

The wizard shows the registered directory on an upgrade and prevents a misleading relocation choice. A same-product reinstall explicitly repairs all files. Interactive installation invokes the native progress dialog with cancellation enabled; a nonzero failure or cancellation result stops the wizard before its completion and launch actions. Windows Installer handles rollback. Setup logging is always enabled, with an adjacent MSI log for the detailed outcome. Restart-required success is presented by the wizard; the engine cannot restart Windows itself.

Inno Setup 6.7.3 is pinned in CI. The workflow installs the immutable official upstream release because the Chocolatey community source does not publish the required package version; the bootstrap verifies the pinned SHA-256 digest and Authenticode signature, and the build helper verifies the installed `ISCC.exe` version before compiling. Its permissive license allows the unmodified setup runtime in the EXE, with its copyright and website notices retained. WiX remains a build-only dependency. Original TeleArk code remains MIT OR Apache-2.0. Installer-owned messages are generated with teleark-i18n from synchronized catalogs; the standard wizard remains English, matching the previous EXE.

The 2026-09-23 verification correction supersedes reading `ISCC.exe`'s Windows `ProductVersion` field. A [hosted probe](https://github.com/wood-made/teleark/actions/runs/35819584440) found both version resources set to `0.0.0.0`, while `--version` returns exit code 1 in Inno Setup 6. Both bootstrap and build helper now compile an output-free script whose `Ver` preprocessor value must equal `EncodeVer(6,7,3)`. The bootstrap regression uses the real installed compiler after checksum and publisher validation and rejects a wrong expected version, so a synthetic executable cannot hide a mismatch in the official binary. The [Windows 2022 regression](https://github.com/wood-made/teleark/actions/runs/35819885640) passed this path.

## Verification

Native tests run fresh installation, missing-file repair, retained location, MSI-to-EXE upgrade, real missing-cabinet rollback, downgrade refusal and uninstall preservation. Release verification uses the actual release identity on clean CI accounts. Local tests may use a private identity; product registration, upgrade searches, every component, registry paths, directory and shortcut must all be isolated and validated before execution. A fixture that merely changes ProductCode or UpgradeCode in the Property table is unsafe: the Upgrade table independently controls which installed products are removed.

Application schema 22 and supported automatic upgrades from schemas 0–21, encrypted file formats and recovery codecs are unchanged. This packaging change needs no data migration or manual reset.

## References

- [Inno Setup installer event functions](https://jrsoftware.org/ishelp/topic_scriptevents.htm)
- [Inno Setup uninstaller control](https://jrsoftware.org/ishelp/topic_setup_uninstallable.htm)
- [Inno Setup compiler command-line parameters](https://jrsoftware.org/ishelp/topic_compilercmdline.htm)
- [Windows Installer UI and command-line options](https://learn.microsoft.com/en-us/windows/win32/msi/command-line-options)
- [Windows Installer Upgrade table](https://learn.microsoft.com/en-us/windows/win32/msi/upgrade-table)
