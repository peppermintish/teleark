//! Library presentation owner. Business operations stay in the runtime.

use super::*;

impl TeleArkApp {
    pub(crate) fn download_library_file(
        &mut self,
        file: crate::library_state::LibraryRowView,
        cx: &mut Context<Self>,
    ) {
        if self.library_action_busy {
            return;
        }
        let Ok((account_id, chat_id, message_id)) =
            file.download_source(self.telegram_account.as_ref().map(|a| a.id))
        else {
            return;
        };
        let (Some(library), Some(transfers)) = (self.library.clone(), self.transfers.clone())
        else {
            return;
        };
        self.library_action_busy = true;
        self.library_action_error = None;
        let file_name = safe_suggested_file_name(&file.name, message_id);
        let work = cx.background_spawn(async move {
            library
                .next_download_destination(&file_name)
                .map(|destination| ChannelDownloadRequest {
                    account_id,
                    chat_id,
                    message_id,
                    message_sent_at_unix_ms: None,
                    file_name,
                    caption: None,
                    mime_type: None,
                    size_bytes: file.size_bytes,
                    destination,
                })
        });
        self.library_action_task = Some(cx.spawn(async move |this, cx| {
            let request = work.await;
            let Some(this) = this.upgrade() else {
                return;
            };
            this.update(cx, |this, cx| {
                this.library_action_busy = false;
                if this.telegram_account.as_ref().map(|a| a.id) != Some(account_id) {
                    this.library_action_error =
                        Some(teleark_core::ApplicationErrorKind::Authorization);
                    cx.notify();
                    return;
                }
                match request.and_then(|request| transfers.enqueue_channel_download(request)) {
                    Ok(id) => {
                        this.telegram_download = Some((id, ChannelDownloadState::Queued));
                        this.monitor_channel_download(id, cx);
                        this.set_page(Page::Transfers, cx);
                    }
                    Err(error) => this.library_action_error = Some(error.kind()),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub(crate) fn refresh_library(&mut self, cx: &mut Context<Self>) {
        if is_preview_library_selection(self.nav_selection) {
            self.library_query_generation = self.library_query_generation.wrapping_add(1);
            self.library_loading_more = false;
            self.library_load_more_error = None;
            self.selected_file = 0;
            cx.notify();
            return;
        }
        let Some(library) = self.library.clone() else {
            cx.notify();
            return;
        };
        let query = self.library_query(cx);
        self.library_query_generation = self.library_query_generation.wrapping_add(1);
        let generation = self.library_query_generation;
        self.library_content = LibraryContent::Loading;
        self.library_loading_more = false;
        self.library_load_more_error = None;
        self.selected_file = 0;
        cx.notify();

        let load = cx.background_spawn(async move {
            let page = library.search(&query)?;
            let statistics = library.statistics()?;
            Ok::<(LibraryPage, LibraryStatistics), ApplicationError>((page, statistics))
        });
        self.library_task = Some(cx.spawn(async move |this, cx| {
            let result = load.await;
            let Some(this) = this.upgrade() else {
                return;
            };
            this.update(cx, |this, cx| {
                if this.library_query_generation != generation {
                    return;
                }
                this.library_content = match result {
                    Ok((page, statistics)) => {
                        LibraryContent::from_snapshot(LibrarySnapshot::from_core(page, statistics))
                    }
                    Err(error) => LibraryContent::Failed(error.kind()),
                };
                cx.notify();
            });
        }));
    }

    pub(crate) fn load_more_library(&mut self, cx: &mut Context<Self>) {
        if self.library_loading_more {
            return;
        }
        let Some(after) = self
            .library_content
            .snapshot()
            .and_then(|snapshot| snapshot.next_cursor.clone())
        else {
            return;
        };
        let Some(library) = self.library.clone() else {
            return;
        };
        let mut query = self.library_query(cx);
        query.after = Some(after);
        let generation = self.library_query_generation;
        self.library_loading_more = true;
        self.library_load_more_error = None;
        cx.notify();

        let load = cx.background_spawn(async move { library.search(&query) });
        self.library_more_task = Some(cx.spawn(async move |this, cx| {
            let result = load.await;
            let Some(this) = this.upgrade() else {
                return;
            };
            this.update(cx, |this, cx| {
                if this.library_query_generation != generation {
                    return;
                }
                this.library_loading_more = false;
                match result {
                    Ok(page) => {
                        if let Some(snapshot) = this.library_content.snapshot_mut() {
                            snapshot.append_page(page);
                        }
                    }
                    Err(error) => this.library_load_more_error = Some(error.kind()),
                }
                cx.notify();
            });
        }));
    }

    pub(crate) fn choose_library_files(&mut self, cx: &mut Context<Self>) {
        if self.import_activity != ImportActivity::Idle {
            return;
        }
        let Some(library) = self.library.clone() else {
            self.import_feedback = Some(ImportFeedback::PickerFailed);
            cx.notify();
            return;
        };

        self.import_activity = ImportActivity::Picking;
        self.import_feedback = None;
        cx.notify();
        let selected_paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some(self.tr("library-file-picker-prompt")),
        });
        let background = cx.background_executor().clone();
        self.import_task = Some(cx.spawn(async move |this, cx| {
            let paths = match selected_paths.await {
                Ok(Ok(Some(paths))) if paths.is_empty() => {
                    if let Some(this) = this.upgrade() {
                        this.update(cx, |this, cx| {
                            this.import_activity = ImportActivity::Idle;
                            this.import_feedback = Some(ImportFeedback::NoFilesSelected);
                            cx.notify();
                        });
                    }
                    return;
                }
                Ok(Ok(Some(paths))) => paths,
                Ok(Ok(None)) => {
                    if let Some(this) = this.upgrade() {
                        this.update(cx, |this, cx| {
                            this.import_activity = ImportActivity::Idle;
                            cx.notify();
                        });
                    }
                    return;
                }
                Ok(Err(_)) | Err(_) => {
                    if let Some(this) = this.upgrade() {
                        this.update(cx, |this, cx| {
                            this.import_activity = ImportActivity::Idle;
                            this.import_feedback = Some(ImportFeedback::PickerFailed);
                            cx.notify();
                        });
                    }
                    return;
                }
            };

            if let Some(entity) = this.upgrade() {
                entity.update(cx, |this, cx| {
                    this.import_activity = ImportActivity::Importing;
                    cx.notify();
                });
            }
            let results = background
                .spawn(async move { library.import_paths(paths) })
                .await;
            let feedback =
                ImportFeedback::from_results(&results).or(Some(ImportFeedback::NoFilesSelected));
            let Some(this) = this.upgrade() else {
                return;
            };
            this.update(cx, |this, cx| {
                this.import_activity = ImportActivity::Idle;
                this.import_feedback = feedback;
                this.refresh_library(cx);
            });
        }));
    }

    pub(super) fn library_query(&self, cx: &Context<Self>) -> LibraryQuery {
        let kind = library_kind_for_selection(self.nav_selection);
        LibraryQuery {
            text: self.search_input.read(cx).value().to_string(),
            filter: LibraryFilter {
                kinds: kind.into_iter().collect(),
                ..LibraryFilter::default()
            },
            sort: LibrarySort::ModifiedNewest,
            page_size: 200,
            after: None,
        }
    }
}
