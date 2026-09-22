# Changelog

## 0.5.2 · Reliable headless macOS signing

- Fixed hosted macOS release signing by importing the persistent PKCS#12 identity into the temporary keychain as a complete signing identity, verifying its public fingerprint before configuring noninteractive access, and retaining an explicit PEM fallback for older import behavior.
- Added sanitized signing phases and identity diagnostics without exposing private key material or CI secrets.
- Application metadata is 0.5.2. Runtime behavior, SQLite read/write schema 23, automatic upgrades from 0–22, and supported credential, encrypted-file, transfer and recovery codecs are unchanged.

## 0.5.1 · Reliable release regression checks

- Fixed a timing-sensitive download-cleanup regression test and a proxy migration assertion that depended on JSON field order. The checks continue to verify that blocked cleanup permits unrelated downloads and that stale migration preserves the newer route.
- Added clear local macOS packaging commands, signing requirements and final output paths, including the required Apple Silicon and Intel build steps.
- Application metadata is 0.5.1. Runtime behavior, SQLite read/write schema 23, automatic upgrades from 0–22, and supported credential, encrypted-file, transfer and recovery codecs are unchanged.

## 0.5.0 · Credential storage, filtered batches and languages

- macOS defaults to system Keychain for API credentials, proxy passwords and encryption recovery material. Settings offers an explicit warning before switching to unencrypted private SQLite storage. Windows and Linux use SQLite with Keychain unavailable and an explanation. Existing supported data migrates automatically.
- macOS app and standalone releases reuse a self-signed certificate for a stable Keychain identity. Access is noninteractive; locked or inaccessible items report recovery guidance. This does not provide Apple notarization or Gatekeeper trust.
- Download matching indexed channel files by time/type filters, including files outside the visible page, into a unique per-batch folder. Discovery is bounded and cancellable; oversized matches require narrower filters.
- Added Spanish, French, German, Brazilian Portuguese, Russian, Korean and Hindi. Common flows are translated and remaining diagnostic/release text explicitly falls back to English.
- Removed Advanced and user-adjustable indexing controls; notifications are in General. Every displayed byte size/rate uses binary IEC units.
- SQLite read/write schema 23 supports automatic upgrades from 0–22, including skipped releases. Existing encrypted file, manifest, transfer and recovery codecs remain supported and unchanged. API credential codec 1 and proxy-reference codec 2 are documented separately.

## 0.4.14 · Contributor guidance corrections

- Corrected contributor guidance for protecting private configuration on Windows and loading development settings in a separate shell process.
- Clarified encrypted transport framing and aligned supporting guidance with the accepted architecture.
- Application metadata is 0.4.14. SQLite read/write schema 22 and supported automatic upgrades from 0–21 are unchanged; encrypted file, transfer and recovery codecs are unchanged.

## 0.4.13 · Reliable batch downloads and EXE setup

- Stop transfer stops all members of a batch together, prevents queued downloads from starting, and removes partial files. Starting an overlapping download no longer revives a stopped batch or disables all controls.
- Running batch members appear before pending ones. Parent and child selections stay synchronized, and large batch windows retain their scrollbar.
- Available locally reflects files currently on disk. Completed batches can retry deleted files while preserving existing copies and the original history.
- Windows releases again provide a visible EXE setup wizard, retaining repair, MSI upgrades, rollback and downgrade protection.
- Corrected encrypted-download in-flight part-size accounting.
- Application metadata is 0.4.13. SQLite read/write schema 22 and supported automatic upgrades from 0–21 are unchanged; encrypted file, transfer and recovery codecs are unchanged.

## 0.4.12 · Transfer table and key activity layout

- ETA and Progress use wider responsive columns. A Telegram cooldown is shown once in the progress row instead of repeating in the rate slot.
- Encryption key activity opens in a scrollable right-hand panel from the bottom status bar, replacing the full-width strip. On Windows, native full-screen keeps the status bar above the taskbar.
- Application metadata is 0.4.12. SQLite read/write schema 22 and supported automatic upgrades from 0–21 are unchanged; transfer, encrypted file and recovery codecs are unchanged.

## 0.4.11 · Native download recovery and diagnostics

