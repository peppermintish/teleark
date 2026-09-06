# Desktop UI review — 2026-09-06

## Scope and reference

The September list-first revision prioritizes task operations in the main
content area. The public [Xunlei desktop product page](https://pc.xunlei.com/)
was inspected for task-list/navigation hierarchy. No source implementation or
third-party assets were copied. This supersedes the old mandatory transfer
summary-card/permanent-inspector geometry, not the Core/frontend architecture.

## Actual window inspection

macOS, native titlebar, light appearance, isolated `--preview-ui` fixtures.
Windows were captured through the desktop UI tool. Requested large windows
were fitted to the active display; a standard Chinese capture was 1318×768
pixels including the titlebar. Backing scale was not independently measured.
These captures appeared in the implementation conversation and are not
committed image files.

| Route | Locale | Requested size | Observed result |
| --- | --- | --- | --- |
| Transfers | zh-CN | 1360×760 | Filters, full-width list, row controls and selection bar visible; checkbox and Details interaction exercised |
| Transfers | en-US | 900×600 | All five toolbar actions and current-speed column fit; table scroll region retained |
| Transfers | ja-JP | 1920×1080 | Display-fitted window; filter labels and row actions fit |
| Settings | zh-CN | 1360×760 | Language cards, section navigation and error feedback visible |
| Channel/login | zh-CN | 900×600 | Dual-method login composition retained; unavailable-adapter notice present; lower content requires scrolling |
| Upload | ja-JP | 900×600 | Footer actions and header close fit; initial missing-file text truncation fixed and recaptured |
| Library | en-US | 1360×760 | Initial vertical filter-stack defect fixed and recaptured; horizontal categories and error/retry state visible |
| File Detail | en-US | 1360×760 | Error/empty-state composition and back action visible |

Inspection found and corrected untranslated direction-filter IDs, the default
black primary palette, an obsolete mock search count, the Library scrollbar's
vertical filter composition, and Japanese upload-placeholder clipping.

## Automated evidence

- 271 workspace tests pass, including 53 GUI tests and all 21 i18n tests.
- New tests cover collapsed-batch expansion, deduplication, visible scope,
  stable Vault selection identity, native lifecycle availability, partial
  command failure and cancellation at task boundaries.
- Format, workspace/all-target check and strict Clippy pass.
- The isolated launch matrix passed 54/54 cases: six routes × en-US/zh-CN/ja-JP
  × 960×640/1360×760/1920×1080. Each process remained alive for a bounded
  one-second observation, then was terminated by its test owner. This is
  startup evidence, not a 54-case pixel comparison. Local raw result:
  `/tmp/teleark-ui-launch-matrix.json`.

## Remaining release review

Authenticated channel/batch rows, completed-file opening against real local
files, unlocked Vault, dark mode, backing-scale variants and the full
route×locale pixel matrix still require dedicated capture. Synthetic preview
lifecycle buttons are intentionally disabled; actual runtime dispatch is
covered by state/scope/unit tests, not a live Telegram account test. GPUI's
native accessibility tree exposes limited content, so full screen-reader and
focus audits remain pending. There is no claim of a commercial release or of
new encrypted-transfer lifecycle capabilities.

No dependency, persistent-format or cryptographic changes were made. Dependency
license/security re-audit and additional documentation builds were not repeated
for this UI-only change; no commit was created. Existing upstream
future-incompatibility warnings remain in the pinned GPUI graph.

## Native throughput settings follow-up

The new Download strategy controls use wrapping buttons and localized explanatory
text in all three catalogs. The final native application builds and its isolated
English Settings preview launches at 960x640. CUA can capture the initial
onboarding overlay but returns `noWindowsAvailable` when attempting coordinate
clicks to reach Downloads; therefore the new controls' three-locale pixel review
is **not claimed as complete**. No real credentials, settings or transfers were
opened by the preview. Next visual check: dismiss onboarding, open Downloads,
verify both strategy buttons and the complete explanation at 960x640 in en-US,
zh-CN and ja-JP.
