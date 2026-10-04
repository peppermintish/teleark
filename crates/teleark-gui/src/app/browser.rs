//! Browser presentation owner. Business operations stay in the runtime.

use super::*;

impl TeleArkApp {
    pub(crate) fn channel_file_load_scope(&self) -> Option<(i64, i64, u64)> {
        let automatic = matches!(self.page, Page::Channel | Page::Storage)
            && self.storage_view == StorageView::RawFiles
            && self.channel_sync.is_some()
            && self.channel_history_armed;
        if self.library.is_none() || self.telegram.is_none() || !automatic {
            return None;
        }
        Some((
            self.telegram_account.as_ref()?.id,
            self.selected_chat_id?,
            self.telegram_file_generation,
        ))
    }

    pub(crate) fn select_telegram_chat(&mut self, chat_id: i64, cx: &mut Context<Self>) {
        self.cancel_channel_history();
        self.remember_channel_view();
        self.cancel_telegram_file_load(cx);
        self.telegram_file_generation = self.telegram_file_generation.wrapping_add(1);
        self.selected_chat_id = Some(chat_id);
        self.telegram_index = None;
        self.telegram_files.clear();
        self.telegram_files_next = None;
        self.telegram_files_exhausted = false;
        let restored = matches!(self.page, Page::Channel | Page::Storage)
            && self.restore_channel_view(chat_id);
        self.telegram_download = None;
        self.selected_telegram_message_id = None;
        self.show_channel_detail = false;
        self.selected_channel_message_ids.clear();
        self.selected_managed_package_ids.clear();
        self.channel_batch_period = ChannelBatchPeriod::AnyTime;
        self.channel_batch_kinds.clear();
        self.channel_batch_activity = ChannelBatchActivity::Idle;
        self.channel_batch_expanded = false;
        self.refresh_channel_file_table(cx);
        if restored {
            self.apply_channel_changes(cx);
        } else {
            self.load_selected_telegram_files(false, cx);
        }
    }

    pub(crate) fn select_channel(&mut self, chat_id: i64, cx: &mut Context<Self>) {
        if !self.telegram_is_authorized() {
            return;
        }
        self.last_channel_id = Some(chat_id);
        self.nav_selection = "nav-channel";
        self.storage_view = StorageView::RawFiles;
        self.set_page(Page::Channel, cx);
        if self.selected_chat_id != Some(chat_id) || self.telegram_files.is_empty() {
            self.select_telegram_chat(chat_id, cx);
        }
    }

    pub(crate) fn select_storage(&mut self, view: StorageView, cx: &mut Context<Self>) {
        if !self.telegram_is_authorized() {
            return;
        }
        if self.storage_view == StorageView::RawFiles {
            self.remember_channel_view();
        }
        self.storage_view = view;
        self.start_managed_status_clock(cx);
        self.nav_selection = "nav-storage";
        self.set_page(Page::Storage, cx);
        let Some(chat_id) = self.active_storage_chat_id() else {
            cx.notify();
            return;
        };
        if view == StorageView::Files {
            self.cancel_telegram_file_load(cx);
            if self.selected_chat_id != Some(chat_id) {
                self.telegram_file_generation = self.telegram_file_generation.wrapping_add(1);
                self.telegram_files.clear();
                self.channel_display_revision = 0;
                self.channel_loaded_scope = None;
                self.telegram_files_next = None;
                self.telegram_files_exhausted = false;
                self.selected_telegram_message_id = None;
                self.show_channel_detail = false;
            }
            self.selected_chat_id = Some(chat_id);
        } else if self.selected_chat_id != Some(chat_id) || self.telegram_files.is_empty() {
            self.select_telegram_chat(chat_id, cx);
        } else {
            self.refresh_channel_file_table(cx);
        }
        cx.notify();
    }

    pub(crate) fn storage_channel_id(&self) -> Option<i64> {
        self.storage_status
            .usable_channel()
            .map(|channel| channel.id)
    }

    pub(crate) fn active_storage_chat_id(&self) -> Option<i64> {
        self.storage_channel_id()
    }

