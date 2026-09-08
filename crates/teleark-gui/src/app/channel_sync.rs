//! Local channel projection and synchronization presentation. No network scan
//! is owned by a selected view; navigating away only cancels its local read.
use super::*;
use gpui_kit::component::button::ButtonVariants as _;
use teleark_i18n::format::{format_duration_millis, format_integer};
use teleark_runtime::ChannelSyncPhase;

const VIEW_CACHE_BYTES: usize = 16 * 1024 * 1024;

impl TeleArkApp {
    pub(crate) fn remember_channel_view(&mut self) {
        let (Some(account), Some(chat)) = (self.telegram_account.as_ref(), self.selected_chat_id)
        else {
            return;
        };
        self.channel_view_cache
            .retain(|(a, c, _)| (*a, *c) != (account.id, chat));
        self.channel_view_cache
            .push_back((account.id, chat, self.telegram_files.clone()));
        while self.channel_view_cache.len() > 8
            || self
                .channel_view_cache
                .iter()
                .flat_map(|(_, _, files)| files)
                .map(|file| {
                    file.file_name.len()
                        + file.caption.len()
                        + file.mime_type.as_ref().map_or(0, String::len)
                        + 128
                })
                .sum::<usize>()
                > VIEW_CACHE_BYTES
        {
            self.channel_view_cache.pop_front();
        }
    }

    pub(crate) fn restore_channel_view(&mut self, chat: i64) {
        let Some(account) = &self.telegram_account else {
            return;
        };
        if let Some((_, _, files)) = self
            .channel_view_cache
            .iter()
            .find(|(a, c, _)| (*a, *c) == (account.id, chat))
        {
            self.telegram_files = files.clone();
        }
        self.channel_display_revision = 0;
        self.channel_local_read_failed = false;
    }

    pub(crate) fn start_channel_sync(&mut self, cx: &mut Context<Self>) {
        if self.visual_preview {
            return;
        }
        if let Some(sync) = &self.channel_sync {
            if let Err(error) = sync.update_sources(self.telegram_chats.clone()) {
                self.telegram_activity = TelegramActivity::Failed(error.kind());
            }
            return;
        }
        let (Some(telegram), Some(library), Some(account)) = (
            self.telegram.clone(),
            self.library.clone(),
            self.telegram_account.clone(),
        ) else {
            return;
        };
        let account_id = account.id;
        let mut subscription = match teleark_runtime::ChannelSync::start(
            telegram,
            library,
            account,
            self.telegram_chats.clone(),
        ) {
            Ok(sync) => {
                let subscription = sync.subscribe();
                self.channel_sync_snapshot = sync.snapshot().ok();
                self.channel_sync = Some(sync);
                subscription
            }
            Err(error) => {
                self.telegram_activity = TelegramActivity::Failed(error.kind());
                cx.notify();
                return;
            }
        };
        self.channel_sync_task = Some(cx.spawn(async move |this, cx| {
            loop {
                let Some(entity) = this.upgrade() else { break };
                let keep = entity.update(cx, |app, cx| {
                    if app.telegram_account.as_ref().map(|a| a.id) != Some(account_id) {
                        return false;
                    }
                    let Some(sync) = &app.channel_sync else {
                        return false;
                    };
                    match sync.snapshot() {
                        Ok(snapshot) => app.channel_sync_snapshot = Some(snapshot),
                        Err(error) => {
                            app.telegram_activity = TelegramActivity::Failed(error.kind())
                        }
                    }
                    app.apply_channel_changes(cx);
                    app.apply_managed_channel_changes(cx);
                    app.start_channel_sync_clock(cx);
                    cx.notify();
                    true
                });
                drop(entity);
                if !keep || !subscription.changed().await {
                    break;
                }
            }
        }));
        cx.notify();
    }

