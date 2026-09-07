# Desktop UI contract

The v0.4.0 design replaces the old reference-image geometry with an Apple-style file workspace: system typography, quiet gray surfaces, clear hierarchy, blue primary actions and generous spacing. Existing functionality stays discoverable; advanced mechanics appear on demand. This document governs presentation, not backend capability. Consult [status](IMPLEMENTATION_STATUS.md) before exposing a control as implemented.

## Navigation and layout

- TeleArk, Channels, Transfers and Library have direct icon navigation. A top-left persisted toggle switches between a 64-point icon rail and 184-point labels. Settings and Account remain in the footer. Channel titles have no decorative hash prefix. The virtualized channel list appears only in its own workspace; disk space stays visible in the bottom status bar.
- Refresh changes source data without changing the page, selected task/file, filter or scroll owner. Replacing a source cancels its previous load and rejects late responses by account/source generation.
- Use a native macOS titlebar and system window/fullscreen behavior. Support a 900×600 minimum; compact/standard/spacious layouts use the collapsible sidebar and 12/20/24-point content padding.
- Main content owns the remaining width/height. File and transfer lists keep a real flex-height viewport at every size. Details open in a dismissible inspector with an independent retained scroll handle, intrinsic content height and an occluding hitbox. Wheel input, including at either boundary, cannot scroll the underlying list.
- Library uses compact segmented Local files / Remote files tabs with a sliding selection indicator. File types have a separate menu; All Files and Recent are not source categories. Local files reads accessible downloads for the current account plus imported originals from disk, excluding deleted/unavailable paths. Remote files reads only the current account’s indexed Telegram catalog. Local counts and bytes describe displayed rows, not an unmeasured whole-disk total.
- Compact Library hides secondary type/source/date/crypto/part columns, preserving name, size and state. Full metadata remains in details. Long original filenames truncate intentionally with an accessible detail path; prose and translated actions wrap where appropriate.

## Account entry

A fresh session defaults to one large, centered QR code, loaded automatically after session restoration finds no authorized account and credentials are available. An underlined 11-point text action below switches to phone sign-in and back, retaining a 34-point click target; code/2FA steps appear only when requested. There is no initial generate/refresh QR button: the retained poll owner refreshes expiring codes. Errors offer an explicit retry, and missing API configuration retains its setup action. Switching methods clears code/password inputs, stops the poll owner and invalidates late auth responses. Explain errors locally and provide a way to change method. QR pixels are black on white in every appearance. Never record QR links or login inputs.

An existing authorized session shows the real avatar (initials fallback), display name, Log In and Switch Account centered in the window. Log In enters the workspace; restoration alone does not start native transfers. Switch Account first opens a focused confirmation modal explaining that continuing signs out the current account. The modal has no close button, ignores Escape/backdrop dismissal, and requires an explicit Cancel or Sign out and switch choice. Confirming pauses native tasks before sign-out and preserves files/history; successful sign-out returns to automatic QR login. If a Vault transfer is active, explain that it must finish first. Configuration should not cover the whole login screen automatically; provide the API setup action when needed.

## TeleArk storage

The dedicated TeleArk entry has distinct brand/private styling and is excluded from the ordinary channel list. Setup shows automatic remote discovery/verification and guidance, with no Create, Rediscover or candidate-selection controls. A unique remote identity is required on every device; conflicting or damaged evidence stops setup and writes with localized feedback. The ready view reports found/created outcomes. The remote channel uses the stable reserved title `🔒 TeleArk · Managed Storage`, a localized warning description and a pinned identity/warning message. The guide explains that editing/deleting the channel or messages in other apps can break file access and recovery. Transient failures receive bounded automatic rechecks.

| Projection | Meaning |
| --- | --- |
| Files | Authenticated manifests reconstructed as complete logical files, with original name and logical size |
| Raw Files | Original Telegram documents and candidate part/manifest objects; recognizable names do not establish authenticity |

Keep Upload prominent. The Help control opens a guide covering private-channel creation, complete files, raw objects, keeping manifests/parts, and offline recovery material. The guide is fully visible during setup and collapsible afterwards. Preserve ordinary source titles/names/captions exactly. Only remotely verified TeleArk-owned storage receives app-managed branding; title alone never proves identity.

A locked Files view has a direct Unlock action. The same focused dialog supports setup, password unlock and advanced recovery. Retain the requested browse/upload/download intent and continue after success. New recovery bundles must be acknowledged/exported before continuing. Changing routes does not itself relock the Vault. Inactivity policy is explained in Key Vault settings.

Legacy Saved Messages recovery lives under Key Vault → Advanced. It is a recovery source, never a new upload destination.

## Transfers, uploads and settings