    pub(crate) fn viewing_storage(&self) -> bool {
        self.page == Page::Storage
    }

    pub(crate) fn load_selected_telegram_files(&mut self, append: bool, cx: &mut Context<Self>) {
        if matches!(self.page, Page::Channel | Page::Storage) && !self.visual_preview {
            if append {
                self.request_channel_history(cx);
            } else {
                self.read_local_channel(cx);
            }
            return;
        }
        if append && (self.telegram_files_loading || self.telegram_files_exhausted) {
            return;
        }
        if !append {
            self.cancel_telegram_file_load(cx);
        }
        let (Some(telegram), Some(library), Some(account), Some(chat_id)) = (
            self.telegram.clone(),
            self.library.clone(),
            self.telegram_account.clone(),
            self.selected_chat_id,
        ) else {
            return;
        };
        let before = append.then_some(self.telegram_files_next).flatten();
        let account_id = account.id;
        let preserve_existing_rows = !self.telegram_files.is_empty();
        let generation = self.telegram_file_generation;
        let target = if append {
            CHANNEL_FILE_LOAD_MORE_SCAN
        } else {
            CHANNEL_FILE_INITIAL_SCAN
        };
        let cancellation = TelegramScanCancellation::new();
        self.telegram_file_cancellation = Some(cancellation.clone());
        if !append {
            self.selected_telegram_message_id = None;
            self.show_channel_detail = false;
            self.selected_channel_message_ids.clear();
        }
        self.telegram_files_loading = true;
        self.telegram_file_auto_load = true;
        self.telegram_files_scanned = 0;
        self.telegram_files_scan_target = u64::try_from(target).unwrap_or(u64::MAX);
        self.telegram_files_slow = false;
        self.telegram_files_retry_append = append;
        self.telegram_activity = TelegramActivity::Working;
        self.refresh_channel_file_table(cx);
        cx.notify();

        self.telegram_file_slow_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(3)).await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| {
                if this.telegram_file_generation == generation && this.telegram_files_loading {
                    this.telegram_files_slow = true;
                    this.refresh_channel_file_table(cx);
                    cx.notify();
                }
            });
        }));

        let background = cx.background_executor().clone();
        self.telegram_file_task = Some(cx.spawn(async move |this, cx| {
            let mut preserve_existing_rows = preserve_existing_rows;
            if !append {
                let cached = background
                    .spawn({
                        let library = library.clone();
                        async move {
                            library.cached_telegram_files(
                                account_id,
                                chat_id,
                                CHANNEL_FILE_LIST_CAPACITY,
                            )
                        }
                    })
                    .await;
                if let Ok(cached) = cached
                    && !cached.is_empty()
                    && !cancellation.is_cancelled()
                    && let Some(entity) = this.upgrade()
                {
                    preserve_existing_rows = true;
                    entity.update(cx, |app, cx| {
                        if app.selected_chat_id == Some(chat_id)
                            && app.telegram_file_generation == generation
                            && app.telegram_files.is_empty()
                        {
                            app.telegram_files = cached;
                            app.refresh_channel_file_table(cx);
                            cx.notify();
                        }
                    });
                }
            }

            let mut cursor = before;
            let mut remaining = target;
            let mut replace = !append && !preserve_existing_rows;
            let mut failure = None;
            while remaining > 0 && !cancellation.is_cancelled() {
                let limit = remaining.min(CHANNEL_FILE_SCAN_CHUNK);
                let scan = background
                    .spawn({
                        let telegram = telegram.clone();
                        let cancellation = cancellation.clone();
                        async move {
                            telegram.scan_file_page_cancellable(
                                account_id,
                                chat_id,
                                cursor,
                                limit,
                                cancellation,
                            )
                        }
                    })
                    .await;
                let page = match scan {
                    Ok(page) => page,
                    Err(error) if error.kind() == teleark_core::ApplicationErrorKind::Cancelled => {
                        break;
                    }
                    Err(error) => {
                        failure = Some(error.kind());
                        break;
                    }
                };
                let examined = usize::try_from(page.examined_messages).unwrap_or(usize::MAX);
                let next = page.next_before_message_id;
                let exhausted = page.exhausted;
                let cached_files = page.files.clone();
                if let Some(entity) = this.upgrade() {
                    entity.update(cx, |app, cx| {
                        if app.selected_chat_id != Some(chat_id)
                            || app.telegram_file_generation != generation
                        {
                            return;
                        }
                        let page_append = !replace;
                        let (page_next, page_exhausted) =
                            merge_telegram_file_page(&mut app.telegram_files, page, page_append);
                        app.telegram_files.truncate(CHANNEL_FILE_LIST_CAPACITY);
                        app.telegram_files_next = page_next;
                        app.telegram_files_exhausted = page_exhausted
                            || app.telegram_files.len() >= CHANNEL_FILE_LIST_CAPACITY;
                        app.telegram_files_scanned = app
                            .telegram_files_scanned
                            .saturating_add(u64::try_from(examined).unwrap_or(u64::MAX));
                        app.refresh_channel_file_table(cx);
                        cx.notify();
                    });
                }

                if !cached_files.is_empty() {
                    let library = library.clone();
                    background
                        .spawn(async move {
                            let _ =
                                library.cache_telegram_files(account_id, chat_id, &cached_files);
                        })
                        .await;
                }
                replace = false;
                remaining = remaining.saturating_sub(examined);
                if exhausted || examined == 0 || next.is_none() {
                    break;
                }
                cursor = next;
            }

            let Some(entity) = this.upgrade() else { return };
            entity.update(cx, |app, cx| {
                if app.selected_chat_id != Some(chat_id)
                    || app.telegram_file_generation != generation
                {
                    return;
                }
                app.telegram_files_loading = false;
                app.telegram_files_slow = false;
                app.telegram_file_cancellation = None;
                app.telegram_activity =
                    failure.map_or(TelegramActivity::Idle, TelegramActivity::Failed);
                app.refresh_channel_file_table(cx);
                cx.notify();
            });
        }));
    }

    pub(crate) fn cancel_telegram_file_load(&mut self, cx: &mut Context<Self>) {
        self.cancel_channel_history();
        // Invalidate deferred pagination even before its worker has started.
        self.telegram_file_auto_load = false;
        self.telegram_file_generation = self.telegram_file_generation.wrapping_add(1);
        if let Some(cancellation) = self.telegram_file_cancellation.take() {
            cancellation.cancel();
        }
        if self.telegram_files_loading {
            self.telegram_files_loading = false;
            self.telegram_files_slow = false;
            self.telegram_activity = TelegramActivity::Idle;
            self.refresh_channel_file_table(cx);
            cx.notify();
        }
    }

    pub(crate) fn retry_telegram_file_load(&mut self, cx: &mut Context<Self>) {
        if matches!(self.page, Page::Channel | Page::Storage) {
            self.refresh_selected_channel(cx);
            return;
        }

        let append = self.telegram_files_retry_append && !self.telegram_files.is_empty();
        self.load_selected_telegram_files(append, cx);
    }

    pub(crate) fn download_telegram_file(&mut self, message_id: i64, cx: &mut Context<Self>) {
        let (Some(library), Some(transfers), Some(chat_id), Some(file)) = (
            self.library.clone(),
            self.transfers.clone(),
            self.selected_chat_id,
            self.telegram_files
                .iter()
                .find(|file| file.message_id == message_id)
                .cloned(),
        ) else {
            self.telegram_activity =
                TelegramActivity::Failed(teleark_core::ApplicationErrorKind::InvalidRequest);
            cx.notify();
            return;
        };
        let Some(account_id) = self.telegram_account.as_ref().map(|account| account.id) else {
            return;
        };
        let suggested_name = safe_suggested_file_name(&file.file_name, file.message_id);
        let destination_work = cx.background_spawn({
            let suggested_name = suggested_name.clone();
            async move { library.next_download_destination(&suggested_name) }
        });
        self.telegram_download_task = Some(cx.spawn(async move |this, cx| {
            let destination = match destination_work.await {
                Ok(destination) => destination,
                Err(error) => {
                    let Some(this) = this.upgrade() else { return };
                    this.update(cx, |this, cx| {
                        this.telegram_activity = TelegramActivity::Failed(error.kind());
                        cx.notify();
                    });
                    return;
                }
            };
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| {
                if this.telegram_account.as_ref().map(|account| account.id) != Some(account_id) {
                    return;
                }
                this.enqueue_telegram_download(
                    transfers,
                    chat_id,
                    file,
                    suggested_name,
                    destination,
                    cx,
                );
            });
        }));
    }

    pub(super) fn enqueue_telegram_download(
        &mut self,
        transfers: DesktopTransfers,
        chat_id: i64,
        file: TelegramFileSummary,
        file_name: String,
        destination: std::path::PathBuf,
        cx: &mut Context<Self>,
    ) {
        let Some(account_id) = self.telegram_account.as_ref().map(|account| account.id) else {
            return;
        };
        match transfers.enqueue_channel_download(ChannelDownloadRequest {
            account_id,
            chat_id,
            message_id: file.message_id,
            message_sent_at_unix_ms: Some(file.sent_at_unix_ms),
            file_name,
            caption: (!file.caption.is_empty()).then_some(file.caption),
            mime_type: file.mime_type,
            size_bytes: file.size_bytes,
            destination,
        }) {
            Ok(id) => {
                self.telegram_download = Some((id, ChannelDownloadState::Queued));
                self.telegram_activity = TelegramActivity::Idle;
                self.monitor_channel_download(id, cx);
            }
            Err(error) => {
                self.telegram_activity = TelegramActivity::Failed(error.kind());
            }
        }
        cx.notify();
    }

    pub(crate) fn download_filtered_telegram_files(&mut self, cx: &mut Context<Self>) {
        if self.channel_batch_activity == ChannelBatchActivity::Preparing
            || self.filtered_batch_active()
        {
            return;
        }
        let (Some(library), Some(transfers), Some(chat_id)) = (
            self.library.clone(),
            self.transfers.clone(),
            self.selected_chat_id,
        ) else {
            return;
        };
        let files: Vec<_> = self
            .telegram_files
            .iter()
            .filter(|file| self.selected_channel_message_ids.contains(&file.message_id))
            .cloned()
            .collect();
        if files.is_empty() {
            self.channel_batch_activity = ChannelBatchActivity::NoMatches;
            cx.notify();
            return;
        }
        let Some(account_id) = self.telegram_account.as_ref().map(|account| account.id) else {
            return;
        };
        self.channel_batch_activity = ChannelBatchActivity::Preparing;
        let work = cx.background_spawn(async move {
            let mut reserved = BTreeSet::new();
            let mut requests = Vec::with_capacity(files.len());
            for file in files {
                let suggested_name = safe_suggested_file_name(&file.file_name, file.message_id);
                let destination =
                    next_reserved_download_destination(&library, &suggested_name, &mut reserved)?;
                requests.push(ChannelDownloadRequest {
                    account_id,
                    chat_id,
                    message_id: file.message_id,
                    message_sent_at_unix_ms: Some(file.sent_at_unix_ms),
                    file_name: suggested_name,
                    caption: (!file.caption.is_empty()).then_some(file.caption),
                    mime_type: file.mime_type,
                    size_bytes: file.size_bytes,
                    destination,
                });
            }
            let count = requests.len();
            transfers
                .enqueue_channel_download_batch(requests)
                .map(|batch_id| (batch_id, count))
        });
        self.telegram_batch_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| {
                this.channel_batch_activity = match result {
                    Ok((batch_id, count)) => {
                        this.selected_channel_message_ids.clear();
                        this.notify_channel_file_table(cx);
                        ChannelBatchActivity::Queued { batch_id, count }
                    }
                    Err(error) => ChannelBatchActivity::Failed(error.kind()),
                };
                cx.notify();
            });
        }));
    }
}
