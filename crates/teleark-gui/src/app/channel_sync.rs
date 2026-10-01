//! Local channel projection and synchronization presentation. No network scan
//! is owned by a selected view; navigating away only cancels its local read.
use super::*;
use gpui_kit::component::button::ButtonVariants as _;
use teleark_i18n::format::{format_integer, format_signed_integer};
use teleark_runtime::ChannelSyncPhase;

const VIEW_CACHE_BYTES: usize = 16 * 1024 * 1024;

pub(super) struct CachedChannelView {
    account: i64,
    chat: i64,
    files: Vec<TelegramFileSummary>,
    revision: i64,
    exhausted: bool,
}

impl TeleArkApp {
    pub(crate) fn remember_channel_view(&mut self) {
        let (Some(account), Some(chat)) = (self.telegram_account.as_ref(), self.selected_chat_id)
        else {
            return;
        };
        if self.channel_loaded_scope != Some((account.id, chat)) || self.telegram_files_loading {
            return;
        }
        self.channel_view_cache
            .retain(|view| (view.account, view.chat) != (account.id, chat));
        self.channel_view_cache.push_back(CachedChannelView {
            account: account.id,
            chat,
            files: self.telegram_files.clone(),
            revision: self.channel_display_revision,
            exhausted: self.telegram_files_exhausted,
        });
        while self.channel_view_cache.len() > 8
            || self
                .channel_view_cache
                .iter()
                .flat_map(|view| &view.files)
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

    pub(crate) fn restore_channel_view(&mut self, chat: i64) -> bool {
        self.channel_display_revision = 0;
        self.channel_loaded_scope = None;
        self.channel_local_read_failed = false;
        let Some(account) = &self.telegram_account else {
            return false;
        };
        let Some(view) = self
            .channel_view_cache
            .iter()
            .find(|view| (view.account, view.chat) == (account.id, chat))
        else {
            return false;
        };
        self.telegram_files = view.files.clone();
        self.channel_display_revision = view.revision;
        self.channel_loaded_scope = Some((account.id, chat));
        self.telegram_files_exhausted = view.exhausted;
        true
    }

    pub(crate) fn start_channel_sync(&mut self, cx: &mut Context<Self>) {
        if self.visual_preview {
            return;
        }
        if self
            .channel_sync
            .as_ref()
            .is_some_and(|sync| !sync.is_stopped())
        {
            return;
        }
        self.channel_sources_revision = 0;
        let (Some(telegram), Some(library), Some(account)) = (
            self.telegram.clone(),
            self.library.clone(),
            self.telegram_account.clone(),
        ) else {
            return;
        };
        let account_id = account.id;
        cx.notify();
        let mut subscription = match teleark_runtime::ChannelSync::start(
            telegram,
            library,
            account,
            self.telegram_chats.clone(),
        ) {
            Ok(sync) => {
                let subscription = sync.subscribe();
                self.channel_sync_source_names.clear();
                self.channel_sync_snapshot = sync.snapshot().ok();
                if let Some(snapshot) = self.channel_sync_snapshot.clone() {
                    self.remember_channel_sync_source_names(&snapshot);
                }
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
                    let Some(sync) = app.channel_sync.clone() else {
                        return false;
                    };
                    let mut library_changed = false;
                    match sync.snapshot() {
                        Ok(snapshot) => {
                            app.remember_channel_sync_source_names(&snapshot);
                            library_changed = app
                                .channel_sync_snapshot
                                .as_ref()
                                .is_none_or(|old| old.data_revision != snapshot.data_revision);
                            app.channel_sync_snapshot = Some(snapshot);
                        }
                        Err(error) => {
                            app.telegram_activity = TelegramActivity::Failed(error.kind())
                        }
                    }
                    if let Some((revision, chats)) =
                        sync.sources_since(app.channel_sources_revision)
                    {
                        library_changed = true;
                        if let Some(snapshot) = app.channel_sync_snapshot.clone() {
                            app.remember_channel_sync_source_names(&snapshot);
                        }
                        let managed_changed = app.storage_channel_id().is_some_and(|id| {
                            app.telegram_chats.iter().find(|chat| chat.id == id)
                                != chats.iter().find(|chat| chat.id == id)
                        });
                        app.channel_sources_revision = revision;
                        app.telegram_chats = chats.as_ref().clone();
                        if app
                            .selected_chat_id
                            .is_some_and(|id| !chats.iter().any(|chat| chat.id == id))
                        {
                            app.cancel_telegram_file_load(cx);
                            app.selected_chat_id = None;
                            app.telegram_files.clear();
                            app.channel_view_cache.clear();
                            app.refresh_channel_file_table(cx);
                        }
                        if managed_changed {
                            app.refresh_storage_channel(cx);
                        }
                        if app.page == Page::Channel
                            && app.selected_chat_id.is_none()
                            && let Some(chat) = chats
                                .iter()
                                .find(|chat| chat.kind == TelegramChatKind::Channel)
                        {
                            app.select_telegram_chat(chat.id, cx);
                        }
                    }
                    if let Some((chat, id)) = app.channel_history_request
                        && app
                            .channel_sync
                            .as_ref()
                            .and_then(|sync| sync.history_status(chat, id))
                            != Some(teleark_runtime::HistoryStatus::Pending)
                    {
                        app.channel_history_failed = matches!(
                            app.channel_sync
                                .as_ref()
                                .and_then(|sync| sync.history_status(chat, id)),
                            Some(teleark_runtime::HistoryStatus::Failed(_))
                        );
                        app.channel_history_request = None;
                        app.refresh_channel_file_table(cx);
                    }
                    if library_changed {
                        app.invalidate_library_from_sync(cx);
                    }
                    app.apply_channel_changes(cx);
                    app.apply_managed_channel_changes(cx);
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
                        app.channel_loaded_scope = Some((account, chat));
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

    pub(super) fn apply_channel_changes(&mut self, cx: &mut Context<Self>) {
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
        let scope = self
            .telegram_account
            .as_ref()
            .map(|account| (account.id, chat));
        if let Some((account, chat)) = scope
            && self
                .channel_sync_snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.managed_watch.as_ref())
                .is_some_and(|watch| watch.catalog_ready)
            && (self.vault_key_selection_scope.is_none()
                || changes.reset_required
                || changes.managed_catalog_changed)
            && self.vault_key_selection_scope != Some((account, chat, changes.revision))
            && self.vault_key_selection_task.is_none()
        {
            self.select_channel_key(account, chat, changes.revision, cx);
            return;
        }
        if self.vault_status.key_selection != Some(teleark_runtime::VaultKeySelection::Ready) {
            self.managed_vault_files = Default::default();
            self.selected_telegram_message_id = None;
            self.show_channel_detail = false;
            cx.notify();
            return;
        }
        if self.vault_locked || self.managed_scan_loading {
            return;
        }
        if self.managed_projection_scope != scope
            || changes.reset_required
            || changes.managed_catalog_changed
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

    pub(crate) fn cancel_channel_history(&mut self) {
        self.channel_history_armed = false;
        if let Some((chat, id)) = self.channel_history_request.take()
            && let Some(sync) = &self.channel_sync
        {
            sync.cancel_history(chat, id);
        }
        self.channel_history_failed = false;
    }

    pub(crate) fn arm_channel_history(&mut self, cx: &mut Context<Self>) {
        if !self.channel_history_armed
            && self.channel_history_request.is_none()
            && matches!(self.page, Page::Channel | Page::Storage)
            && self.storage_view == StorageView::RawFiles
        {
            self.channel_history_armed = true;
            self.refresh_channel_file_table(cx);
        }
    }

    pub(crate) fn request_channel_history(&mut self, cx: &mut Context<Self>) {
        if self.telegram_files_exhausted
            || self.channel_history_request.is_some()
            || self.channel_history_failed
        {
            return;
        }
        self.channel_history_armed = false;
        if let (Some(sync), Some(chat)) = (&self.channel_sync, self.selected_chat_id) {
            match sync.request_history(chat) {
                Ok(id) => self.channel_history_request = Some((chat, id)),
                Err(error) => {
                    self.channel_history_failed = true;
                    self.telegram_activity = TelegramActivity::Failed(error.kind());
                }
            }
        }
        self.refresh_channel_file_table(cx);
        cx.notify();
    }

    pub(crate) fn refresh_selected_channel(&mut self, cx: &mut Context<Self>) {
        self.channel_local_read_failed = false;
        self.channel_history_failed = false;
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
        if self.library_sync_loading {
            return self.tr("global-sync-library");
        }
        if self.library_sync_error.is_some() {
            return self.tr("global-sync-library-failed");
        }
        if self.account_restoring {
            return self.tr("global-sync-connecting");
        }
        if let Some(scan) = self
            .channel_sync_snapshot
            .as_ref()
            .and_then(|s| s.managed_scan.as_ref())
            .filter(|scan| scan.active())
        {
            return self.tr(sync_phase_id(scan.phase));
        }
        self.channel_sync_snapshot.as_ref().map_or_else(
            || self.tr("channel-sync-local-only"),
            |snapshot| self.tr(sync_phase_id(snapshot.phase)),
        )
    }

    pub(crate) fn sync_event_time(&self, at: std::time::Instant) -> SharedString {
        teleark_i18n::format::format_unix_millis(
            self.locale(),
            self.sync_time_anchor.unix_millis(at),
        )
        .into()
    }

    fn sync_completed_time(&self) -> SharedString {
        self.channel_sync_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.last_completed_at)
            .map_or_else(
                || self.tr("sync-no-completion"),
                |at| self.sync_event_time(at),
            )
    }

    fn remember_channel_sync_source_names(
        &mut self,
        snapshot: &teleark_runtime::ChannelSyncSnapshot,
    ) {
        let retained_ids = snapshot
            .events
            .iter()
            .filter_map(|event| event.chat_id)
            .chain(snapshot.active.iter().filter_map(|event| event.chat_id))
            .collect::<BTreeSet<_>>();
        self.channel_sync_source_names
            .retain(|chat_id, _| retained_ids.contains(chat_id));
        for chat_id in retained_ids {
            if self.channel_sync_source_names.contains_key(&chat_id) {
                continue;
            }
            if let Some(chat) = self.telegram_chats.iter().find(|chat| chat.id == chat_id) {
                self.channel_sync_source_names
                    .insert(chat_id, chat.name.clone());
            }
        }
    }

    pub(super) fn sync_history_rows(&self) -> Vec<sync_history::HistoryRow> {
        let mut rows = Vec::new();
        let source = |id: Option<i64>| -> SharedString {
            if id.is_some() && id == self.storage_channel_id() {
                return self
                    .storage_status
                    .channel()
                    .map(|c| c.name.clone().into())
                    .unwrap_or_else(|| self.tr("managed-watch-title"));
            }
            id.map_or_else(
                || self.tr("global-sync-account"),
                |chat_id| {
                    self.channel_sync_source_names
                        .get(&chat_id)
                        .map(String::as_str)
                        .or_else(|| {
                            self.telegram_chats
                                .iter()
                                .find(|chat| chat.id == chat_id)
                                .map(|chat| chat.name.as_str())
                        })
                        .map_or_else(
                            || {
                                self.tr_with(
                                    "global-sync-channel-id",
                                    MessageArgs::new().with(
                                        "chat_id",
                                        format_signed_integer(self.locale(), chat_id),
                                    ),
                                )
                            },
                            |name| name.to_owned().into(),
                        )
                },
            )
        };
        if let Some(snapshot) = &self.channel_sync_snapshot {
            for event in &snapshot.events {
                rows.push((
                    self.sync_time_anchor.unix_millis(event.at),
                    sync_history::HistoryRow {
                        title: self.tr(if event.phase == ChannelSyncPhase::Idle {
                            "sync-event-completed"
                        } else {
                            sync_phase_id(event.phase)
                        }),
                        time: self.sync_event_time(event.at),
                        tone: sync_tone(event.phase),
                        source: source(event.chat_id),
                        error: event
                            .failure
                            .map_or_else(|| "".into(), |e| self.application_error_message(e)),
                    },
                ));
            }
            if let Some(watch) = &snapshot.managed_watch {
                for change in &watch.changes {
                    let kind = match change.kind {
                        teleark_runtime::ManagedChannelChangeKind::Edited => "managed-watch-edited",
                        teleark_runtime::ManagedChannelChangeKind::Deleted => {
                            "managed-watch-deleted"
                        }
                        teleark_runtime::ManagedChannelChangeKind::Gap => "managed-watch-gap",
                    };
                    rows.push((
                        change.observed_at_unix_ms,
                        sync_history::HistoryRow {
                            title: if change.message_id > 0 {
                                self.tr_with(
                                    "sync-private-event",
                                    MessageArgs::new()
                                        .with("kind", self.tr(kind).to_string())
                                        .with(
                                            "message",
                                            format_integer(self.locale(), change.message_id as u64),
                                        ),
                                )
                            } else {
                                self.tr(kind)
                            },
                            time: teleark_i18n::format::format_unix_millis(
                                self.locale(),
                                change.observed_at_unix_ms,
                            )
                            .into(),
                            tone: components::Tone::Amber,
                            source: source(snapshot.managed_chat_id),
                            error: "".into(),
                        },
                    ));
                }
            }
        }
        for (phase, error, at) in &self.dialogs.history {
            rows.push((
                self.sync_time_anchor.unix_millis(*at),
                sync_history::HistoryRow {
                    title: self.tr(dialogs::phase_id(*phase)),
                    time: self.sync_event_time(*at),
                    tone: if error.is_some() {
                        components::Tone::Amber
                    } else {
                        components::Tone::Neutral
                    },
                    source: self.tr("global-sync-account"),
                    error: error.map_or_else(|| "".into(), |e| self.application_error_message(e)),
                },
            ));
        }
        rows.sort_by_key(|row| std::cmp::Reverse(row.0));
        rows.into_iter().map(|(_, row)| row).collect()
    }

    fn render_sync_summary(&self) -> AnyElement {
        let tone = if self.library_sync_error.is_some() {
            components::Tone::Red
        } else if self.library_sync_loading || self.account_restoring {
            components::Tone::Blue
        } else {
            self.channel_sync_snapshot
                .as_ref()
                .map_or(components::Tone::Neutral, |snapshot| {
                    sync_tone(
                        snapshot
                            .managed_scan
                            .as_ref()
                            .filter(|scan| scan.active())
                            .map_or(snapshot.phase, |scan| scan.phase),
                    )
                })
        };
        let mut summary = div()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .rounded(theme::RADIUS_LARGE)
            .bg(theme::canvas())
            .debug_selector(|| "sync-summary".into())
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap_2()
                    .child(
                        div()
                            .mt(px(6.0))
                            .size(px(7.0))
                            .flex_none()
                            .rounded_full()
                            .bg(tone.foreground()),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .text_color(theme::text_primary())
                            .child(self.channel_sync_label()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::text_secondary())
                            .child(self.tr("sync-last-completed")),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme::text_primary())
                            .child(self.sync_completed_time()),
                    ),
            );
        let event = if self.library_sync_loading {
            self.library_sync_started
        } else if self.account_restoring {
            self.account_restore_event_at
        } else {
            self.channel_sync_snapshot.as_ref().and_then(|snapshot| {
                snapshot
                    .managed_scan
                    .as_ref()
                    .filter(|scan| scan.active())
                    .map(|scan| scan.last_activity)
                    .or_else(|| {
                        (snapshot.phase != ChannelSyncPhase::Idle).then_some(snapshot.last_activity)
                    })
            })
        };
        if let Some(at) = event {
            summary = summary.child(div().text_xs().text_color(theme::text_secondary()).child(
                self.tr_with(
                    "sync-event-time",
                    MessageArgs::new().with("time", self.sync_event_time(at).to_string()),
                ),
            ));
        }
        if let Some(snapshot) = self.channel_sync_snapshot.as_ref()
            && (snapshot.phase != ChannelSyncPhase::Idle
                || snapshot
                    .managed_scan
                    .as_ref()
                    .is_some_and(|scan| scan.active()))
        {
            let (phase, activity) = snapshot
                .managed_scan
                .as_ref()
                .filter(|scan| scan.active())
                .map_or((snapshot.phase_started, snapshot.last_activity), |scan| {
                    (scan.phase_started, scan.last_activity)
                });
            let elapsed = |at: std::time::Instant| {
                teleark_i18n::format::format_duration_millis(
                    self.locale(),
                    u64::try_from(at.elapsed().as_millis()).unwrap_or(u64::MAX),
                )
            };
            summary = summary.child(
                div().debug_selector(|| "sync-phase-duration".into()).child(
                    self.tr_with(
                        "channel-sync-timing",
                        MessageArgs::new()
                            .with("duration", elapsed(phase))
                            .with("activity", elapsed(activity)),
                    ),
                ),
            );
            if let Some(at) = snapshot.retry_at {
                summary = summary.child(
                    self.tr_with(
                        "channel-sync-next-at",
                        MessageArgs::new().with(
                            "seconds",
                            format_integer(
                                self.locale(),
                                at.saturating_duration_since(std::time::Instant::now())
                                    .as_secs(),
                            ),
                        ),
                    ),
                );
            }
        }
        summary
            .child(
                div()
                    .debug_selector(|| "sync-silence-policy".into())
                    .pt_2()
                    .border_t_1()
                    .border_color(theme::border_subtle())
                    .text_xs()
                    .text_color(theme::text_secondary())
                    .child(self.tr_with(
                        "sync-silence-policy",
                        MessageArgs::new().with(
                            "minutes",
                            format_integer(
                                self.locale(),
                                teleark_runtime::CHANNEL_UPDATE_SILENCE_RECOVERY_MINUTES,
                            ),
                        ),
                    )),
            )
            .into_any_element()
    }

    fn render_sync_work(&self) -> AnyElement {
        let mut work = div().flex().flex_col().gap_3();
        if let Some(snapshot) = &self.channel_sync_snapshot {
            let sources = self
                .telegram_chats
                .iter()
                .map(|chat| (chat.id, &chat.name))
                .collect::<std::collections::BTreeMap<_, _>>();
            for event in &snapshot.active {
                let source = event.chat_id.and_then(|id| sources.get(&id)).map_or_else(
                    || self.tr("global-sync-account"),
                    |name| (*name).clone().into(),
                );
                work = work.child(sync_work_row(
                    source,
                    self.tr(sync_phase_id(event.phase)),
                    self.sync_event_time(event.at),
                ));
            }
            if let Some(scan) = &snapshot.managed_scan
                && (scan.active() || scan.failure.is_some())
            {
                work = work.child(sync_work_row(
                    self.tr("sync-file-verification"),
                    self.tr(sync_phase_id(scan.phase)),
                    self.sync_event_time(scan.last_activity),
                ));
            }
        }
        work.into_any_element()
    }

    pub(crate) fn render_channel_sync_details(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let history = if let Some(history) = &self.sync_history {
            history.clone()
        } else {
            let rows = self.sync_history_rows();
            let owner = cx.entity();
            let history = cx.new(|cx| sync_history::SyncHistory::new(owner, rows, cx));
            self.sync_history = Some(history.clone());
            history
        };
        let mut history_style = gpui_kit::StyleRefinement::default();
        history_style.size.width = Some(gpui_kit::relative(1.0).into());
        let history_height = (f32::from(window.viewport_size().height) - 340.0).clamp(240.0, 640.0);
        history_style.size.height = Some(px(history_height).into());
        let mut body = div()
            .flex()
            .flex_col()
            .gap_4()
            .p_4()
            .min_w_0()
            .text_xs()
            .text_color(theme::text_secondary())
            .child(self.render_sync_summary())
            .when(
                self.channel_sync_snapshot.as_ref().is_some_and(|snapshot| {
                    !snapshot.active.is_empty()
                        || snapshot
                            .managed_scan
                            .as_ref()
                            .is_some_and(|scan| scan.active() || scan.failure.is_some())
                }),
                |body| body.child(self.render_sync_work()),
            );
        if (self.channel_sync_snapshot.is_none() || self.account_restoring)
            && let TelegramActivity::Failed(error) = self.telegram_activity
        {
            body = body.child(self.application_error_message(error));
        }
        if let Some(error) = self.library_sync_error {
            body = body.child(self.application_error_message(error));
        }
        if self.dialogs.has_activity() && !self.dialogs.ready {
            body = body.child(self.render_dialog_sync_activity(cx));
        }
        if let Some(snapshot) = &self.channel_sync_snapshot {
            if let Some(scan) = &snapshot.managed_scan {
                if let Some(delay) = scan.retry_after {
                    body = body.child(
                        div()
                            .debug_selector(|| "managed-sync-retry".into())
                            .text_sm()
                            .child(self.tr_with(
                                "managed-sync-retry",
                                MessageArgs::new().with(
                                    "seconds",
                                    format_integer(self.locale(), delay.as_secs()),
                                ),
                            )),
                    );
                } else if let Some(failure) = scan.failure {
                    body = body.child(self.application_error_message(failure));
                }
            }
            if let Some(failure) = snapshot.failure {
                body = body.child(self.application_error_message(failure));
            }
            body = body.child(self.tr("global-sync-automatic"));
            if let Some(chat) = snapshot
                .managed_chat_id
                .filter(|id| Some(*id) == self.storage_channel_id())
            {
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
                }
            }
            if snapshot.dropped_events > 0
                || snapshot.overflow_signals > 0
                || snapshot
                    .managed_watch
                    .as_ref()
                    .is_some_and(|w| w.omitted_changes() > 0)
            {
                body = body.child(self.tr("sync-older-events"));
            }
        }
        let history_empty = self.dialogs.history.is_empty()
            && self.channel_sync_snapshot.as_ref().is_none_or(|snapshot| {
                snapshot.events.is_empty()
                    && snapshot
                        .managed_watch
                        .as_ref()
                        .is_none_or(|watch| watch.changes.is_empty())
            });
        body = body
            .child(sync_section_title(self.tr("sync-recent-events")))
            .when(history_empty, |body| {
                body.child(
                    div()
                        .debug_selector(|| "sync-history-empty".into())
                        .child(self.tr("sync-no-events")),
                )
            })
            .child(
                div()
                    .flex()
                    .gap_2()
                    .h(theme::ROW_HEIGHT)
                    .items_center()
                    .border_b_1()
                    .border_color(theme::border())
                    .text_size(theme::LIST_TEXT_SIZE)
                    .child(
                        div()
                            .w(px(116.0))
                            .flex_none()
                            .child(self.tr("sync-column-time")),
                    )
                    .child(self.tr("sync-column-event")),
            ) // The locked GPUI cache does not replay accessibility nodes.
            // Keep controls discoverable when assistive technology is active.
            .child(if window.is_a11y_active() {
                div()
                    .w_full()
                    .h(px(history_height))
                    .child(history)
                    .into_any_element()
            } else {
                history.cached(history_style).into_any_element()
            });
        components::inspector_panel("channel-sync-inspector", theme::SYNC_INSPECTOR_WIDTH)
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
                    .flex_none()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(theme::border_subtle())
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .text_color(theme::text_primary())
                            .child(self.tr("channel-sync-details-title")),
                    )
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

fn sync_section_title(title: SharedString) -> gpui_kit::Div {
    div()
        .pt_2()
        .text_xs()
        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
        .text_color(theme::text_primary())
        .child(title)
}

fn sync_work_row(
    source: SharedString,
    phase: SharedString,
    time: SharedString,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    components::list_summary(source.clone(), format!("{phase} · {source} · {time}"))
        .min_w_0()
        .pl_3()
        .border_l_2()
        .border_color(theme::border())
}

fn sync_tone(phase: ChannelSyncPhase) -> components::Tone {
    use components::Tone;
    match phase {
        ChannelSyncPhase::Idle | ChannelSyncPhase::ManifestCompleted => Tone::Green,
        ChannelSyncPhase::Failed | ChannelSyncPhase::ManifestFailed => Tone::Red,
        ChannelSyncPhase::Waiting | ChannelSyncPhase::RateLimited => Tone::Amber,
        ChannelSyncPhase::Cancelled | ChannelSyncPhase::ManifestCancelled => Tone::Neutral,
        _ => Tone::Blue,
    }
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
        ChannelSyncPhase::Connecting => "global-sync-connecting",
        ChannelSyncPhase::Discovering => "global-sync-discovering",
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
pub(super) mod tests {
    use super::*;
    use gpui_kit as gpui;
    use gpui_kit::{TestAppContext, component::table::TableDelegate as _};

    #[gpui::test]
    fn retired_channel_history_keeps_source_attribution_and_resolves_retry(
        cx: &mut TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Transfers);
        app.update(cx, |app, _| {
            app.localizer = Localizer::new(SupportedLocale::EnUs).expect("catalog");
            let now = std::time::Instant::now();
            let retired_chat = TelegramChatSummary {
                id: 74,
                name: "Former channel".into(),
                username: None,
                kind: TelegramChatKind::Channel,
                sync_pts: None,
            };
            app.telegram_chats = vec![retired_chat];

            let mut snapshot = fixture_snapshot();
            snapshot.events.clear();
            snapshot.managed_watch = None;
            snapshot.active = vec![teleark_runtime::ChannelSyncEvent {
                phase: ChannelSyncPhase::Receiving,
                chat_id: Some(74),
                at: now,
                failure: None,
            }];
            app.remember_channel_sync_source_names(&snapshot);

            app.telegram_chats.clear();
            snapshot
                .events
                .push_back(teleark_runtime::ChannelSyncEvent {
                    phase: ChannelSyncPhase::Failed,
                    chat_id: Some(74),
                    at: now,
                    failure: Some(teleark_core::ApplicationErrorKind::Network),
                });
            snapshot
                .events
                .push_back(teleark_runtime::ChannelSyncEvent {
                    phase: ChannelSyncPhase::Cancelled,
                    chat_id: Some(74),
                    at: now,
                    failure: Some(teleark_core::ApplicationErrorKind::SourceMissing),
                });
            snapshot.active.clear();
            snapshot.phase = ChannelSyncPhase::Cancelled;
            snapshot.chat_id = None;
            snapshot.failure = Some(teleark_core::ApplicationErrorKind::SourceMissing);
            app.remember_channel_sync_source_names(&snapshot);

            assert!(snapshot.retry_target().is_none());
            assert!(app.channel_sync_source_names.len() <= snapshot.events.len());
            app.channel_sync_snapshot = Some(snapshot);
            let rows = app.sync_history_rows();
            assert_eq!(
                rows.iter()
                    .filter(|row| row.source.as_ref() == "Former channel")
                    .count(),
                2,
                "retained failure and retirement events keep the channel title"
            );

            let mut uncached = fixture_snapshot();
            uncached.events.clear();
            uncached.managed_watch = None;
            uncached
                .events
                .push_back(teleark_runtime::ChannelSyncEvent {
                    phase: ChannelSyncPhase::Cancelled,
                    chat_id: Some(73),
                    at: now,
                    failure: Some(teleark_core::ApplicationErrorKind::SourceMissing),
                });
            app.channel_sync_source_names.clear();
            app.channel_sync_snapshot = Some(uncached);
            let fallback = app
                .sync_history_rows()
                .into_iter()
                .next()
                .expect("retired channel history row");
            assert_eq!(fallback.source.as_ref(), "Telegram channel 73");
        });
    }

    #[gpui::test]
    fn unresolved_sync_failure_keeps_diagnostics_without_manual_retry(cx: &mut TestAppContext) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Transfers);
        cx.simulate_resize(gpui::size(px(900.0), px(600.0)));
        app.update(cx, |app, cx| {
            let now = std::time::Instant::now();
            let mut snapshot = fixture_snapshot();
            snapshot.events.clear();
            snapshot
                .events
                .push_back(teleark_runtime::ChannelSyncEvent {
                    phase: ChannelSyncPhase::Failed,
                    chat_id: Some(7),
                    at: now,
                    failure: Some(teleark_core::ApplicationErrorKind::Network),
                });
            snapshot
                .events
                .push_back(teleark_runtime::ChannelSyncEvent {
                    phase: ChannelSyncPhase::Receiving,
                    chat_id: Some(3),
                    at: now,
                    failure: None,
                });
            snapshot
                .events
                .push_back(teleark_runtime::ChannelSyncEvent {
                    phase: ChannelSyncPhase::Idle,
                    chat_id: Some(3),
                    at: now,
                    failure: None,
                });
            snapshot.phase = ChannelSyncPhase::Idle;
            snapshot.chat_id = None;
            snapshot.failure = None;
            snapshot.active.clear();
            snapshot.retry_at = None;
            assert_eq!(
                snapshot.retry_target().and_then(|event| event.chat_id),
                Some(7)
            );
            app.channel_sync_snapshot = Some(snapshot);
            app.channel_sync_details = true;
            cx.notify();
        });
        cx.run_until_parked();