Transfers uses an original paired upload/download arrow icon and leads with the list, speed and filters. Select rows or expand Manage for bulk operations; expose only lifecycle actions supported by that runtime. Native tasks support pause/resume/cancel/retry and confirmed terminal-history deletion. Vault controls remain limited by the current owner. Deletion never deletes successfully downloaded user files. Stable task identities survive sort/filter changes. Ordinary rows, batch headers and inspector member rows use the same 42-point height as Library. The virtual list retains its scroll owner. Batch headers show source, names and completed/failed counts in two compact lines; time remains in details; a separate chevron expands children and Info opens actual virtualized members. File availability is distinct from historical completion, with explicit missing/changed/unavailable states.

Details retain file/message metadata, verification, timing, failure guidance and Live/Replay telemetry. Unknown physical connection/DC values stay unavailable. Preview data is labeled and must never substitute for a failed runtime. Local Library identifies remote metadata as indexed, explains how browsing populates the catalog, and shows actual source account/channel/message IDs. It does not invent parts, upload timestamps or verification evidence. Library rows expose Reveal for local files and Download for uniquely identified native Telegram files at the far right; transfer list rows likewise offer Reveal without a separate Open action; details retain Open/Reveal and Download. Downloads use the record’s source account/channel/message, recheck the account after destination preparation, and open Transfers on successful enqueue. Unavailable or managed sources show localized guidance. Library checkboxes select actionable loaded rows (up to 5,000), keyed by stable local/catalog identity. Changing source/type/search clears selection; loading another page preserves it. The selection toolbar reveals one file per distinct parent folder locally or queues remote files in per-channel batches with globally reserved destination names. Preparation and enqueue run off the UI thread, are cancellable between files/batches, and retain completed batches on partial failure; only successfully queued identities leave the selection. Cancellation does not undo already queued transfers, which remain manageable in Transfers. Account switching and app destruction cancel pending bulk work, and the runtime revalidates account scope when enqueueing.

DataTable pagination defers owner callbacks until the table entity is released. Account/chat/generation checks reject stale callbacks; cancellation disables automatic loading until an explicit load, and failures require explicit retry.

The upload dialog keeps its title and bottom actions visible while the body scrolls. It supports up to 128 files with a scrollable review/removal list and total count/size, followed by real account and destination, a short explanation and queue action. Crypto/splitting mechanics are collapsed. Content/name/metadata encryption and the current part ceiling are runtime policy, not inactive editable options.

Settings initially exposes General, Appearance, Accounts, Key Vault, Downloads, Storage and About. Uploads, Indexing and Notifications remain behind Advanced. Key Vault likewise keeps password/recovery rotation and legacy recovery in Advanced. Preserve local import/search/open/reveal, storage paths, throughput/soft-limit policy and diagnostics access.

About shows the running version, the full localized release record and expandable license notices. The English release text must equal `CHANGELOG.md`; a regression test enforces parity.

## Controls, keyboard and accessibility

Use GPUI Kit Base behavior for buttons/dialogs/focus, Component controls for standard visual semantics, and DataTable, uniform lists or variable-height lists for large data. The original application bitmap and its generation provenance live under `crates/teleark-gui/assets/icons`; the native app bundle includes its ICNS variants. Keep palette in `theme.rs`, reusable controls in `components.rs` and responsive geometry in `layout.rs`.

Every icon action has a localized accessible label and tooltip. Disabled actions explain the missing prerequisite where useful. Modal focus is trapped; dismissal restores workspace focus. Escape closes a modal before leaving fullscreen. Return submits only the modal's intended action; Tab must not reach obscured content.

| Shortcut | Action |
| --- | --- |
| ⌘1 / ⌘2 | Transfers / TeleArk |
| ⌘U | Upload |
| ⌘F / ⌘R | Search / refresh current context |
| ⌘, | Settings |
| ⌃⌘F | Native fullscreen |
| ⌘M / ⌘Q | Minimize / quit |

All prose, labels, validation, status, menus, tooltips and accessibility strings follow [i18n](I18N.md). Use system-readable contrast and sizes; do not shrink translations to force a layout. Product marks, protocol identifiers, exact version numbers and user/source content are deliberate exceptions to localization.

## Review

Use the isolated [development matrix](DEVELOPMENT.md#isolated-ui-review). Review 200-channel navigation, 5,000 raw rows, all three locales, minimum/default/display-fitted large windows and light/dark. Exercise keyboard as well as pointer actions. Process survival and geometry tests are not pixel or screen-reader audits. Keep actual observations and remaining checks in [status](IMPLEMENTATION_STATUS.md), rather than maintaining a second competing UI report.

Upload submission immediately shows a localized preparation notice while no upload is running, including before runtime rows exist. Runtime queue rows appear before channel verification. Verification failure/cancellation retains terminal task rows. Account switching is blocked while submission/preflight is pending as well as while Vault files run.

Upload rows show a localized current phase in place of the generic Uploading label. The page status line and file details retain phase duration and current object byte counts, even when a batch is collapsed or no part has verified yet. Waiting preparation displays an unavailable percentage instead of an idle 0%; actual byte-stream activity advances progress, with 100% reserved for successful finalization. Batch summaries select the running member, and failed/cancelled/completed rows hide active-phase text. Stop after current retains its existing explicit boundary.
