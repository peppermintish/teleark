//! Library presentation owner. Business operations stay in the runtime.

use super::*;
use teleark_core::ApplicationErrorKind;

impl TeleArkApp {
    pub(crate) fn library_row_selectable(
        &self,
        file: &crate::library_state::LibraryRowView,
    ) -> bool {
        file.local_source_path.is_some()
            || file
                .download_source(self.telegram_account.as_ref().map(|a| a.id))
                .is_ok()
    }

    pub(crate) fn act_on_library_selection(&mut self, cx: &mut Context<Self>) {
        if self.library_action_busy || self.visual_preview {
            return;
        }
        let files = self
            .library_content
            .snapshot()
            .map(|snapshot| {
                snapshot
                    .rows
                    .iter()
                    .filter(|file| {
                        self.library_selection.contains(&file.id)
                            && self.library_row_selectable(file)
                    })
                    .take(5000)
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if files.is_empty() {
            return;
        }
        if self.library_view == LibraryView::Local {
            let mut folders = BTreeSet::new();
            for file in files {
                if let Some(path) = file.local_source_path
                    && let Some(parent) = path.parent()
                    && folders.insert(parent.to_owned())
                {
                    cx.reveal_path(&path);
                }
            }
            return;
        }
        let (Some(library), Some(transfers), Some(account_id)) = (
            self.library.clone(),
            self.transfers.clone(),
            self.telegram_account.as_ref().map(|a| a.id),
        ) else {
            return;
        };
        self.library_action_busy = true;
        self.library_action_error = None;
        self.library_batch_cancellation = Default::default();
        let cancellation = self.library_batch_cancellation.clone();
        let work = cx.background_spawn(async move {
            let prepared =
                prepare_library_downloads(files, account_id, &cancellation, |name, reserved| {
                    next_reserved_download_destination(&library, name, reserved)
                });
            match prepared {
                Ok(groups) => enqueue_library_groups(groups, &cancellation, |requests| {
                    transfers.enqueue_channel_download_batch(requests)
                }),
                Err(error) => (Vec::new(), Vec::new(), Err(error)),
            }
        });
        self.library_action_task = Some(cx.spawn(async move |this, cx| {
            let (queued, _batches, result) = work.await;
            let Some(this) = this.upgrade() else {
                return;
            };
            this.update(cx, |this, cx| {
                this.library_action_busy = false;
                if this.telegram_account.as_ref().map(|a| a.id) != Some(account_id) {
                    this.library_action_error = Some(ApplicationErrorKind::Authorization);
                    cx.notify();
                    return;
                }
                this.library_selection.retain(|id| !queued.contains(id));
                match result {
                    Ok(()) if this.page == Page::Library => this.set_page(Page::Transfers, cx),
                    Ok(()) => {}
                    Err(error) => this.library_action_error = Some(error.kind()),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

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
        self.library_batch_cancellation = Default::default();
        let cancellation = self.library_batch_cancellation.clone();
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
            let request = work.await.and_then(|request| {
                if cancellation.load(std::sync::atomic::Ordering::Relaxed) {
                    Err(ApplicationError::new(ApplicationErrorKind::Cancelled))
                } else {
                    Ok(request)
                }
            });
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

    pub(crate) fn invalidate_library_from_sync(&mut self, cx: &mut Context<Self>) {
        self.library_sync_dirty = true;
        self.apply_library_sync(cx);
    }

    fn apply_library_sync(&mut self, cx: &mut Context<Self>) {
        if !self.library_sync_dirty
            || self.library_sync_loading
            || self.visual_preview
            || self.page != Page::Library
            || self.library_view != LibraryView::Remote
            || self.library_content.snapshot().is_none()
            || self.library_loading_more
        {
            return;
        }
        let Some(library) = self.library.clone() else {
            return;
        };
        let query = self.library_query(cx);
        let account = self.telegram_account.as_ref().map(|account| account.id);
        let generation = self.library_query_generation;
        let cancellation = self.library_scan_cancellation.clone();
        let rows = self
            .library_content
            .snapshot()
            .map_or(0, |snapshot| snapshot.rows.len())
            .min(5000);
        self.library_sync_dirty = false;
        self.library_sync_loading = true;
        self.library_sync_error = None;
        self.library_sync_started = Some(std::time::Instant::now());
        // Preserve the visible projection and selection while its replacement is read.
        cx.notify();
        let work = cx.background_spawn(async move {
            let mut snapshot = load_library_snapshot(
                &library,
                LibraryView::Remote,
                account,
                query.clone(),
                None,
                &cancellation,
            )?;
            while snapshot.rows.len() < rows {
                let Some(after) = snapshot.next_cursor.clone() else {
                    break;
                };
                let page = load_library_snapshot(
                    &library,
                    LibraryView::Remote,
                    account,
                    query.clone(),
                    Some(after),
                    &cancellation,
                )?;
                if page.rows.is_empty() {
                    snapshot.next_cursor = page.next_cursor;
                    break;
                }
                snapshot.append_snapshot(page);
            }
            Ok::<_, ApplicationError>(snapshot)
        });
        self.library_sync_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let Some(entity) = this.upgrade() else { return };
            entity.update(cx, |app, cx| {
                app.library_sync_loading = false;
                if app.library_query_generation != generation
                    || app.telegram_account.as_ref().map(|account| account.id) != account
                {
                    app.apply_library_sync(cx);
                    cx.notify();
                    return;
                }
                match result {
                    Ok(snapshot) => {
                        let selected = app
                            .library_content
                            .snapshot()
                            .and_then(|old| old.rows.get(app.selected_file))
                            .map(|row| row.id.clone());
                        app.library_selection
                            .retain(|id| snapshot.rows.iter().any(|row| &row.id == id));
                        app.selected_file = selected
                            .and_then(|id| snapshot.rows.iter().position(|row| row.id == id))
                            .unwrap_or(0);
                        app.library_content = LibraryContent::from_snapshot(snapshot);
                    }
                    Err(error) => app.library_sync_error = Some(error.kind()),
                }
                app.apply_library_sync(cx);
                cx.notify();
            });
        }));
    }

    pub(crate) fn refresh_library(&mut self, cx: &mut Context<Self>) {
        self.library_sync_error = None;
        self.library_selection.clear();
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
                this.apply_library_sync(cx);
                cx.notify();
            });
        }));
    }

    pub(crate) fn load_more_library(&mut self, cx: &mut Context<Self>) {
        if self.library_loading_more || self.library_sync_loading {
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
                this.apply_library_sync(cx);
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

type LibraryDownloadGroups = std::collections::BTreeMap<
    i64,
    (
        Vec<crate::library_state::LibraryRowId>,
        Vec<ChannelDownloadRequest>,
    ),
>;

fn enqueue_library_groups(
    groups: LibraryDownloadGroups,
    cancellation: &std::sync::atomic::AtomicBool,
    mut enqueue: impl FnMut(Vec<ChannelDownloadRequest>) -> Result<u64, ApplicationError>,
) -> (
    Vec<crate::library_state::LibraryRowId>,
    Vec<u64>,
    Result<(), ApplicationError>,
) {
    let mut queued = Vec::new();
    let mut batches = Vec::new();
    let result = (|| {
        for (ids, requests) in groups.into_values() {
            if cancellation.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
            }
            batches.push(enqueue(requests)?);
            queued.extend(ids);
        }
        Ok(())
    })();
    (queued, batches, result)
}

fn prepare_library_downloads(
    files: Vec<crate::library_state::LibraryRowView>,
    account_id: i64,
    cancellation: &std::sync::atomic::AtomicBool,
    mut destination: impl FnMut(
        &str,
        &mut BTreeSet<std::path::PathBuf>,
    ) -> Result<std::path::PathBuf, ApplicationError>,
) -> Result<LibraryDownloadGroups, ApplicationError> {
    if files.is_empty() || files.len() > 5000 {
        return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
    }
    let mut reserved = BTreeSet::new();
    let mut groups = LibraryDownloadGroups::new();
    for file in files {
        if cancellation.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(ApplicationError::new(ApplicationErrorKind::Cancelled));
        }
        let (_, chat_id, message_id) = file
            .download_source(Some(account_id))
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::Authorization))?;
        let file_name = safe_suggested_file_name(&file.name, message_id);
        let destination = destination(&file_name, &mut reserved)?;
        let (ids, requests) = groups.entry(chat_id).or_default();
        ids.push(file.id);
        requests.push(ChannelDownloadRequest {
            account_id,
            chat_id,
            message_id,
            message_sent_at_unix_ms: None,
            file_name,
            caption: None,
            mime_type: None,
            size_bytes: file.size_bytes,
            destination,
        });
    }
    Ok(groups)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit as gpui;
    use teleark_core::ApplicationErrorKind;

