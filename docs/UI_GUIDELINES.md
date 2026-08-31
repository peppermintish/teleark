# UI Guidelines

Status: design contract for a persistent local Library plus real Telegram login/source selection/bounded indexing. The two supplied reference images are the visual source of truth. Transfer, encrypted recovery, and Vault routes remain preview-only and must not imply an end-to-end workflow that is not implemented.

## Product abstraction and branding

Use **TeleArk** consistently. Never reintroduce “Telegram Drive,” “Telegram Vault,” or “tdl GUI” as user-facing names. Every library row, detail panel, collection, and transfer task represents a `LogicalFile`. Multipart part names, message IDs, DC IDs, file references, and MTProto details appear only in intentionally advanced diagnostics.

No UI behavior may depend on Telegram Premium. The upload dialog shows one compatibility-oriented strategy, such as “Automatic multipart” and an exact “1900 MiB compatibility mode.” Do not show a Premium part-size option or imply premium speed/limits.

## Reference composition

The references establish two closely related desktop compositions:

- a light file-library window with grouped left navigation, title/search toolbar, dense file table, storage summary, and optional upload dialog;
- a transfer-focused window with sidebar, top search/status bar, summary cards, action toolbar, dense queue table, detail inspector, logs/connections panels, and queue statistics.

Additional required views reuse that visual language: file detail with overview/parts/details/activity tabs, key-vault settings, channel index detail, and a real non-contiguous index coverage map.

Treat the images as geometry targets, not loose inspiration. Measure layout from screenshots while accounting for source-image scale; do not mistake bitmap pixels for GPUI logical pixels. Preserve hierarchy, whitespace rhythm, table density, control size, borders, radii, progress thickness, tab treatment, badge shape, and restrained status colors.

## Application information architecture

```text
LIBRARY
  All Files, Recent, Videos, Documents, Archives, Images, Audio, Other

CHANNELS
  Saved Messages and indexed channels

COLLECTIONS
  Manual and Smart Collections

TRANSFERS
  All, Uploads, Downloads, Waiting, Completed, Failed

STORAGE
  Telegram Storage, Index, Key Vault

SETTINGS
```

Navigation labels use i18n message IDs. Dynamic channel/collection/file names preserve original source/user text.

## Design tokens

Centralize tokens in the GUI theme rather than scattering literal values:

```text
color.background.*, color.surface.*, color.text.*, color.border.*
color.accent.*, color.success.*, color.warning.*, color.danger.*
font.family, font.size.*, font.weight.*, line_height.*
space.1 ... space.n
radius.control, radius.card, radius.dialog
border.subtle, border.focus
shadow.window, shadow.dialog
size.sidebar, size.toolbar, size.row, size.control, size.icon
```

Light mode matching the supplied images is the first fidelity target. A future dark mode adds semantic token values; components never branch on raw RGB. Status is not conveyed by color alone—pair icon/text/badge state for accessibility.

Typography uses restrained size/weight contrast: prominent page title, compact section labels, regular table text, subdued secondary metadata, and tabular numerals where useful for sizes/rates/ETA. Do not shrink translations to preserve fixed widths.

## Reusable components

Build focused reusable controls where stock `gpui-component` output cannot match the references:

- grouped navigation section and count row;
- search field with shortcut hint;
- compact toolbar/segmented action buttons;
- metric summary card;
- virtualized file/transfer data table with selectable rows;
- semantic status badge and progress bar;
- filter chip and filter popover;
- inspector property grid and tab strip;
- modal upload dialog;
- storage summary and coverage visualization;
- empty/error/loading state.

Component APIs accept structured state and message IDs/parameters rather than final hard-coded strings. Business actions go through Core commands. Avoid one global view owning navigation, tables, dialogs, all mock data, and backend logic.

### Pinned GPUI API baseline

TeleArk targets the published `gpui 0.2.2` / `gpui-component 0.5.1` API, not the component website's unreleased `main` examples. For this release pair:

- start with `Application::new()`;
- register assets as needed and call `gpui_component::init(cx)` before component/window creation;
- wrap each window's first-level view in `Root` so overlays, focus, dialogs, and notifications work;
- use the virtualized `Table` + `TableState` + `TableDelegate` API (the current main branch calls the older component `DataTable` and defines a different `Table`);
- use `Sidebar::left()`/`Sidebar::right()`, not unreleased `Sidebar::new()` examples;
- verify dialog signatures against the `v0.5.1` source/examples rather than current-main docs.

Use semantic theme access/tokens and retain `Entity<InputState>`/other component state according to the pinned release's examples. An upgrade is an API, visual-regression, and license-review project; do not mix main-branch snippets into the pinned build.

