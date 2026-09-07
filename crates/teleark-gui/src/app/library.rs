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
        self.library_scan_cancellation.cancel();
        self.library_scan_cancellation = Default::default();
        if self.visual_preview {
            self.refresh_preview_library(cx);
            return;
        }
        let Some(library) = self.library.clone() else {
            cx.notify();
            return;
        };
        let query = self.library_query(cx);
        let view = self.library_view;
        let account = self.telegram_account.as_ref().map(|a| a.id);
        let cancellation = self.library_scan_cancellation.clone();
        self.library_query_generation = self.library_query_generation.wrapping_add(1);
        let generation = self.library_query_generation;
        self.library_content = LibraryContent::Loading;
        self.library_loading_more = false;
        self.library_load_more_error = None;
        self.selected_file = 0;
        cx.notify();

        let load = cx.background_spawn(async move {
            load_library_snapshot(&library, view, account, query, None, &cancellation)
        });
        self.library_task = Some(cx.spawn(async move |this, cx| {
            let result = load.await;
            let Some(this) = this.upgrade() else {
                return;
            };
            this.update(cx, |this, cx| {
                if this.library_query_generation != generation
                    || this.telegram_account.as_ref().map(|a| a.id) != account
                {
                    return;
                }
                this.library_content = match result {
                    Ok(snapshot) => LibraryContent::from_snapshot(snapshot),
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
        let query = self.library_query(cx);
        let view = self.library_view;
        let account = self.telegram_account.as_ref().map(|a| a.id);
        let cancellation = self.library_scan_cancellation.clone();
        let generation = self.library_query_generation;
        self.library_loading_more = true;
        self.library_load_more_error = None;
        cx.notify();

        let load = cx.background_spawn(async move {
            load_library_snapshot(&library, view, account, query, Some(after), &cancellation)
        });
        self.library_more_task = Some(cx.spawn(async move |this, cx| {
            let result = load.await;
            let Some(this) = this.upgrade() else {
                return;
            };
            this.update(cx, |this, cx| {
                if this.library_query_generation != generation
                    || this.telegram_account.as_ref().map(|a| a.id) != account
                {
                    return;
                }
                this.library_loading_more = false;
                match result {
                    Ok(page) => {
                        if let Some(snapshot) = this.library_content.snapshot_mut() {
                            snapshot.append_snapshot(page);
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
                this.library_view = LibraryView::Local;
                this.refresh_library(cx);
            });
        }));
    }

    pub(super) fn library_query(&self, cx: &Context<Self>) -> LibraryQuery {
        let kind = library_kind_for_selection(self.library_kind_selection);
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

fn load_library_snapshot(
    library: &DesktopLibrary,
    view: LibraryView,
    account: Option<i64>,
    mut query: LibraryQuery,
    after: Option<LibraryPageCursor>,
    cancellation: &teleark_runtime::LocalLibraryCancellation,
) -> Result<LibrarySnapshot, ApplicationError> {
    match view {
        LibraryView::Local => {
            let after = match after {
                Some(LibraryPageCursor::Local(cursor)) => Some(cursor),
                None => None,
                _ => {
                    return Err(ApplicationError::new(
                        teleark_core::ApplicationErrorKind::InvalidRequest,
                    ));
                }
            };
            library
                .local_library_page(
                    account,
                    &query.text,
                    query.filter.kinds.first().copied(),
                    after,
                    cancellation,
                )
                .map(LibrarySnapshot::from_local)
        }
        LibraryView::Remote => {
            let account = account.ok_or_else(|| {
                ApplicationError::new(teleark_core::ApplicationErrorKind::Authorization)
            })?;
            query.filter.source_account_id = Some(teleark_core::AccountId::new(account));
            query.after = match after {
                Some(LibraryPageCursor::Remote(cursor)) => Some(cursor),
                None => None,
                _ => {
                    return Err(ApplicationError::new(
                        teleark_core::ApplicationErrorKind::InvalidRequest,
                    ));
                }
            };
            let page = library.search(&query)?;
            Ok(LibrarySnapshot::from_core(
                page,
                LibraryStatistics::default(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit as gpui;

    #[gpui::test]
    fn source_tabs_switch_local_and_remote_rows_and_preserve_type_filter(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Library);
        cx.simulate_resize(gpui::size(px(900.0), px(600.0)));
        cx.run_until_parked();
        app.update(cx, |app, _| {
            assert_eq!(app.library_view, LibraryView::Local);
            assert!(!app.library_content.rows().is_empty());
            assert!(
                app.library_content
                    .rows()
                    .iter()
                    .all(|row| row.local_source_path.is_some())
            );
        });
        let remote_tab = cx
            .debug_bounds("library-tab-remote")
            .expect("remote tab")
            .center();
        cx.simulate_click(remote_tab, gpui::Modifiers::default());
        app.update(cx, |app, cx| {
            assert_eq!(app.library_view, LibraryView::Remote);
            assert!(
                app.library_content
                    .rows()
                    .iter()
                    .all(|row| row.local_source_path.is_none())
            );
            app.library_kind_selection = "nav-docs";
            app.refresh_library(cx);
            assert_eq!(app.library_content.rows().len(), 1);
        });
        cx.run_until_parked();
        let local_tab = cx
            .debug_bounds("library-tab-local")
            .expect("local tab")
            .center();
        cx.simulate_click(local_tab, gpui::Modifiers::default());
        app.update(cx, |app, _| {
            assert_eq!(app.library_view, LibraryView::Local);
            assert_eq!(app.library_content.rows().len(), 1);
            assert!(app.library_content.rows()[0].local_source_path.is_some());
        });
    }
}