- Native download part retries switch to another transfer connection slot after a failure. A slot that reaches the 60-second request deadline is avoided for later parts of the same download, preventing one stalled slot from exhausting every attempt assigned to it.
- The transfer inspector shows the measured part failure cause, part number, attempt, local connection slot and wait time. New version-2 part events retain this detail in the private session log so the final failure remains explainable after restart; older log records remain readable.
- Application metadata is 0.4.11. SQLite read/write schema 22 and supported automatic upgrades from 0–21 are unchanged. The native partial-file bitmap and encrypted file/recovery formats are unchanged.

## 0.4.10 · Configured Telegram builds and Windows MSI

- Release CI validates the Telegram distribution API ID/hash secrets and passes them to every native compilation, including both universal macOS slices. Installed and portable apps use the same embedded configuration; reports never include the values.
- Windows releases now include a native MSI alongside the standalone EXE and portable ZIP. The MSI supports in-place upgrades, repair, downgrade refusal and rollback, and automatically adopts previous per-user Inno installations while preserving unrelated files and app data. No extra SDK or runtime installation is required for users.
- Application metadata is 0.4.10. SQLite read/write schema 22 and automatic upgrades from supported older schemas are unchanged; encrypted file and recovery formats are unchanged. The single workflow still targets Windows x64, universal macOS and Linux x64 and starts one release run per version tag.

## 0.4.9 · Correct Linux package verification

- Linux release verification compares AppImage and portable archive executables with each other. Debian payloads are verified independently, allowing the expected binary changes made by their different packaging tools. Native installation, downgrade refusal and checksums remain checked.
- CI tests the package comparison before compiling Rust, including corrupt and missing payload cases. Linux packaging and verification now appear as separate steps in the same workflow.
- Application metadata is 0.4.9. SQLite read/write schema 22 and automatic upgrades from supported older schemas are unchanged; encrypted file and recovery formats are unchanged. Targets remain Windows x64, universal macOS and Linux x64; macOS packages remain unsigned and unnotarized.

## 0.4.8 · One release run per version tag

- GitHub Actions now starts on release tags, pull requests and manual dispatch. Pushing `main` and a matching `v*` tag together starts one release run; direct branch pushes no longer trigger CI.
- The release targets Windows x64, universal macOS (Intel and Apple Silicon), and Linux x64, with standalone executables, portable archives and native installers. The macOS and Linux packaging fixes from 0.4.7 remain included.
- Application metadata is 0.4.8. SQLite read/write schema 22 and automatic upgrades from supported older schemas are unchanged; encrypted file and recovery formats are unchanged. macOS packages remain unsigned and unnotarized.

## 0.4.7 · Focused release targets and packaging fixes

- Tagged releases target Windows x64, universal macOS (Intel and Apple Silicon), and Linux x64. Each target provides a standalone executable, portable archive and native installer, for nine package files in the GitHub Release. Windows and Linux ARM64-only release jobs have been removed.
- macOS verification now checks linked libraries in each architecture separately, avoiding a false failure on the universal binary's second heading. Debian packaging includes the copyright metadata required by `cargo-deb`. The three native installers retain in-place upgrades and downgrade refusal.
- Application metadata is 0.4.7. SQLite read/write schema 22 and automatic upgrades from supported older schemas are unchanged; encrypted file and recovery formats are unchanged. macOS packages remain unsigned and unnotarized.

## 0.4.6 · Unified CI and wider native releases

- One GitHub Actions workflow runs quality and platform tests for branches and pull requests, and builds/publishes installers for matching `vX.Y.Z` tags. Its summaries show stage results, LF policy counts, Rust cache hits and release checksums. An optional manual package preview builds without publishing.
- Windows x64 and ARM64, universal macOS (Apple Silicon and Intel), and Linux x64 and ARM64 now have native release packages. Windows uses a static CRT; macOS uses system frameworks; Linux AppImage and portable archives bundle linked runtime libraries. Debian installers resolve ordinary OS runtime packages automatically. No end-user build SDK is required.
- Windows, macOS and Debian installers retain in-place upgrades and reject older versions before replacing files. Application metadata is 0.4.6; SQLite schema 22 and supported encrypted/recovery formats are unchanged. macOS packages remain unsigned and unnotarized.