## Tables and virtualization

Tables preserve dense, aligned columns and stable row heights from the references. Numeric columns align consistently; filenames receive flexible width and sensible truncation with accessible tooltip/full-detail access. Selection, hover, focus, keyboard navigation, sort indicator, and multi-selection states are distinct.

Use GPUI/gpui-component virtualization if its actual supported API is verified; otherwise implement a custom bounded visible-window list with keyset page prefetch. Never instantiate hundreds of thousands of rows. Deep scroll/search fetches Core pages, not SQL directly.

## Required screen behavior

### Library

Sidebar + toolbar/search + file table + footer/storage summary. The current alpha imports local metadata, searches SQLite/FTS5, filters every Core file kind, follows opaque cursors through Load More, and opens or reveals retained source paths through native platform actions. Empty/loading/error/preview states must not masquerade as “zero files.” Large-catalog virtualization and sortable columns remain required before claiming million-record UI readiness.

### Settings

Every Settings navigation item is connected to runtime-owned state in the
alpha. All three explicit locales switch live—including every input
placeholder—and persist, while System Default removes the override and
renegotiates. A build with neither a personal nor distributor API ID/API Hash
pair triggers a startup prompt that can be completed or skipped. The Settings
route identifies which source is active, can save or explicitly remove the
personal pair, and links through a real action to Telegram's official API
development panel. Personal values persist in the local SQLite Library
database with a visible local-data warning; distributor values are build-time
configuration. Long localized notices wrap inside their card at the supported
minimum width. Downloads always use the runtime-owned managed `Downloads`
directory without a per-file prompt and avoid overwriting an existing file.
The managed root and its `Downloads`, `Cache`, and `Logs` children are shown
together. Upload defaults,
Key Vault auto-lock, indexing batch size, in-app transfer notifications, and
light/dark/system appearance are typed, validated, and atomically persisted by
the frontend-neutral runtime. Upload defaults configure the current desktop
upload form; they do not imply that the preview encrypted-upload workflow is
production-ready.

### Upload dialog

Shows selected logical file, account/storage target, automatic multipart compatibility strategy, exact part-size unit, expected part count/total, and security options. Original-name hiding and encrypted metadata are understandable but not overclaimed. “Add to upload queue” creates a Core command only after backend integration; the mock milestone labels/isolates demo behavior.

### Transfers

Summary cards, queue controls/table, speed totals, logs/connections tabs, and inspector mirror the reference. State, verified parts, direction, speed, ETA, concurrency, retry, pause, and failure cause use real Core snapshots when integrated. A 100% byte bar is not “Completed” until verification succeeds.
The first table column supports row selection and select-all for the current
filtered result. Completed native downloads expose both Open and Show in
Finder actions. Tables and Telegram source/file panes own bounded internal
scroll regions so their last rows remain reachable without moving fixed
controls off screen.

Starting a Telegram download must not open a per-file destination dialog. The
Downloads settings page presents the single TeleArk managed-files root and its
derived `Downloads`, `Cache`, and `Logs` paths together. Users can choose the root,
restore the platform default, or open it in the system file manager. Existing
files are never overwritten; collision suffixes are assigned by the runtime.

The Storage settings page shows the active structured diagnostic-log location,
bounded-writer drop count, privacy disclosure, and an action to open the logs.
Runtime-backed transfer details show safe performance timings, average speed,
Trace ID, lifecycle events, verification state, and a localized failure reason
with action guidance. A transfer that fails before verification must say
verification was not reached rather than claiming integrity verification failed.
Running native downloads update transferred bytes, current speed, progress,
and ETA from real chunk events. Pause, resume, retry, and cancel controls invoke
runtime commands. Persisted rows remain visible after restart; interrupted work
stays queued until Telegram authorization is restored.

Each Telegram source offers bounded batch download controls for sent-time and
file-kind filters. One batch occupies one Transfers row; activating the row
expands its individual files without losing their independent controls or
durable checkpoints. Telegram file rows show sent time and a Unicode-safe
caption preview. Hover reveals the full caption, and selection opens a bounded,
scrollable detail panel with the full file name, message ID, sent time, MIME
type, declared size, and caption. Filenames and captions remain unmodified user
content. The per-file list is the primary source action and owns the available
vertical space. Batch filters start collapsed and expand on demand so they do
not displace direct per-file download controls.