        assert!(
            cx.debug_bounds("global-sync-retry").is_none(),
            "the sync inspector never asks users to retry synchronization"
        );
    }

    #[gpui::test]
    fn automatic_directory_recovery_remains_visible_without_manual_controls(
        cx: &mut TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Transfers);
        cx.simulate_resize(gpui::size(px(900.0), px(600.0)));
        app.update(cx, |app, cx| {
            let now = std::time::Instant::now();
            let mut snapshot = fixture_snapshot();
            snapshot.phase = ChannelSyncPhase::Receiving;
            snapshot.chat_id = Some(3);
            snapshot.failure = Some(teleark_core::ApplicationErrorKind::Network);
            snapshot.retry_at = Some(now + Duration::from_secs(2));
            snapshot.events.clear();
            snapshot
                .events
                .push_back(teleark_runtime::ChannelSyncEvent {
                    phase: ChannelSyncPhase::Waiting,
                    chat_id: None,
                    at: now,
                    failure: Some(teleark_core::ApplicationErrorKind::Network),
                });
            snapshot
                .events
                .push_back(teleark_runtime::ChannelSyncEvent {
                    phase: ChannelSyncPhase::Receiving,
                    chat_id: Some(3),
                    at: now,
                    failure: None,
                });
            snapshot.active = vec![teleark_runtime::ChannelSyncEvent {
                phase: ChannelSyncPhase::Receiving,
                chat_id: Some(3),
                at: now,
                failure: None,
            }];
            assert!(snapshot.directory_retry_waiting());
            app.channel_sync_snapshot = Some(snapshot);
            app.channel_sync_details = true;
            cx.notify();
        });
        cx.run_until_parked();

        assert!(
            cx.debug_bounds("global-sync-cancel-directory-retry")
                .is_none(),
            "the pending directory recovery stays automatic"
        );
        assert!(
            cx.debug_bounds("global-sync-cancel-chat-3").is_none(),
            "active channel recovery stays automatic"
        );
    }

    #[gpui::test]
    fn rate_limited_directory_recovery_has_no_manual_sync_controls(cx: &mut TestAppContext) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Transfers);
        cx.simulate_resize(gpui::size(px(900.0), px(600.0)));
        app.update(cx, |app, cx| {
            let now = std::time::Instant::now();
            let mut snapshot = fixture_snapshot();
            snapshot.phase = ChannelSyncPhase::RateLimited;
            snapshot.chat_id = None;
            snapshot.failure = None;
            snapshot.retry_at = Some(now + Duration::from_secs(120));
            snapshot.active.clear();
            snapshot.events.clear();
            snapshot
                .events
                .push_back(teleark_runtime::ChannelSyncEvent {
                    phase: ChannelSyncPhase::RateLimited,
                    chat_id: None,
                    at: now,
                    failure: None,
                });
            assert!(snapshot.directory_retry_waiting());
            app.channel_sync_snapshot = Some(snapshot);
            app.channel_sync_details = true;
            cx.notify();
        });
        cx.run_until_parked();

        assert!(
            cx.debug_bounds("global-sync-cancel-directory-retry")
                .is_none(),
            "automatic recovery honors FloodWait without exposing manual sync controls"
        );
    }

    #[gpui::test]
    fn concurrent_sync_work_remains_automatic(cx: &mut TestAppContext) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Transfers);
        cx.simulate_resize(gpui::size(px(900.0), px(600.0)));
        app.update(cx, |app, cx| {
            let now = std::time::Instant::now();
            let mut snapshot = fixture_snapshot();
            snapshot.phase = ChannelSyncPhase::Discovering;
            snapshot.chat_id = None;
            snapshot.failure = None;
            snapshot.active = vec![
                teleark_runtime::ChannelSyncEvent {
                    phase: ChannelSyncPhase::Discovering,
                    chat_id: None,
                    at: now,
                    failure: None,
                },
                teleark_runtime::ChannelSyncEvent {
                    phase: ChannelSyncPhase::Receiving,
                    chat_id: Some(3),
                    at: now,
                    failure: None,
                },
                teleark_runtime::ChannelSyncEvent {
                    phase: ChannelSyncPhase::Receiving,
                    chat_id: Some(4),
                    at: now,
                    failure: None,
                },
            ];
            app.channel_sync_snapshot = Some(snapshot);
            app.channel_sync_details = true;
            cx.notify();
        });
        cx.run_until_parked();

        assert!(
            cx.debug_bounds("global-sync-cancel-directory-active")
                .is_none()
        );
        assert!(cx.debug_bounds("global-sync-cancel-chat-3").is_none());
        assert!(cx.debug_bounds("global-sync-cancel-chat-4").is_none());
    }

    #[gpui::test]
    fn pending_channel_recovery_has_no_manual_sync_controls(cx: &mut TestAppContext) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Transfers);
        cx.simulate_resize(gpui::size(px(900.0), px(600.0)));
        app.update(cx, |app, cx| {
            let now = std::time::Instant::now();
            let mut snapshot = fixture_snapshot();
            snapshot.phase = ChannelSyncPhase::Receiving;
            snapshot.chat_id = Some(3);
            snapshot.failure = None;
            snapshot.retry_at = None;
            snapshot.events.clear();
            snapshot
                .events
                .push_back(teleark_runtime::ChannelSyncEvent {
                    phase: ChannelSyncPhase::Waiting,
                    chat_id: Some(7),
                    at: now,
                    failure: Some(teleark_core::ApplicationErrorKind::Network),
                });
            snapshot
                .events
                .push_back(teleark_runtime::ChannelSyncEvent {
                    phase: ChannelSyncPhase::Receiving,
                    chat_id: Some(3),
                    at: now,
                    failure: None,
                });
            snapshot.active = vec![teleark_runtime::ChannelSyncEvent {
                phase: ChannelSyncPhase::Receiving,
                chat_id: Some(3),
                at: now,
                failure: None,
            }];
            let retry_target = snapshot.retry_target().expect("retained channel retry");
            assert_eq!(retry_target.chat_id, Some(7));
            assert_eq!(retry_target.phase, ChannelSyncPhase::Waiting);
            app.channel_sync_snapshot = Some(snapshot);
            app.channel_sync_details = true;
            cx.notify();
        });
        cx.run_until_parked();

        assert!(cx.debug_bounds("global-sync-cancel-chat-retry-7").is_none());
        assert!(
            cx.debug_bounds("global-sync-cancel-pending-channel-retries")
                .is_none()
        );
        assert!(cx.debug_bounds("global-sync-cancel-chat-3").is_none());
    }

    #[gpui::test]
    fn vault_retry_remains_visible_and_cancellable_after_unlock(cx: &mut TestAppContext) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Storage);
        cx.simulate_resize(gpui::size(px(900.0), px(600.0)));
        let cancellation = TelegramScanCancellation::new();
        app.update(cx, |app, cx| {
            let now = std::time::Instant::now();
            let mut snapshot = fixture_snapshot();
            snapshot.managed_scan = Some(teleark_runtime::ManagedScanStatus {
                chat_id: 9000,
                phase: ChannelSyncPhase::Waiting,
                phase_started: now,
                last_activity: now,
                completed: 1,
                total: Some(3),
                cached: 1,
                rejected: 0,
                failure: Some(teleark_core::ApplicationErrorKind::Network),
                retry_after: Some(Duration::from_secs(2)),
            });
            app.channel_sync_snapshot = Some(snapshot);
            app.channel_sync_details = true;
            app.vault_locked = false;
            app.vault_activity = VaultActivity::Succeeded;
            app.managed_scan_loading = true;
            app.managed_scan_cancellation = Some(cancellation.clone());
            cx.notify();
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("managed-sync-retry").is_some());
        for selector in [
            "global-sync-retry",
            "managed-scan-retry",
            "managed-scan-cancel",
            "global-library-retry",
        ] {
            assert!(
                cx.debug_bounds(selector).is_none(),
                "sync recovery stays automatic: {selector}"
            );
        }
        app.update(cx, |app, cx| {
            assert_eq!(app.vault_activity, VaultActivity::Succeeded);
            assert!(!app.vault_locked);
            app.set_page(Page::Transfers, cx);
            assert!(
                app.managed_scan_loading,
                "navigation retains automatic sync"
            );
            assert!(!cancellation.is_cancelled());
            app.cancel_managed_scan();
            assert!(cancellation.is_cancelled());
            assert!(!app.managed_scan_loading);
            assert_eq!(app.vault_activity, VaultActivity::Succeeded);
        });
    }

    #[gpui::test]
    fn sync_status_has_fixed_times_and_no_idle_notifications(cx: &mut TestAppContext) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Transfers);
        app.update(cx, |app, cx| {
            app.channel_sync_snapshot = Some(fixture_snapshot());
            app.channel_sync_details = true;
            cx.notify();
        });
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("sync-silence-policy").is_some(),
            "the recovery policy stays inspectable without a countdown"
        );
        let history = app.update(cx, |app, _| app.sync_history.clone().expect("history"));
        let before = app.update(cx, |app, _| {
            (app.sync_completed_time(), app.sync_history_rows())
        });
        let built = history.update(cx, |history, _| history.materialized.get());
        assert!(built > 0 && built < 128, "only visible rows materialize");
        assert_eq!(
            cx.debug_bounds("sync-history-row-0")
                .expect("history summary")
                .size
                .height,
            px(24.0)
        );
        let notifications = std::rc::Rc::new(std::cell::Cell::new(0));
        let counter = notifications.clone();
        let _observe =
            cx.update(|_, cx| cx.observe(&app, move |_, _| counter.set(counter.get() + 1)));
        cx.background_executor
            .advance_clock(Duration::from_secs(3600));
        cx.run_until_parked();
        assert_eq!(
            notifications.get(),
            0,
            "waiting never schedules app refreshes"
        );
        assert_eq!(
            history.update(cx, |history, _| history.materialized.get()),
            built
        );
        app.update(cx, |app, cx| {
            assert_eq!(app.sync_completed_time(), before.0);
            assert!(app.sync_history_rows() == before.1);
            let completed = app.sync_completed_time();
            for phase in [
                ChannelSyncPhase::Receiving,
                ChannelSyncPhase::RateLimited,
                ChannelSyncPhase::Failed,
                ChannelSyncPhase::Cancelled,
                ChannelSyncPhase::Idle,
            ] {
                app.channel_sync_snapshot.as_mut().expect("snapshot").phase = phase;
                assert_eq!(
                    app.sync_completed_time(),
                    completed,
                    "only a successful event replaces completion"
                );
                let label = app.channel_sync_label();
                assert!(
                    !label.contains("Phase:")
                        && !label.contains(" active")
                        && !label.contains(" queued")
                );
            }
            let snapshot = app.channel_sync_snapshot.as_mut().expect("snapshot");
            snapshot
                .events
                .push_back(teleark_runtime::ChannelSyncEvent {
                    phase: ChannelSyncPhase::Cancelled,
                    chat_id: Some(9000),
                    at: snapshot.last_activity + Duration::from_secs(60),
                    failure: None,
                });
            assert!(
                app.sync_history_rows() != before.1,
                "business events update fixed rows"
            );
            app.channel_sync_details = false;
            cx.notify();
        });
        drop(history);
        cx.run_until_parked();
        app.update(cx, |app, _| assert!(app.sync_history.is_none()));
    }

    #[gpui::test]
    fn channel_and_storage_navigation_preserve_running_work_and_cached_files(
        cx: &mut TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Storage);
        let cancellation = TelegramScanCancellation::new();
        app.update(cx, |app, cx| {
            app.channel_sync_snapshot = Some(fixture_snapshot());
            app.managed_scan_loading = true;
            app.managed_scan_cancellation = Some(cancellation.clone());
            let generation = app.managed_scan_generation;
            let files = app.managed_vault_files.clone();
            let event_time = app
                .channel_sync_snapshot
                .as_ref()
                .expect("snapshot")
                .last_activity;
            let event_count = app
                .channel_sync_snapshot
                .as_ref()
                .expect("snapshot")
                .events
                .len();
            for _ in 0..20 {
                app.select_channel(1001, cx);
                app.select_storage(StorageView::Files, cx);
                assert!(
                    !app.channel_history_armed,
                    "mounting a view never requests history"
                );
                assert!(
                    !cancellation.is_cancelled(),
                    "navigation is not work cancellation"
                );
                assert!(app.managed_scan_loading);
                assert_eq!(app.managed_scan_generation, generation);
                assert!(std::sync::Arc::ptr_eq(&app.managed_vault_files, &files));
                let snapshot = app.channel_sync_snapshot.as_ref().expect("snapshot");
                assert_eq!(snapshot.last_activity, event_time);
                assert_eq!(snapshot.events.len(), event_count);
            }
        });
    }

    #[gpui::test]
    fn only_user_scroll_arms_history_and_navigation_disarms_it(cx: &mut TestAppContext) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Channel);
        app.update(cx, |app, cx| app.select_channel(1001, cx));
        cx.simulate_resize(gpui::size(px(900.0), px(600.0)));
        cx.run_until_parked();
        app.update(cx, |app, _| assert!(!app.channel_history_armed));
        let table = cx
            .debug_bounds("channel-file-table-viewport")
            .expect("table viewport");
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: table.center(),
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.0), px(-80.0))),
            ..Default::default()
        });
        cx.run_until_parked();
        app.update(cx, |app, cx| {
            assert!(
                app.channel_history_armed,
                "actual scroll allows near-end history demand"
            );
            app.select_storage(StorageView::Files, cx);
            assert!(!app.channel_history_armed);
            app.select_channel(1001, cx);
            assert!(
                !app.channel_history_armed,
                "reopening is not a scrolling event"
            );
        });
        app.update(cx, |app, cx| {
            app.channel_sync_snapshot = Some(fixture_snapshot());
            app.channel_sync_details = true;
            cx.notify();
        });
        cx.run_until_parked();
        let inspector = cx
            .debug_bounds("channel-sync-inspector")
            .expect("inspector");
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: inspector.center(),
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.0), px(-80.0))),
            ..Default::default()
        });
        cx.run_until_parked();
        app.update(cx, |app, _| {
            assert!(
                !app.channel_history_armed,
                "inspector scroll never requests file history"
            )
        });
    }

    #[gpui::test]
    fn cached_channel_reopening_preserves_revision_and_empty_results(cx: &mut TestAppContext) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Channel);
        app.update(cx, |app, cx| {
            app.selected_chat_id = Some(1001);
            app.channel_display_revision = 42;
            app.channel_loaded_scope =
                Some((app.telegram_account.as_ref().expect("account").id, 1001));
            app.telegram_files.clear();
            app.telegram_files_exhausted = true;
            app.remember_channel_view();
            app.selected_chat_id = Some(9000);
            app.channel_display_revision = 0;
            app.select_telegram_chat(1001, cx);
            assert_eq!(app.channel_display_revision, 42);
            assert!(app.telegram_files.is_empty());
            assert!(app.telegram_files_exhausted);
            assert!(!app.telegram_files_loading);
            app.telegram_account.as_mut().expect("account").id += 1;
            assert!(!app.restore_channel_view(1001), "cache is account-scoped");
        });
    }

    #[gpui::test]
    fn english_compact_sync_summary_and_independent_timeline_remain_reachable(
        cx: &mut TestAppContext,
    ) {
        use teleark_runtime::{ChannelSyncEvent, ChannelSyncSnapshot};
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Channel);
        cx.simulate_resize(gpui::size(px(900.0), px(600.0)));
        {
            let locale = SupportedLocale::EnUs;
            cx.update(|window, cx| {
                app.update(cx, |app, cx| {
                    app.localizer = Localizer::new(locale).expect("catalog");
                    theme::apply_appearance(AppearancePreference::Light, window, cx);
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
                        last_completed_at: Some(now - Duration::from_secs(60)),
                        retry_at: Some(now + Duration::from_secs(30)),
                        failure: None,
                        queued: 3,
                        failed_channels: 0,
                        committed_pages: 4,
                        data_revision: 4,
                        active: Vec::new(),
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
            for selector in [
                "channel-sync-footer",
                "channel-sync-history",
                "channel-sync-retry",
                "channel-sync-cancel",
                "channel-files-refresh",
            ] {
                assert!(
                    cx.debug_bounds(selector).is_none(),
                    "routine sync control must be absent: {selector}"
                );
            }
            let details = cx
                .debug_bounds("global-sync-details")
                .expect("one global status entry")
                .center();
            cx.simulate_click(details, gpui::Modifiers::default());
            cx.run_until_parked();
            assert!(
                cx.debug_bounds("channel-sync-inspector").is_some(),
                "unlocked status opens synchronization details"
            );
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
        let library = DesktopLibrary::open_synthetic(directory.join("catalog.sqlite3"))
            .expect("fixture database");
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
            app.telegram_auth = TelegramAuthState::Authorized(account.clone());
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
        cx.run_until_parked();
        drop(library);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while let Err(err) = std::fs::remove_dir_all(&directory) {
            if std::time::Instant::now() >= deadline {
                panic!("remove fixture: {err}");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    pub(in crate::app) fn fixture_snapshot() -> teleark_runtime::ChannelSyncSnapshot {
        let now = std::time::Instant::now();
        teleark_runtime::ChannelSyncSnapshot {
            account_id: 1,
            phase: ChannelSyncPhase::RateLimited,
            chat_id: Some(9000),
            phase_started: now,
            last_activity: now,
            last_completed_at: Some(now - Duration::from_secs(60)),
            retry_at: Some(now + Duration::from_secs(30)),
            failure: None,
            queued: 3,
            failed_channels: 0,
            committed_pages: 2,
            data_revision: 2,
            active: Vec::new(),
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
    fn private_changes_are_visible_in_global_history_without_a_key_unlock(cx: &mut TestAppContext) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Storage);
        for full in [false, true] {
            cx.simulate_resize(gpui::size(px(900.0), px(600.0)));
            cx.update(|window, _| {
                if window.is_fullscreen() != full {
                    window.toggle_fullscreen();
                }
                assert_eq!(window.is_fullscreen(), full);
            });
            app.update(cx, |app, cx| {
                app.vault_locked = true;
                app.channel_sync_details = true;
                app.dialogs.history.clear();
                let mut snapshot = fixture_snapshot();
                snapshot.events.clear();
                app.channel_sync_snapshot = Some(snapshot);
                cx.notify();
            });
            cx.run_until_parked();
            assert!(cx.debug_bounds("sync-private-disclosure").is_none());
            assert!(cx.debug_bounds("sync-history-empty").is_none());
            assert!(cx.debug_bounds("managed-watch-acknowledge").is_some());
            let inspector = cx
                .debug_bounds("channel-sync-inspector")
                .expect("inspector");
            cx.simulate_event(gpui::ScrollWheelEvent {
                position: inspector.center(),
                delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.0), px(-240.0))),
                ..Default::default()
            });
            cx.run_until_parked();
            assert!(cx.debug_bounds("sync-history-row-0").is_some());
            assert!(cx.debug_bounds("sync-history-row-1").is_none());
            app.update(cx, |app, cx| {
                app.channel_sync_snapshot
                    .as_mut()
                    .expect("snapshot")
                    .managed_watch = None;
                cx.notify();
            });
            cx.run_until_parked();
            assert!(cx.debug_bounds("managed-watch-acknowledge").is_none());
            assert!(cx.debug_bounds("sync-history-row-0").is_none());
            assert!(cx.debug_bounds("sync-history-empty").is_some());
            app.update(cx, |app, cx| {
                app.channel_sync_snapshot
                    .as_mut()
                    .expect("snapshot")
                    .managed_watch = fixture_snapshot().managed_watch;
                cx.notify();
            });
            cx.run_until_parked();
            assert!(cx.debug_bounds("sync-history-empty").is_none());
            assert!(cx.debug_bounds("sync-history-row-0").is_some());
            assert!(cx.debug_bounds("managed-watch-acknowledge").is_some());
            app.read_with(cx, |app, _| {
                assert!(
                    app.channel_sync_details,
                    "live updates keep the inspector open"
                );
                assert!(
                    app.vault_locked,
                    "viewing events requires no encryption key"
                );
                assert!(app.channel_sync.is_none(), "viewing starts no network work");
            });
        }
    }

    #[gpui::test]
    fn global_status_and_locked_private_warning_stay_at_window_bottom_on_every_page(
        cx: &mut TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Storage);
        cx.simulate_resize(gpui::size(px(900.0), px(600.0)));
        {
            let locale = SupportedLocale::EnUs;
            for dark in [false, true] {
                for page in [
                    Page::Account,
                    Page::Channel,
                    Page::Storage,
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
                    assert!(
                        cx.debug_bounds("channel-sync-inspector").is_none(),
                        "private status is display-only too"
                    );
                    app.update(cx, |app, cx| {
                        app.channel_sync_details = true;
                        cx.notify();
                    });
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
                    managed_catalog_changed: false,
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
                    managed_catalog_changed: false,
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