    pub(crate) fn read_local_channel(&mut self, cx: &mut Context<Self>) {
        if self.telegram_files_loading {
            return;
        }
        let (Some(library), Some(account), Some(chat)) = (
            self.library.clone(),
            self.telegram_account.as_ref(),
            self.selected_chat_id,
        ) else {
            return;
        };
        let account = account.id;
        let generation = self.telegram_file_generation;
        self.telegram_file_auto_load = false;
        self.telegram_files_loading = true;
        self.telegram_activity = TelegramActivity::Idle;
        self.refresh_channel_file_table(cx);
        cx.notify();
        let work = cx.background_spawn(async move {
            library.cached_channel_view(account, chat, CHANNEL_FILE_LIST_CAPACITY)
        });
        self.telegram_file_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let Some(entity) = this.upgrade() else { return };
            entity.update(cx, |app, cx| {
                if app.telegram_account.as_ref().map(|a| a.id) != Some(account)
                    || app.selected_chat_id != Some(chat)
                    || app.telegram_file_generation != generation
                {
                    return;
                }
                app.telegram_files_loading = false;
                match result {
                    Ok(view) => {
                        app.channel_local_read_failed = false;
                        app.telegram_files = view.files;
                        app.telegram_files_exhausted = view.history_exhausted
                            || app.telegram_files.len() >= CHANNEL_FILE_LIST_CAPACITY;
                        app.channel_display_revision = view.revision;
                        // Keep selection and scroll owner while applying the local projection.
                        app.selected_channel_message_ids
                            .retain(|id| app.telegram_files.iter().any(|f| f.message_id == *id));
                        if app.selected_telegram_message_id.is_some_and(|id| {
                            !app.telegram_files.iter().any(|f| f.message_id == id)
                        }) {
                            app.selected_telegram_message_id = None;
                            app.show_channel_detail = false;
                        }
                        app.remember_channel_view();
                    }
                    Err(error) => {
                        app.channel_local_read_failed = true;
                        app.telegram_activity = TelegramActivity::Failed(error.kind());
                    }
                }
                app.refresh_channel_file_table(cx);
                app.apply_channel_changes(cx);
                cx.notify();
            });
        }));
    }

    fn apply_channel_changes(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.page, Page::Channel | Page::Storage)
            || self.storage_view != StorageView::RawFiles
            || self.telegram_files_loading
            || self.channel_local_read_failed
        {
            return;
        }
        let (Some(sync), Some(chat)) = (&self.channel_sync, self.selected_chat_id) else {
            return;
        };
        let changes = match sync.changes_since(chat, self.channel_display_revision) {
            Ok(changes) => changes,
            Err(error) => {
                self.telegram_activity = TelegramActivity::Failed(error.kind());
                return;
            }
        };
        self.apply_channel_change_batch(changes, cx);
    }

    fn apply_channel_change_batch(
        &mut self,
        changes: teleark_runtime::ChannelChanges,
        cx: &mut Context<Self>,
    ) {
        if changes.reset_required {
            self.read_local_channel(cx);
            return;
        }
        self.channel_display_revision = changes.revision;
        if changes.deltas.is_empty() {
            return;
        }
        let rows_changed = patch_channel_rows(&mut self.telegram_files, &changes.deltas);
        let exhausted = changes
            .deltas
            .last()
            .is_some_and(|delta| delta.history_exhausted)
            || self.telegram_files.len() >= CHANNEL_FILE_LIST_CAPACITY;
        let coverage_changed = self.telegram_files_exhausted != exhausted;
        self.telegram_files_exhausted = exhausted;
        if !rows_changed && !coverage_changed {
            return;
        }
        let visible: BTreeSet<_> = self
            .telegram_files
            .iter()
            .map(|file| file.message_id)
            .collect();
        self.selected_channel_message_ids
            .retain(|id| visible.contains(id));
        if self
            .selected_telegram_message_id
            .is_some_and(|id| !visible.contains(&id))
        {
            self.selected_telegram_message_id = None;
            self.show_channel_detail = false;
        }
        self.refresh_channel_file_table(cx);
    }

    pub(super) fn apply_managed_channel_changes(&mut self, cx: &mut Context<Self>) {
        if self.page != Page::Storage
            || self.storage_view != StorageView::Files
            || self.vault_locked
            || self.managed_scan_loading
        {
            return;
        }
        let (Some(sync), Some(chat)) = (&self.channel_sync, self.storage_channel_id()) else {
            return;
        };
        let changes = match sync.changes_since(chat, self.managed_display_revision) {
            Ok(changes) => changes,
            Err(error) => {
                self.vault_activity = VaultActivity::Failed(error.kind());
                return;
            }
        };
        if changes.reset_required {
            self.managed_upload_receipts.clear();
        }
        for delta in &changes.deltas {
            self.managed_upload_receipts.retain(|(_, _, file)| {
                !delta.removed.contains(&file.manifest_message_id)
                    && !delta
                        .upserted
                        .iter()
                        .any(|seen| seen.message_id == file.manifest_message_id)
            });
        }
        if changes.reset_required
            || changes
                .deltas
                .iter()
                .any(|delta| delta.managed_files_changed)
        {
            self.scan_managed_vault_files(cx);
        } else {
            self.managed_display_revision = changes.revision;
        }
    }

    pub(super) fn managed_change_warning(&self) -> Option<SharedString> {
        let snapshot = self.channel_sync_snapshot.as_ref()?;
        if snapshot.managed_chat_id != self.storage_channel_id() {
            return None;
        }
        if snapshot.managed_review_pending {
            return Some(self.tr("managed-watch-pending"));
        }
        let count = snapshot.managed_watch.as_ref()?.unacknowledged();
        (count > 0).then(|| {
            self.tr_with(
                "managed-watch-changed",
                MessageArgs::new().with("count", format_integer(self.locale(), count)),
            )
        })
    }

    fn sync_clock_needed(&self) -> bool {
        self.channel_sync_snapshot.as_ref().is_some_and(|snapshot| {
            !matches!(
                snapshot.phase,
                ChannelSyncPhase::Idle | ChannelSyncPhase::Failed | ChannelSyncPhase::Cancelled
            ) || snapshot
                .managed_scan
                .as_ref()
                .is_some_and(|scan| scan.active())
        })
    }

    fn start_channel_sync_clock(&mut self, cx: &mut Context<Self>) {
        if self.channel_sync_clock_task.is_some() || !self.sync_clock_needed() {
            return;
        }
        self.channel_sync_clock_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let Some(entity) = this.upgrade() else {
                    break;
                };
                if !entity.update(cx, |app, cx| {
                    if !app.sync_clock_needed() {
                        app.channel_sync_clock_task = None;
                        return false;
                    }
                    cx.notify();
                    true
                }) {
                    break;
                }
            }
        }));
    }

    pub(crate) fn request_channel_history(&mut self, cx: &mut Context<Self>) {
        if self.telegram_files_exhausted {
            return;
        }
        if let (Some(sync), Some(chat)) = (&self.channel_sync, self.selected_chat_id)
            && let Err(error) = sync.request_history(chat)
        {
            self.telegram_activity = TelegramActivity::Failed(error.kind());
        }
        cx.notify();
    }

    pub(crate) fn refresh_selected_channel(&mut self, cx: &mut Context<Self>) {
        self.channel_local_read_failed = false;
        if !matches!(self.page, Page::Channel | Page::Storage) {
            self.load_selected_telegram_files(false, cx);
            return;
        }
        if self
            .channel_sync
            .as_ref()
            .is_some_and(|sync| sync.is_stopped())
        {
            self.channel_sync = None;
            self.channel_sync_task = None;
            self.start_channel_sync(cx);
        }
        if let (Some(sync), Some(chat)) = (&self.channel_sync, self.selected_chat_id)
            && let Err(error) = sync.refresh(chat)
        {
            self.telegram_activity = TelegramActivity::Failed(error.kind());
        }
        self.read_local_channel(cx);
    }

    pub(crate) fn channel_sync_label(&self) -> SharedString {
        if let Some(scan) = self
            .channel_sync_snapshot
            .as_ref()
            .and_then(|s| s.managed_scan.as_ref())
            .filter(|scan| scan.active())
        {
            return self.tr_with(
                "managed-scan-progress",
                MessageArgs::new()
                    .with("phase", self.tr(sync_phase_id(scan.phase)).to_string())
                    .with("done", format_integer(self.locale(), scan.completed as u64))
                    .with(
                        "total",
                        scan.total.map_or_else(
                            || self.tr("managed-scan-unknown").to_string(),
                            |total| format_integer(self.locale(), total as u64),
                        ),
                    ),
            );
        }

        let Some(snapshot) = &self.channel_sync_snapshot else {
            return self.tr("channel-sync-local-only");
        };
        let phase = self.tr(sync_phase_id(snapshot.phase));
        self.tr_with(
            "channel-sync-status",
            MessageArgs::new().with("phase", phase.to_string()).with(
                "queued",
                format_integer(self.locale(), snapshot.queued as u64),
            ),
        )
    }

    pub(crate) fn channel_sync_timing(&self) -> SharedString {
        let Some(snapshot) = &self.channel_sync_snapshot else {
            return self.tr("channel-sync-local-only");
        };
        let (started, activity) = snapshot
            .managed_scan
            .as_ref()
            .filter(|scan| scan.active())
            .map_or((snapshot.phase_started, snapshot.last_activity), |scan| {
                (scan.phase_started, scan.last_activity)
            });
        self.tr_with(
            "channel-sync-timing",
            MessageArgs::new()
                .with("duration", sync_elapsed(self.locale(), started))
                .with("activity", sync_elapsed(self.locale(), activity)),
        )
    }

    pub(crate) fn render_channel_sync(&self, cx: &mut Context<Self>) -> AnyElement {
        let header = div()
            .flex()
            .min_w_0()
            .items_center()
            .gap_2()
            .child(
                components::button(
                    "channel-sync-details",
                    self.tr("channel-sync-details-title"),
                    None,
                    false,
                )
                .ghost()
                .min_w_0()
                .flex_1()
                .justify_start()
                .overflow_hidden()
                .debug_selector(|| "channel-sync-details".into())
                .tooltip(self.channel_sync_label())
                .on_click(cx.listener(|app, _, _, cx| {
                    app.channel_sync_details = !app.channel_sync_details;
                    app.show_channel_detail = false;
                    cx.notify();
                })),
            )
            .when(!self.telegram_files_exhausted, |row| {
                row.child(
                    components::button(
                        "channel-sync-history",
                        self.tr("channel-sync-history"),
                        None,
                        false,
                    )
                    .flex_none()
                    .debug_selector(|| "channel-sync-history".into())
                    .on_click(cx.listener(|app, _, _, cx| app.request_channel_history(cx))),
                )
            })
            .child(
                components::icon_button(
                    "channel-sync-retry",
                    IconName::Redo2,
                    self.tr("telegram-files-retry-action"),
                )
                .ghost()
                .debug_selector(|| "channel-sync-retry".into())
                .on_click(cx.listener(|app, _, _, cx| app.refresh_selected_channel(cx))),
            )
            .child(
                components::icon_button(
                    "channel-sync-cancel",
                    IconName::Close,
                    self.tr("telegram-files-cancel-action"),
                )
                .ghost()
                .debug_selector(|| "channel-sync-cancel".into())
                .on_click(cx.listener(|app, _, _, cx| {
                    if let (Some(sync), Some(chat)) = (&app.channel_sync, app.selected_chat_id)
                        && let Err(error) = sync.cancel(chat)
                    {
                        app.telegram_activity = TelegramActivity::Failed(error.kind());
                    }
                    cx.notify();
                })),
            );
        div()
            .debug_selector(|| "channel-sync-footer".into())
            .flex_none()
            .p_2()
            .border_t_1()
            .border_color(theme::border())
            .text_xs()
            .text_color(theme::text_secondary())
            .child(header)
            .into_any_element()
    }

    pub(crate) fn render_channel_sync_details(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut body = div()
            .flex()
            .flex_col()
            .gap_3()
            .p_3()
            .text_xs()
            .text_color(theme::text_secondary())
            .child(self.channel_sync_label())
            .child(self.channel_sync_timing());
        if let Some(snapshot) = &self.channel_sync_snapshot {
            if let Some(scan) = &snapshot.managed_scan {
                body = body.child(
                    self.tr_with(
                        "managed-scan-detail",
                        MessageArgs::new()
                            .with("phase", self.tr(sync_phase_id(scan.phase)).to_string())
                            .with("done", format_integer(self.locale(), scan.completed as u64))
                            .with(
                                "total",
                                scan.total.map_or_else(
                                    || self.tr("managed-scan-unknown").to_string(),
                                    |total| format_integer(self.locale(), total as u64),
                                ),
                            )
                            .with("cached", format_integer(self.locale(), scan.cached as u64))
                            .with(
                                "rejected",
                                format_integer(self.locale(), scan.rejected as u64),
                            )
                            .with("duration", sync_elapsed(self.locale(), scan.phase_started))
                            .with("activity", sync_elapsed(self.locale(), scan.last_activity)),
                    ),
                );
                if let Some(failure) = scan.failure {
                    body = body.child(self.application_error_message(failure));
                }
                if scan.active() {
                    body = body.child(
                        components::button(
                            "managed-scan-cancel",
                            self.tr("telegram-files-cancel-action"),
                            None,
                            false,
                        )
                        .on_click(cx.listener(|app, _, _, cx| {
                            app.cancel_managed_scan();
                            cx.notify();
                        })),
                    );
                }
            }
            if let Some(failure) = snapshot.failure {
                body = body.child(self.application_error_message(failure));
            }
            if let Some(at) = snapshot.retry_at {
                body = body.child(
                    self.tr_with(
                        "channel-sync-retry-after",
                        MessageArgs::new().with(
                            "duration",
                            format_duration_millis(
                                self.locale(),
                                u64::try_from(
                                    at.saturating_duration_since(std::time::Instant::now())
                                        .as_millis(),
                                )
                                .unwrap_or(u64::MAX),
                            ),
                        ),
                    ),
                );
            }
            if let Some(chat) = snapshot.chat_id.or(snapshot.managed_chat_id) {
                body = body.child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            components::button(
                                "global-sync-retry",
                                self.tr("telegram-files-retry-action"),
                                None,
                                false,
                            )
                            .on_click(cx.listener(
                                move |app, _, _, cx| {
                                    if let Some(sync) = &app.channel_sync
                                        && let Err(error) = sync.refresh(chat)
                                    {
                                        app.telegram_activity =
                                            TelegramActivity::Failed(error.kind());
                                    }
                                    cx.notify();
                                },
                            )),
                        )
                        .child(
                            components::button(
                                "global-sync-cancel",
                                self.tr("telegram-files-cancel-action"),
                                None,
                                false,
                            )
                            .on_click(cx.listener(
                                move |app, _, _, cx| {
                                    if let Some(sync) = &app.channel_sync
                                        && let Err(error) = sync.cancel(chat)
                                    {
                                        app.telegram_activity =
                                            TelegramActivity::Failed(error.kind());
                                    }
                                    cx.notify();
                                },
                            )),
                        ),
                );
            }
            if let Some(chat) = snapshot
                .managed_chat_id
                .filter(|id| Some(*id) == self.storage_channel_id())
            {
                body = body.child(self.tr("managed-watch-title"));
                if snapshot.managed_review_pending {
                    body = body.child(self.tr("managed-watch-pending"));
                }
                if let Some(watch) = &snapshot.managed_watch {
                    let through = watch.change_count;
                    if watch.unacknowledged() > 0 {
                        body = body.child(
                            components::button(
                                "managed-watch-acknowledge",
                                self.tr("managed-watch-acknowledge"),
                                None,
                                false,
                            )
                            .debug_selector(|| "managed-watch-acknowledge".into())
                            .on_click(cx.listener(
                                move |app, _, _, cx| {
                                    if let Some(sync) = &app.channel_sync
                                        && let Err(error) =
                                            sync.acknowledge_managed_changes(chat, through)
                                    {
                                        app.telegram_activity =
                                            TelegramActivity::Failed(error.kind());
                                    }
                                    cx.notify();
                                },
                            )),
                        );
                    }
                    body = body.child(self.tr_with(
                        "managed-watch-retention",
                        MessageArgs::new().with(
                            "omitted",
                            format_integer(self.locale(), watch.omitted_changes()),
                        ),
                    ));
                    for change in &watch.changes {
                        let kind = match change.kind {
                            teleark_runtime::ManagedChannelChangeKind::Edited => {
                                "managed-watch-edited"
                            }
                            teleark_runtime::ManagedChannelChangeKind::Deleted => {
                                "managed-watch-deleted"
                            }
                            teleark_runtime::ManagedChannelChangeKind::Gap => "managed-watch-gap",
                        };
                        body = body.child(
                            self.tr_with(
                                if change.message_id > 0 {
                                    "managed-watch-event"
                                } else {
                                    "managed-watch-gap-event"
                                },
                                MessageArgs::new()
                                    .with("kind", self.tr(kind).to_string())
                                    .with(
                                        "message",
                                        format_integer(self.locale(), change.message_id as u64),
                                    )
                                    .with(
                                        "time",
                                        teleark_i18n::format::format_unix_millis(
                                            self.locale(),
                                            change.observed_at_unix_ms,
                                        ),
                                    ),
                            ),
                        );
                    }
                }
            }
            body = body
                .children(snapshot.events.iter().rev().map(|event| {
                    let mut row = div().flex().flex_col().gap_1().child(
                        self.tr_with(
                            "channel-sync-event",
                            MessageArgs::new()
                                .with("phase", self.tr(sync_phase_id(event.phase)).to_string())
                                .with("age", sync_elapsed(self.locale(), event.at)),
                        ),
                    );
                    if let Some(chat) = event
                        .chat_id
                        .and_then(|id| self.telegram_chats.iter().find(|chat| chat.id == id))
                    {
                        row = row.child(chat.name.clone());
                    }
                    if let Some(failure) = event.failure {
                        row = row.child(self.application_error_message(failure));
                    }
                    row
                }))
                .child(
                    self.tr_with(
                        "channel-sync-retention",
                        MessageArgs::new()
                            .with(
                                "dropped",
                                format_integer(self.locale(), snapshot.dropped_events),
                            )
                            .with(
                                "overflow",
                                format_integer(self.locale(), snapshot.overflow_signals),
                            ),
                    ),
                );
        }
        components::inspector_panel("channel-sync-inspector", 320.0)
            .debug_selector(|| "channel-sync-inspector".into())
            .absolute()
            .right_0()
            .top_0()
            .bottom(px(28.0))
            .h_auto()
            .shadow_lg()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .p_2()
                    .child(self.tr("channel-sync-details-title"))
                    .child(
                        components::icon_button(
                            "channel-sync-close-details",
                            IconName::Close,
                            self.tr("action-close-details"),
                        )
                        .ghost()
                        .debug_selector(|| "channel-sync-close-details".into())
                        .on_click(cx.listener(|app, _, _, cx| {
                            app.channel_sync_details = false;
                            cx.notify();
                        })),
                    ),
            )
            .child(components::inspector_body(
                "channel-sync-events",
                &self.channel_sync_scroll,
                body,
            ))
            .into_any_element()
    }
}