## 0.4.5 · Portable recovery and release installers

- Encrypted uploads and downloads stream through bounded memory in 512 KiB transport blocks. New uploads do not create ciphertext spools; downloads write authenticated plaintext, and compatible older files remain readable.
- Upload recovery can use authenticated remote receipts across a restart or a second device. Verified published containers are reused, while unpublished work restarts with fresh encryption identity. Transfer charts and timelines show measured activity with bounded history.
- When a bound private channel is unavailable, management verifies remote state before discovering or creating a replacement. Existing local history and keys remain intact; files lost with the old remote channel are not restored.
- Tagged releases build standalone executables, portable archives and native installers for Windows x86_64, macOS arm64 and Linux x86_64. Installers upgrade in place and reject an older version with an explanation. The workflow checks that the tag matches the application version before publication.
- CI checks every tracked plain-text file for CRLF on Linux, Windows and macOS, rejects `.cmd` and `.bat` scripts, and reports counts and violations in the GitHub Actions summary. The release workflow applies the same policy before publication.
- Updated application and macOS bundle metadata to 0.4.5. SQLite remains at read/write schema 22; supported older schemas migrate automatically. Encrypted file and recovery readers retain their documented compatibility. macOS packages remain unsigned and unnotarized.

## 0.4.4 · Session unlock and resilient background work

- The storage page groups channel identification changes in a highlighted card with the exact message, pin and description updates. Locked-file cards keep their controls inside the border and scroll naturally at small sizes.
- Vault unlock lasts for the account session. Explicit lock hides decrypted names and paths while admitted transfers, queued batches and synchronization continue; new operations require unlock. Upload unlock returns to confirmation.
- Existing private-channel bindings remain fixed, with explicit management repair, file-local health and retained historical key versions. Proxy routing fails closed and applies across Telegram connections.
- TeleArk setup separates instructions, the current task and state changes into distinct cards. Phase badges, response panels and action bars make waiting, retries and completion easy to distinguish.
- Recent state changes are shown in timestamped rows, with an expandable bounded history. The independently scrolling inspector uses the same cards in English, Simplified Chinese and Japanese, in both themes.
- Channel browsing uses local projections and background synchronization. Large libraries and transfer histories use bounded updates, indexed reads and background filesystem work to keep the window responsive.
- Catalog server failures no longer overwrite the login state. Reads support cancellation and bounded retry, and incomplete discovery never authorizes private-channel creation. The observed Telegram catalog 500 error remains unresolved and also reproduces with the pre-performance code.
- Database schemas 0–14 upgrade automatically to read/write schema 15, including skipped releases. Existing encrypted files, recovery bundles, sessions and transfer checkpoint codecs retain their supported versions.
- Updated the application and macOS bundle to 0.4.4. Packaging remains unsigned; protected real-account and complete platform qualification remain outstanding.

## 0.4.3 · Upload activity at every step

- Uploads now show storage checks, reading and encryption, waiting for Telegram, data transfer, remote confirmation, readback verification and manifest publication, with elapsed time and current object bytes.
- Fixed progress remaining at zero until an entire encrypted part finished uploading and verification. In-flight progress updates as Telegram reads the byte stream; 100% is reserved for completed finalization.
- Collapsed upload batches show the current member's activity. Queue rows appear before network preflight, and complete storage discovery runs once per batch while each file still rechecks its destination.
- Private storage setup automatically verifies remote identity, preserves cross-device discovery, and explains why managed channel messages must be kept. Conflicting or damaged identities stop uploads.
- New sign-ins start with QR login and offer a secondary phone method. Switching accounts requires confirmation and cannot interrupt pending uploads.
- Library selection supports bulk reveal and account-scoped downloads. Newly uploaded files appear immediately, and older scans cannot replace their completion records.
- Updated English, Simplified Chinese, Japanese and macOS bundle metadata. Database and encryption formats are unchanged; encrypted transfer controls and independent security qualification remain incomplete.

## 0.4.2 · Clearer Library, compact Transfers