    #[gpui::test]
    fn automatic_remote_library_updates_keep_rows_and_selection_and_reject_old_accounts(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Library);
        let directory = std::env::temp_dir().join(format!(
            "teleark-library-sync-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).expect("directory");
        let library = DesktopLibrary::open(directory.join("catalog.sqlite3")).expect("library");
        let account = TelegramAccount {
            id: 7,
            display_name: "Fixture".into(),
            username: None,
        };
        let chat = TelegramChatSummary {
            id: 20,
            name: "Source".into(),
            username: None,
            kind: TelegramChatKind::Channel,
            sync_pts: None,
        };
        library
            .save_telegram_sources(&account, &[chat])
            .expect("sources");
        let file = |name: &str, revision| TelegramFileSummary {
            message_id: 8,
            file_name: name.into(),
            caption: String::new(),
            mime_type: None,
            size_bytes: 42,
            sent_at_unix_ms: 1000,
            modified_at_unix_ms: revision,
        };
        library
            .cache_telegram_files(7, 20, &[file("Before.bin", 1000)])
            .expect("baseline file");
        let baseline = load_library_snapshot(
            &library,
            LibraryView::Remote,
            Some(7),
            Default::default(),
            None,
            &Default::default(),
        )
        .expect("baseline");
        let selected = baseline.rows[0].id.clone();
        app.update(cx, |app, _| {
            app.visual_preview = false;
            app.library = Some(library.clone());
            app.telegram_account = Some(account);
            app.library_view = LibraryView::Remote;
            app.nav_selection = "nav-all";
            app.library_content = LibraryContent::from_snapshot(baseline);
            app.library_selection = vec![selected.clone()];
        });
        library
            .cache_telegram_files(7, 20, &[file("After.bin", 2000)])
            .expect("changed file");
        app.update(cx, |app, cx| {
            app.invalidate_library_from_sync(cx);
            app.invalidate_library_from_sync(cx);
            assert!(app.library_sync_loading);
            assert_eq!(
                app.library_content.rows()[0].name,
                "Before.bin",
                "no loading flash"
            );
            assert_eq!(app.library_selection, vec![selected.clone()]);
        });
        cx.run_until_parked();
        app.update(cx, |app, cx| {
            assert!(!app.library_sync_loading);
            assert!(!app.library_sync_dirty);
            assert!(app.library_sync_error.is_none());
            assert_eq!(app.library_content.rows()[0].name, "After.bin");
            assert_eq!(app.library_selection, vec![selected.clone()]);
            app.invalidate_library_from_sync(cx);
            app.telegram_account.as_mut().expect("account").id = 99;
            app.library_content = LibraryContent::Loading;
        });
        cx.run_until_parked();
        app.update(cx, |app, _| {
            assert!(
                matches!(app.library_content, LibraryContent::Loading),
                "old-account result must not appear"
            );
            app.library = None;
            app.library_sync_task = None;
            app.visual_preview = true;
        });
        cx.run_until_parked();
        drop(library);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while let Err(err) = std::fs::remove_dir_all(&directory) {
            if std::time::Instant::now() >= deadline {
                panic!("cleanup: {err}");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[gpui::test]
    fn library_selection_is_scoped_to_loaded_results_and_clears_on_filter(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Library);
        cx.run_until_parked();
        let select = cx
            .debug_bounds("library-select-all")
            .expect("select all")
            .center();
        cx.simulate_click(select, gpui::Modifiers::default());
        app.update(cx, |app, cx| {
            assert_eq!(
                app.library_selection.len(),
                app.library_content.rows().len()
            );
            assert!(!app.library_selection.is_empty());
            assert_eq!(app.page, Page::Library);
            app.library_kind_selection = "nav-docs";
            app.refresh_library(cx);
            assert!(app.library_selection.is_empty());
        });
    }

    #[gpui::test]
    fn bulk_download_preparation_groups_channels_reserves_names_and_checks_account_and_cancel(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Library);
        let mut files = app.update(cx, |app, cx| {
            app.library_view = LibraryView::Remote;
            app.refresh_library(cx);
            app.library_content.rows().to_vec()
        });
        files.truncate(3);
        for (index, file) in files.iter_mut().enumerate() {
            file.source_account_id = Some(7);
            file.source_chat_id = Some(if index == 2 { 20 } else { 10 });
            file.source_message_id = Some(index as i64 + 1);
            file.encryption_state = teleark_core::EncryptionState::Unencrypted;
            file.package_id = None;
            file.name = "same.txt".into();
        }
        let cancellation = std::sync::atomic::AtomicBool::new(false);
        let groups =
            prepare_library_downloads(files.clone(), 7, &cancellation, |name, reserved| {
                assert_eq!(name, "same.txt");
                let path = std::path::PathBuf::from(format!("/tmp/{}.txt", reserved.len()));
                assert!(reserved.insert(path.clone()));
                Ok(path)
            })
            .expect("prepare");
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[&10].1.len(), 2);
        assert_eq!(groups[&20].1.len(), 1);
        assert_eq!(
            groups
                .values()
                .flat_map(|(_, requests)| requests.iter().map(|r| &r.destination))
                .collect::<BTreeSet<_>>()
                .len(),
            3
        );
        let (queued, batches, result) =
            enqueue_library_groups(groups.clone(), &cancellation, |requests| {
                if requests[0].chat_id == 10 {
                    Ok(42)
                } else {
                    Err(ApplicationError::new(ApplicationErrorKind::Persistence))
                }
            });
        assert_eq!(queued, groups[&10].0);
        assert_eq!(batches, vec![42]);
        assert_eq!(
            result.expect_err("partial failure").kind(),
            ApplicationErrorKind::Persistence
        );
        let (queued, batches, result) = enqueue_library_groups(groups, &cancellation, |_| {
            cancellation.store(true, std::sync::atomic::Ordering::Relaxed);
            Ok(43)
        });
        assert_eq!(queued.len(), 2);
        assert_eq!(batches, vec![43]);
        assert_eq!(
            result.expect_err("cancel between groups").kind(),
            ApplicationErrorKind::Cancelled
        );
        cancellation.store(false, std::sync::atomic::Ordering::Relaxed);
        let wrong_account = prepare_library_downloads(files.clone(), 8, &cancellation, |_, _| {
            panic!("must reject account before allocating")
        });
        assert_eq!(
            wrong_account.expect_err("wrong account").kind(),
            ApplicationErrorKind::Authorization
        );
        cancellation.store(true, std::sync::atomic::Ordering::Relaxed);
        let cancelled = prepare_library_downloads(files, 7, &cancellation, |_, _| {
            panic!("must cancel before allocating")
        });
        assert_eq!(
            cancelled.expect_err("cancelled").kind(),
            ApplicationErrorKind::Cancelled
        );
    }

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