/// Coalesce a burst by message ID and touch each existing row once. Rename-only
/// edits reuse the current order; dates/new messages trigger one bounded sort.
fn patch_channel_rows(
    files: &mut Vec<TelegramFileSummary>,
    deltas: &[std::sync::Arc<teleark_runtime::ChannelDelta>],
) -> bool {
    let mut changes = std::collections::BTreeMap::new();
    for delta in deltas {
        for id in &delta.removed {
            changes.insert(*id, None);
        }
        for file in &delta.upserted {
            changes.insert(file.message_id, Some(file));
        }
    }
    let mut changed = false;
    let mut reorder = false;
    files.retain_mut(|file| match changes.remove(&file.message_id) {
        Some(Some(new)) => {
            if file != new {
                reorder |= file.sent_at_unix_ms != new.sent_at_unix_ms;
                *file = new.clone();
                changed = true;
            }
            true
        }
        Some(None) => {
            changed = true;
            false
        }
        None => true,
    });
    for file in changes.into_values().flatten() {
        files.push(file.clone());
        changed = true;
        reorder = true;
    }
    if reorder {
        files.sort_by(|a, b| {
            b.sent_at_unix_ms
                .cmp(&a.sent_at_unix_ms)
                .then_with(|| b.message_id.cmp(&a.message_id))
        });
    }
    files.truncate(CHANNEL_FILE_LIST_CAPACITY);
    changed
}

