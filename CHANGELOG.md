# Changelog

## Unreleased · Everyday usability

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
