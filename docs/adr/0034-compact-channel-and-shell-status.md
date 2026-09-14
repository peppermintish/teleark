# ADR 0034: Compact channel controls and concise shell status

Status: Accepted, 2026-09-13. Extends ADR 0033 without restoring synchronization timers or navigation commands. The shell's user-requested active-channel count is distinct from the inspector, which retains fixed event timestamps and no elapsed counters.

## Decision

The channel page has one 48-point title row and one compact toolbar. File count and selection actions share that toolbar; selecting rows does not move the table. Filters expand on demand, and less frequent indexing stays inside them. Controls use the centralized 26-point compact button style. Earlier history is an intrinsic-width action at the right of a 32-point table footer.

A 28-point shell bar keeps concise background status at the left: completed, active distinct channels, queued, waiting, paused, connecting or needs attention. Only a completed snapshot with no known pending work gets the green Synced label; queued work is never counted as an executing channel. Active errors remain visible. Clicking the label opens the detailed synchronization/preparation inspector. A small lock icon and notice sit immediately beside it; the full explanation remains available as a tooltip. The former full-width lock banner is removed. Disk free space and aggregate download/upload rates align at the right.

Rate presentation reuses an account-scoped cache keyed by transfer snapshot revision and retained allocation identity, so unchanged renders do not rescan retained history. Unknown measurements display an em dash. There is no new animation timer, network activity, persistence change or schema/codec migration.

The inspector displays the fixed seven-minute channel-update silence recovery policy from Runtime's shared constant. Actual delivery postpones the deadline; ordinary tab navigation does not affect it. ADR 0033 describes the unchanged event/recovery ownership and unchanged-result suppression.

## References and verification

Reviewed MIT-licensed [shadcn/ui data-table guidance](https://ui.shadcn.com/docs/components/base/data-table), [Ant Design data display](https://ant.design/docs/spec/data-display/), and [GitHub Primer action-bar guidance](https://primer.style/product/components/action-bar/guidelines/) before designing the compact hierarchy. Original GPUI implementation uses the existing component kit and theme; no external implementation, imagery or dependency was imported.

Deterministic tests cover status truth and distinct-channel counts, queued/failed/account activity, rate-cache operation counts and account replacement, compact footer geometry, and a stable table position across selection. Runtime tests exercise the seven-minute deadline and its postponement by actual delivery. Native English review covers 900×600 and actual full-screen mode in light/dark themes, with reachable selection/history controls and independent inspector scrolling. Executed results are recorded in Implementation Status.

[ADR 0037](0037-application-pin-and-transfer-drain.md) supersedes the clickable sync status and application lock indicator: sync status is display-only, and an independent optional PIN gates the whole application.