The global sidebar footer shows live aggregate upload/download rates, available
space on the managed-files destination, and current TeleArk disk usage. Rates
must come from current transfer samples rather than completed-task averages.
When an upload owner is unavailable, the upload rate is truthfully zero rather
than a simulated estimate. Disk sizing runs outside the UI thread, is bounded,
and never follows symbolic links.

### File detail

Presents one logical file. Parts tab may expose application parts and remote locators for diagnostics but never makes users manage them. Distinguish uploaded, verifying, verified, remote missing, encrypted, and native states.

### Key Vault

Clearly distinguishes password unlock, OS credential convenience, Recovery Key backup, and irreversible key-loss warning. Never display secrets by default or include them in screenshots/logs. Prototype controls must not imply encryption exists before it does.

### Channel index detail

Shows examined messages, files/bytes indexed, latest sync, job progress/current date/rate/ETA, content policy, controls, and an `IndexRange`-backed coverage map with gaps/partial/complete intervals. Never derive “all indexed” from a single last-message ID.

Before authorization, the Telegram route presents phone/code/two-step login and
QR login together in one two-column composition at every supported width,
including the 900 px minimum. Fields within the phone panel use responsive
widths, but neither login method moves behind a tab or mode switch.
A primary Telegram sign-in action remains visible in the global header until an
account is authorized. When no complete personal or distributor credential pair is configured,
both login methods are visibly dimmed and non-interactive, the QR region shows
an on-brand non-scannable placeholder, and a prominent localized notice links
to the connected credential setting. A distributor build may provide
credentials registered for its own TeleArk application; the app never presents
shared Telegram Desktop credentials as an available fallback.

Normal macOS windows retain the native titlebar and traffic-light controls.
Full-screen mode must remain reversible through the top-edge AppKit controls,
the platform `Control-Command-F` shortcut, Escape, and an in-app exit control.
The desktop installs native, localized application, View, and Window menus;
AppKit owns the View menu's Enter/Exit Full Screen command and the standard
full-screen Space transition. This preserves the menu-bar and window-management
context. Custom titlebar styling must not replace those recovery paths.

## Internationalization and flexible layout

All visible/accessibility text resolves through the frontend-independent i18n layer. Support `en-US`, `zh-CN`, and `ja-JP`. Controls use flexible content widths, sensible minimums, wrapping where intended, and truncation only for secondary/filename regions with a recovery path to full text. Do not special-case widths by locale unless documented and truly necessary.

Dates, numbers, percentages, sizes, and rates use centralized formatters. Technical hashes/IDs remain canonical. Never concatenate sentence fragments or translate filenames, captions, paths, channel titles, or collection names. See `I18N.md`.

## Accessibility and interaction

- Every interactive element is keyboard reachable with visible focus.
- Dialogs trap/restore focus and support expected escape/confirm behavior without destructive surprises.
- Icons have localized accessible labels when meaningful; decorative icons are hidden from accessibility APIs.
- Text/status contrast and target sizes meet applicable platform guidance.
- Progress updates are throttled and announced without overwhelming assistive technology.
- Destructive actions name their scope and require proportionate confirmation/undo where feasible.
- Animations respect reduced-motion settings and never block completion/error feedback.

## Icons and third-party assets

Prefer a coherent icon set exposed by the selected component/assets package. The selected candidate asset package includes/derives from Lucide icons; preserve the Lucide ISC copyright/license attribution in distributed third-party notices and audit the exact packaged files. Do not assume every bundled asset shares Lucide's license, and do not ship third-party trademarks or incompatible assets without review.

## Screenshot verification

For every major visual change:

```text
implement -> run -> capture -> compare to reference -> adjust -> repeat
```

Record a reproducible window size, display scale, OS/theme, locale, and mock fixture. Compare overlays/diffs for layout geometry, text baselines, column widths, row height, spacing, controls, borders, radii, shadows, tabs, badges, and progress bars.

Smoke-test this matrix at minimum:

```text
Library, Upload, Transfers, File Detail, Key Vault,
Channel Index Detail, Settings
x en-US, zh-CN, ja-JP
```

Check clipped primary controls, overlaps, broken rows, missing focus, and misleading state. If automated GPUI snapshots are not practical, retain a documented manual capture checklist and attach comparison artifacts to review. Functional similarity without visual verification is not completion.

The current reproducible launch matrix uses `960x640`, `1360x760`, and
`1920x1080` for each of the seven routes and three locales (63 launches). Unit
tests additionally exercise the `900x600` supported minimum and breakpoint
budgets. Passing these checks establishes startup and layout-policy coverage,
not pixel fidelity. The window launcher fits oversized requests to the active
display before centering; the remaining captured comparison work covers the
full route/locale matrix and authenticated Telegram states.