- Replaced All Files with Local files and Remote files tabs. Local files shows accessible downloads for the current account and imported originals; remote files shows the account's indexed Telegram catalog. File types have a separate filter.
- Local file pages read current disk metadata, omit deleted or inaccessible files, and support cancellable paging and search without starting transfers.
- Transfer files and batch rows now use the same compact 42-point height as Library rows. Upload and download arrows identify Transfers; batch details and controls remain available.
- Fixed downloading again after deleting a local output: new tasks avoid paths retained by transfer history and preserve earlier records.
- Updated English, Simplified Chinese, Japanese and macOS bundle version metadata. Schema and encryption formats are unchanged.

## 0.4.1 · Everyday usability

- Library file details now offer account-scoped downloads for indexed Telegram files, with explicit guidance for unavailable sources. Local files retain Open and Reveal actions.

- Fixed a channel pagination crash caused by re-entering the table update. Cancelled or stale requests cannot restart loading after navigation or account changes.
- Local Library now explains indexed Telegram files, labels them as indexed instead of uploaded, and shows the original channel and scoped message ID. Removed the placeholder parts table.
- Added a collapsible icon sidebar, direct Local Library access, a separate channel browser, and visible free space for the download disk.
- Fixed scrolling in Raw Files and transfer inspectors so details scroll independently of the underlying list.
- Batch rows now show their source, file names, counts, time, and real member lists. Multiple uploads can be reviewed together; stopping a batch skips remaining files after the current file finishes.
- Downloaded files are checked in the background. Missing, changed, and unavailable files have distinct states; a missing native download can be downloaded again as a new task.
- Added original TeleArk artwork and a macOS app bundle with an application icon.
- Schema v10 preserves account-scoped local output identities across restarts. Encryption and manifest formats are unchanged.

## 0.4.0 · A new home for your files

### Made for the Mac
- Rebuilt the desktop layer with GPUI Kit 0.6 and the matching gpui-pre family. Native typography, quiet surfaces, consistent controls, original vector symbols, and light/dark appearance.
- Transfers and TeleArk storage stay at the top of the sidebar. Channels scroll independently, and refreshing never changes your current page or selection.
- A centered account screen restores your avatar and name, with Log In and Switch Account. New sessions offer phone, QR, code, and two-step verification.
- Unlock from the place that needs a key and continue your upload, download, or browse action. Changing pages no longer immediately locks the vault.
- Unlock dialogs support Tab navigation, Return submission, Escape dismissal, and readable text at the smallest window size.

### Your own private channel
- Create or rediscover a private channel owned by your Telegram account, with a distinct TeleArk destination in the sidebar.
- Files shows authenticated manifests as complete logical files. Raw Files exposes ordinary files, encrypted parts, and manifests with their original names and metadata.
- A built-in guide explains channel ownership, encryption, raw objects, recovery keys, and why manifests and parts must be kept.
- Existing files in Saved Messages remain recoverable through Settings → Key Vault. New encrypted uploads target the private channel.

### Everything in its place
- Transfer selection, batches, pause, resume, cancel, retry, safe deletion, file inspection, and live/replay diagnostics remain available. Advanced controls and details are tucked away until needed.
- Settings groups daily preferences clearly and collapses less-used controls. Local import, search, file actions, indexing, storage paths, throughput strategies, and the full key lifecycle remain accessible.
- Added native About and Settings menu items plus keyboard shortcuts for transfers, storage, search, refresh, and upload. About includes this complete release record.
- Updated English, Simplified Chinese, and Japanese together.

### Reliability and compatibility
- Native download history now records its Telegram account. Switching pauses active downloads and keeps progress; another account cannot resume or retry them.
- Schema v9 preserves older history. Tasks without an account bind only when restoring the existing session, never to an arbitrary new login.
- Private-channel discovery validates ownership, privacy, and the description marker. It preserves renamed bindings and asks you to choose when multiple candidates exist.
- Simplified contributor documentation while preserving format specifications, security constraints, and architectural decisions. Git checkpoints preserve the documentation before and after consolidation.
- Encrypted formats remain provisional pending independent security review. No live Telegram account is required by ordinary tests.