fn sync_elapsed(locale: SupportedLocale, instant: std::time::Instant) -> String {
    format_duration_millis(
        locale,
        u64::try_from(instant.elapsed().as_millis()).unwrap_or(u64::MAX),
    )
}
fn sync_phase_id(phase: ChannelSyncPhase) -> &'static str {
    match phase {
        ChannelSyncPhase::ManifestQueued => "managed-scan-queued",
        ChannelSyncPhase::ManifestReading => "managed-scan-reading",
        ChannelSyncPhase::ManifestReceiving => "managed-scan-receiving",
        ChannelSyncPhase::ManifestVerifying => "managed-scan-verifying",
        ChannelSyncPhase::ManifestCompleted => "managed-scan-completed",
        ChannelSyncPhase::ManifestFailed => "managed-scan-failed",
        ChannelSyncPhase::ManifestCancelled => "managed-scan-cancelled",
        ChannelSyncPhase::Queued => "channel-sync-queued",
        ChannelSyncPhase::Seeding => "channel-sync-seeding",
        ChannelSyncPhase::History => "channel-sync-history-loading",
        ChannelSyncPhase::ReadingLocal => "channel-sync-reading",
        ChannelSyncPhase::Receiving => "channel-sync-receiving",
        ChannelSyncPhase::Persisting => "channel-sync-persisting",
        ChannelSyncPhase::Verifying => "channel-sync-verifying",
        ChannelSyncPhase::Waiting => "channel-sync-waiting",
        ChannelSyncPhase::RateLimited => "channel-sync-rate-limited",
        ChannelSyncPhase::Idle => "channel-sync-idle",
        ChannelSyncPhase::Failed => "channel-sync-failed",
        ChannelSyncPhase::Cancelled => "channel-sync-cancelled",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit as gpui;
    use gpui_kit::{TestAppContext, component::table::TableDelegate as _};

    #[gpui::test]
    fn compact_sync_controls_and_independent_timeline_work_in_every_locale(
        cx: &mut TestAppContext,
    ) {
        use teleark_runtime::{ChannelSyncEvent, ChannelSyncSnapshot};
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Channel);
        cx.simulate_resize(gpui::size(px(900.0), px(600.0)));
        for locale in [
            SupportedLocale::EnUs,
            SupportedLocale::ZhCn,
            SupportedLocale::JaJp,
        ] {
            cx.update(|window, cx| {
                app.update(cx, |app, cx| {
                    app.localizer = Localizer::new(locale).expect("catalog");
                    theme::apply_appearance(
                        if locale == SupportedLocale::JaJp {
                            AppearancePreference::Dark
                        } else {
                            AppearancePreference::Light
                        },
                        window,
                        cx,
                    );
                    app.page = Page::Channel;
                    app.storage_view = StorageView::RawFiles;
                    app.selected_chat_id = Some(1001);
                    app.channel_sync_details = false;
                    app.telegram_files_exhausted = false;
                    let now = std::time::Instant::now();
                    app.channel_sync_snapshot = Some(ChannelSyncSnapshot {
                        account_id: 1,
                        phase: ChannelSyncPhase::RateLimited,
                        chat_id: Some(1001),
                        phase_started: now,
                        last_activity: now,
                        retry_at: Some(now + Duration::from_secs(30)),
                        failure: None,
                        queued: 3,
                        failed_channels: 0,
                        committed_pages: 4,
                        data_revision: 4,
                        events: (0..128)
                            .map(|_| ChannelSyncEvent {
                                phase: ChannelSyncPhase::Receiving,
                                chat_id: Some(1001),
                                at: now,
                                failure: None,
                            })
                            .collect(),
                        dropped_events: 0,
                        overflow_signals: 0,
                        managed_chat_id: None,
                        managed_watch: None,
                        managed_review_pending: false,
                        managed_scan: None,
                    });
                    app.refresh_channel_file_table(cx);
                    cx.notify();
                })
            });
            cx.run_until_parked();
            let footer = cx.debug_bounds("channel-sync-footer").expect("footer");
            assert!(
                footer.size.height < px(125.0),
                "timeline must not consume the list"
            );
            for selector in [
                "channel-sync-history",
                "channel-sync-retry",
                "channel-sync-cancel",
            ] {
                let bounds = cx.debug_bounds(selector).expect("reachable action");
                assert!(bounds.right() <= px(900.0) && bounds.bottom() <= px(600.0));
            }
            let details = cx
                .debug_bounds("channel-sync-details")
                .expect("details toggle")
                .center();
            cx.simulate_click(details, gpui::Modifiers::default());
            cx.run_until_parked();
            let inspector = cx
                .debug_bounds("channel-sync-inspector")
                .expect("separate inspector");
            cx.simulate_event(gpui::ScrollWheelEvent {
                position: inspector.center(),
                delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.0), px(-120.0))),
                ..Default::default()
            });
            app.update(cx, |app, _| {
                assert!(app.channel_sync_scroll.offset().y < px(0.0))
            });
            let close = cx
                .debug_bounds("channel-sync-close-details")
                .expect("close")
                .center();
            cx.simulate_click(close, gpui::Modifiers::default());
            cx.run_until_parked();
            assert!(cx.debug_bounds("channel-sync-inspector").is_none());
        }
    }

    #[gpui::test]
    fn channel_selection_reads_local_without_network_and_rejects_stale_results(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let (app, cx) = cx.add_window_view(|window, cx| {
            let unavailable =
                || ApplicationError::new(teleark_core::ApplicationErrorKind::Authorization);
            TeleArkApp::new(
                window,
                cx,
                Localizer::new(SupportedLocale::EnUs).expect("catalog"),
                RuntimeStartup {
                    configuration: None,
                    library: Err(unavailable()),
                    telegram: Err(unavailable()),
                    transfers: Err(unavailable()),
                    vault: Err(unavailable()),
                },
                AppStartup {
                    page: Page::Account,
                    visual_preview: false,
                    show_upload: false,
                    locale: LocaleStartup {
                        system_locale: SupportedLocale::EnUs,
                        follows_system_locale: false,
                    },
                },
            )
        });
        let directory = std::env::temp_dir().join(format!(
            "teleark-local-channel-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).expect("fixture directory");
        let library =
            DesktopLibrary::open(directory.join("catalog.sqlite3")).expect("fixture database");
        let account = TelegramAccount {
            id: 1,
            display_name: "Fixture".into(),
            username: None,
        };
        let chats = [10, 20].map(|id| TelegramChatSummary {
            id,
            name: format!("Fixture {id}"),
            username: None,
            kind: TelegramChatKind::Channel,
            sync_pts: Some(50),
        });
        library
            .save_telegram_sources(&account, &chats)
            .expect("sources");
        let file = |id| TelegramFileSummary {
            message_id: id,
            sent_at_unix_ms: 1_000,
            modified_at_unix_ms: 1_000,
            file_name: format!("{id}.pdf"),
            caption: String::new(),
            mime_type: None,
            size_bytes: 42,
        };
        library
            .cache_telegram_files(1, 10, &[file(7)])
            .expect("first cache");
        library
            .cache_telegram_files(1, 20, &[file(8)])
            .expect("second cache");
        app.update(cx, |app, cx| {
            app.library = Some(library.clone());
            app.telegram_account = Some(account);
            app.telegram_chats = chats.to_vec();
            app.select_channel(10, cx);
            assert!(app.telegram.is_none());
            assert!(app.telegram_files_loading);
        });
        cx.run_until_parked();
        app.update(cx, |app, cx| {
            assert_eq!(app.telegram_files, vec![file(7)]);
            assert!(!app.telegram_files_loading);
            assert!(!app.channel_file_table.read(cx).delegate().has_more(cx)); // no implicit history fetch
            app.select_channel(20, cx);
        });
        cx.run_until_parked();
        app.update(cx, |app, cx| {
            assert_eq!(app.telegram_files, vec![file(8)]);
            app.select_channel(10, cx);
            assert_eq!(app.telegram_files, vec![file(7)]); // immediate warm view
            app.select_channel(20, cx);
            assert_eq!(app.telegram_files, vec![file(8)]);
        });
        cx.run_until_parked();
        app.update(cx, |app, cx| {
            assert_eq!(app.selected_chat_id, Some(20));
            assert_eq!(app.telegram_files, vec![file(8)]);
            app.select_channel(10, cx);
            app.cancel_telegram_file_load(cx);
            app.telegram_account.as_mut().expect("account").id = 2;
            app.telegram_files.clear();
            app.restore_channel_view(10);
            assert!(app.telegram_files.is_empty());
        });
        cx.run_until_parked();
        app.update(cx, |app, _| {
            assert!(app.telegram_files.is_empty());
            app.library = None;
            app.telegram_file_task = None;
        });
        drop(library);
        std::fs::remove_dir_all(directory).expect("remove fixture");
    }
    fn fixture_snapshot() -> teleark_runtime::ChannelSyncSnapshot {
        let now = std::time::Instant::now();
        teleark_runtime::ChannelSyncSnapshot {
            account_id: 1,
            phase: ChannelSyncPhase::RateLimited,
            chat_id: Some(9000),
            phase_started: now,
            last_activity: now,
            retry_at: Some(now + Duration::from_secs(30)),
            failure: None,
            queued: 3,
            failed_channels: 0,
            committed_pages: 2,
            data_revision: 2,
            events: (0..128)
                .map(|_| teleark_runtime::ChannelSyncEvent {
                    phase: ChannelSyncPhase::Receiving,
                    chat_id: Some(9000),
                    at: now,
                    failure: None,
                })
                .collect(),
            dropped_events: 4,
            overflow_signals: 1,
            managed_chat_id: Some(9000),
            managed_review_pending: false,
            managed_scan: None,
            managed_watch: Some(teleark_runtime::ManagedChannelWatch {
                catalog_ready: true,
                change_count: 2,
                acknowledged_count: 0,
                last_changed_at_unix_ms: Some(1_000),
                changes: vec![teleark_runtime::ManagedChannelChange {
                    sequence: 2,
                    message_id: 10,
                    kind: teleark_runtime::ManagedChannelChangeKind::Deleted,
                    observed_at_unix_ms: 1_000,
                }],
            }),
        }
    }

    #[gpui::test]
    fn global_status_and_locked_private_warning_stay_at_window_bottom_on_every_page(
        cx: &mut TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Storage);
        cx.simulate_resize(gpui::size(px(900.0), px(600.0)));
        for locale in [
            SupportedLocale::EnUs,
            SupportedLocale::ZhCn,
            SupportedLocale::JaJp,
        ] {
            for dark in [false, true] {
                for page in [
                    Page::Account,
                    Page::Channel,
                    Page::Storage,
                    Page::LegacyRecovery,
                    Page::Library,
                    Page::Transfers,
                    Page::Settings,
                ] {
                    cx.update(|window, cx| {
                        app.update(cx, |app, cx| {
                            app.localizer = Localizer::new(locale).expect("catalog");
                            theme::apply_appearance(
                                if dark {
                                    AppearancePreference::Dark
                                } else {
                                    AppearancePreference::Light
                                },
                                window,
                                cx,
                            );
                            app.page = page;
                            app.vault_locked = true;
                            app.channel_sync_details = false;
                            app.channel_sync_snapshot = Some(fixture_snapshot());
                            cx.notify();
                        })
                    });
                    cx.run_until_parked();
                    let bar = cx
                        .debug_bounds("global-background-status")
                        .expect("global footer");
                    assert_eq!(bar.left(), px(0.0));
                    assert_eq!(bar.right(), px(900.0));
                    assert_eq!(bar.bottom(), px(600.0));
                    assert!(bar.size.height <= px(28.0));
                    let alert = cx
                        .debug_bounds("managed-watch-alert")
                        .expect("warning while locked");
                    assert!(alert.left() >= px(0.0) && alert.right() <= px(900.0));
                    cx.simulate_click(alert.center(), gpui::Modifiers::default());
                    cx.run_until_parked();
                    app.update(cx, |app, _| {
                        assert_eq!(app.page, page, "details preserve navigation");
                        assert!(app.vault_locked);
                    });
                    let inspector = cx
                        .debug_bounds("channel-sync-inspector")
                        .expect("global details");
                    assert!(inspector.bottom() <= bar.top());
                    assert!(cx.debug_bounds("managed-watch-acknowledge").is_some());
                }
            }
        }
    }

    #[gpui::test]
    fn delta_batch_preserves_list_owner_selection_and_avoids_loading_state(
        cx: &mut TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Channel);
        app.update(cx, |app, cx| {
            let f = |id| TelegramFileSummary {
                message_id: id,
                sent_at_unix_ms: id * 1_000,
                modified_at_unix_ms: id * 1_000,
                file_name: format!("{id}.pdf"),
                caption: String::new(),
                mime_type: None,
                size_bytes: 42,
            };
            app.telegram_files = vec![f(2), f(1)];
            app.telegram_files_loading = false;
            app.selected_telegram_message_id = Some(2);
            app.selected_channel_message_ids = BTreeSet::from([2]);
            app.refresh_channel_file_table(cx);
            let owner = app.channel_file_table.entity_id();
            let mut renamed = f(2);
            renamed.file_name = "renamed.pdf".into();
            let delta = |revision, upserted, removed| {
                std::sync::Arc::new(teleark_runtime::ChannelDelta {
                    chat_id: 1001,
                    revision,
                    upserted,
                    removed,
                    history_exhausted: true,
                    managed_files_changed: false,
                })
            };
            app.apply_channel_change_batch(
                teleark_runtime::ChannelChanges {
                    revision: 4,
                    reset_required: false,
                    deltas: vec![
                        delta(3, vec![renamed.clone()], vec![]),
                        delta(4, vec![f(3)], vec![1]),
                    ],
                },
                cx,
            );
            assert_eq!(app.telegram_files, vec![f(3), renamed]);
            assert!(!app.telegram_files_loading);
            assert_eq!(app.channel_file_table.entity_id(), owner);
            assert_eq!(app.selected_telegram_message_id, Some(2));
            assert!(app.selected_channel_message_ids.contains(&2));
            app.apply_channel_change_batch(
                teleark_runtime::ChannelChanges {
                    revision: 5,
                    reset_required: false,
                    deltas: vec![delta(5, vec![], vec![2])],
                },
                cx,
            );
            assert_eq!(app.telegram_files, vec![f(3)]);
            assert!(app.selected_channel_message_ids.is_empty());
            assert_eq!(app.selected_telegram_message_id, None);
        });
    }
}
