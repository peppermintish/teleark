# Application access interface

The application PIN, proxy configuration and transfer confirmations use a shared
desktop visual language. The access gate and background ownership contract are
defined in [ADR 0037](adr/0037-application-pin-and-transfer-drain.md).

## Reference review

The following GitHub projects were reviewed after checking their MIT licenses:

- [VS Code settings editor](https://github.com/microsoft/vscode/blob/main/src/vs/workbench/contrib/preferences/browser/media/settingsEditor2.css),
  [license](https://github.com/microsoft/vscode/blob/main/LICENSE.txt): bounded
  content width, separate navigation and content, restrained label/description
  hierarchy, and independent scrolling.
- [Microsoft Fluent UI dialogs](https://github.com/microsoft/fluentui/tree/master/packages/react-components/react-dialog/stories/src/Dialog),
  [license](https://github.com/microsoft/fluentui/blob/master/LICENSE): distinct
  title, content and action areas; a clear primary action; inert background
  content during a modal operation.

These are design references, not new dependencies. The implementation uses
TeleArk's own GPUI Kit components, icons, palette and layout constants. No source,
fonts or artwork were copied from these projects.

## Layout decisions

- The lock screen centers a 384-point account panel on a quiet tinted background.
  The avatar, PIN field and primary sign-in action form one group. A short
  background-work note is separated from authentication; account switching and
  proxy access remain secondary footer actions. The shell status remains inert.
- Proxy and application-lock settings use a bounded 640-point form. Connection
  mode uses a segmented selection; address and authentication fields share two
  aligned columns. Small windows scroll the form while access navigation remains
  available. Other settings retain their existing content width.
- Confirmation dialogs share a 440-point surface, icon/title/description header,
  and separated action footer. Transfer confirmations add a named activity panel;
  waiting displays the fixed request time and a cancellation action. Cancellation
  changes only the pending operation, never the underlying transfer.
- Sync history separates fixed timestamps from activity descriptions. Rows keep
  the shared 24-point height and 12-point text, semantic state markers, virtualized
  rendering, and complete hover text. All existing event sources and retention
  disclosures remain present. The redesign introduces no animation or timer.
- Settings navigation uses the shared button component, with native keyboard and
  accessibility behavior. Language choices use two columns within the compact
  General panel.

## Review scope

Use English and light appearance with synthetic `--preview-ui` data. Review the
lock screen, expanded proxy form, General settings, sync inspector, and account /
transfer confirmations at 900×600 and actual native full-screen. Check scroll
boundaries, primary action reachability, cancel/wait transitions, and inert status
bar interaction. Runtime PIN verification and transfer drain behavior retain their
deterministic tests; visual previews do not use real Telegram credentials.

Review completed on 2026-09-14 using the isolated native preview bundle. Both
900×600 and actual macOS full-screen covered lock/PIN access, the expanded proxy
form, General settings, the merged sync timeline, and account/transfer dialogs.
Cancel/wait feedback, form and inspector scrolling, and display-only status-bar
clicks were exercised. A four-column language layout that wrapped words in the
narrower panel was found during review and corrected to two columns.
