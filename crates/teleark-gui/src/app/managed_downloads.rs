//! Managed-file selection and presentation; restore execution stays in Runtime.
use super::*;
use teleark_i18n::format::format_integer;
use teleark_runtime::{
    VaultDownloadBatchPhase as Phase, VaultDownloadBatchProgress, VaultDownloadBatchSnapshot,
    VaultFileHealth,
};

pub(crate) struct ManagedDownloadBatchUi {
    pub account: i64,
    pub chat: i64,
    pub progress: VaultDownloadBatchProgress,
    pub snapshot: VaultDownloadBatchSnapshot,
    pub finished: bool,
    pub error: Option<teleark_core::ApplicationErrorKind>,
}

impl Drop for ManagedDownloadBatchUi {
    fn drop(&mut self) {
        self.progress.stop_remaining();
    }
}

pub(crate) fn downloadable(file: &ManagedVaultFile) -> bool {
    matches!(
        file.health,
        VaultFileHealth::Unchecked | VaultFileHealth::Present
    )
}

impl TeleArkApp {
    pub(crate) fn managed_batch_active(&self) -> bool {
        self.managed_download_batch
            .as_ref()
            .is_some_and(|batch| !batch.finished)
    }

    pub(crate) fn prune_managed_download_selection(&mut self) {
        let eligible = self
            .managed_vault_files
            .iter()
            .filter(|file| downloadable(file))
            .map(|file| file.package_numeric_id)
            .collect::<BTreeSet<_>>();
        self.selected_managed_package_ids
            .retain(|id| eligible.contains(id));
    }

    pub(crate) fn set_managed_file_selected(
        &mut self,
        package: u64,
        selected: bool,
        cx: &mut Context<Self>,
    ) {
        if selected {
            if self
                .managed_vault_files
                .iter()
                .any(|file| file.package_numeric_id == package && downloadable(file))
            {
                self.selected_managed_package_ids.insert(package);
            }
        } else {
            self.selected_managed_package_ids.remove(&package);
        }
        cx.notify();
    }

    pub(crate) fn managed_matching_packages(&self, cx: &Context<Self>) -> Vec<u64> {
        let query = self.search_input.read(cx).value().to_lowercase();
        let rows = self.managed_projection.borrow_mut().filtered_rows(
            &self.managed_vault_files,
            &query,
            self.channel_batch_period,
            &self.channel_batch_kinds,
        );
        rows.iter()
            .filter_map(|index| self.managed_vault_files.get(*index))
            .filter(|file| downloadable(file))
            .map(|file| file.package_numeric_id)
            .collect()
    }

    pub(crate) fn select_all_managed_files(&mut self, cx: &mut Context<Self>) {
        self.selected_managed_package_ids
            .extend(self.managed_matching_packages(cx));
        cx.notify();
    }

    pub(crate) fn download_managed_files(&mut self, matching: bool, cx: &mut Context<Self>) {
        if self.managed_batch_active()
            || self.vault_download_in_flight
            || self.vault_locked
            || self.visual_preview
        {
            return;
        }
        self.prune_managed_download_selection();
        let packages = if matching {
            self.managed_matching_packages(cx)
        } else {
            self.selected_managed_package_ids.iter().copied().collect()
        };
        if packages.is_empty() {
            return;
        }
        self.start_managed_download_packages(packages, cx);
    }

    pub(crate) fn retry_managed_download_batch(&mut self, cx: &mut Context<Self>) {
        if self.managed_batch_active()
            || self.vault_download_in_flight
            || self.vault_locked
            || self.visual_preview
        {
            return;
        }
        let Some(batch) = &self.managed_download_batch else {
            return;
        };
        if self.telegram_account.as_ref().map(|a| a.id) != Some(batch.account)
            || self.active_storage_chat_id() != Some(batch.chat)
        {
            return;
        }
        let packages = batch.progress.failed_packages();
        if !packages.is_empty() {
            self.start_managed_download_packages(packages, cx);
        }
    }

    fn start_managed_download_packages(&mut self, packages: Vec<u64>, cx: &mut Context<Self>) {
        let (Some(vault), Some(account), Some(chat)) = (
            self.vault.clone(),
            self.telegram_account.as_ref().map(|a| a.id),
            self.active_storage_chat_id(),
        ) else {
            return;
        };
        let generation = (
            self.telegram_login_generation,
            self.vault_session_generation,
        );
        let progress = VaultDownloadBatchProgress::new(packages.len());
        let mut changes = progress.subscribe();
        self.managed_download_batch = Some(ManagedDownloadBatchUi {
            account,
            chat,
            snapshot: progress.snapshot(),
            progress: progress.clone(),
            finished: false,
            error: None,
        });
        self.vault_download_in_flight = true;
        cx.notify();
        let work = cx.background_spawn({
            let progress = progress.clone();
            async move {
                vault
                    .submit_download_batch(account, chat, packages, progress)?
                    .wait()
            }
        });
        self.managed_download_batch_task = Some(cx.spawn(async move |this, cx| {
            let mut work = work;
            loop {
                let result = tokio::select! {
                    result = &mut work => Some(result),
                    change = changes.changed() => { if change.is_err() { return; } None },
                    () = cx.background_executor().timer(Duration::from_secs(1)) => None,
                };
                let done = result.is_some();
                let Some(entity) = this.upgrade() else {
                    progress.stop_remaining();
                    return;
                };
                entity.update(cx, |app, cx| {
                    if app.telegram_account.as_ref().map(|a| a.id) != Some(account) {
                        progress.stop_remaining();
                        if done {
                            app.vault_download_in_flight = false;
                            app.managed_download_batch = None;
                        }
                        cx.notify();
                        return;
                    }
                    if generation != (app.telegram_login_generation, app.vault_session_generation) {
                        progress.stop_remaining();
                    }
                    if let Some(batch) = &mut app.managed_download_batch {
                        batch.snapshot = progress.snapshot();
                        batch.finished = done;
                        if let Some(Err(error)) = &result {
                            batch.error = Some(error.kind());
                        }
                    }
                    if done {
                        app.vault_download_in_flight = false;
                        app.advance_transition(cx);
                    }
                    cx.notify();
                });
                if done {
                    break;
                }
            }
        }));
    }

    pub(crate) fn stop_managed_download_batch(&mut self, cx: &mut Context<Self>) {
        if let Some(batch) = &mut self.managed_download_batch {
            batch.progress.stop_remaining();
            batch.snapshot = batch.progress.snapshot();
            cx.notify();
        }
    }

    pub(crate) fn managed_batch_status(&self) -> Option<(SharedString, Tone)> {
        let batch = self.managed_download_batch.as_ref()?;
        if self.telegram_account.as_ref().map(|a| a.id) != Some(batch.account)
            || self.active_storage_chat_id() != Some(batch.chat)
        {
            return None;
        }
        let snapshot = &batch.snapshot;
        let phase = if batch
            .error
            .is_some_and(|error| error != teleark_core::ApplicationErrorKind::Cancelled)
        {
            Phase::Failed
        } else if !batch.finished && !snapshot.active() {
            Phase::Restoring
        } else {
            snapshot.phase
        };
        let id = match phase {
            Phase::Queued => "managed-download-queued",
            Phase::Restoring => "managed-download-restoring",
            Phase::Stopping => "managed-download-stopping",
            Phase::Completed => "managed-download-completed",
            Phase::Cancelled => "managed-download-cancelled",
            Phase::Failed => "managed-download-failed",
        };
        Some((
            self.tr_with(
                id,
                MessageArgs::new()
                    .with(
                        "total",
                        format_integer(self.locale(), snapshot.total as u64),
                    )
                    .with(
                        "completed",
                        format_integer(self.locale(), snapshot.completed as u64),
                    )
                    .with(
                        "failed",
                        format_integer(self.locale(), snapshot.failed as u64),
                    )
                    .with(
                        "skipped",
                        format_integer(self.locale(), snapshot.skipped as u64),
                    )
                    .with(
                        "seconds",
                        format_integer(self.locale(), snapshot.phase_started.elapsed().as_secs()),
                    )
                    .with(
                        "idle",
                        format_integer(self.locale(), snapshot.last_activity.elapsed().as_secs()),
                    ),
            ),
            match phase {
                Phase::Completed => Tone::Green,
                Phase::Failed | Phase::Cancelled => Tone::Amber,
                _ => Tone::Blue,
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui_kit::test]
    fn batch_feedback_survives_navigation_and_waits_for_result_application(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Storage);
        app.update(cx, |app, cx| {
            let progress = VaultDownloadBatchProgress::new(6);
            app.managed_download_batch = Some(ManagedDownloadBatchUi {
                account: app.telegram_account.as_ref().expect("account").id,
                chat: app.active_storage_chat_id().expect("managed channel"),
                snapshot: progress.snapshot(),
                progress,
                finished: false,
                error: None,
            });
            app.vault_download_in_flight = true;
            app.storage_view = StorageView::Files;
            cx.notify();
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("managed-download-status").is_some());
        assert!(cx.debug_bounds("managed-download-stop").is_some());
        app.update(cx, |app, cx| app.set_page(Page::Settings, cx));
        cx.run_until_parked();
        assert!(cx.debug_bounds("shell-managed-download-batch").is_some());
        app.update(cx, |app, _| {
            let batch = app.managed_download_batch.as_mut().expect("batch");
            batch.snapshot.phase = Phase::Completed;
            batch.snapshot.completed = 6;
            assert!(
                !app.managed_batch_status()
                    .expect("pending status")
                    .0
                    .contains("complete:")
            );
            app.managed_download_batch.as_mut().expect("batch").finished = true;
            assert!(
                app.managed_batch_status()
                    .expect("applied status")
                    .0
                    .contains("complete:")
            );
            app.telegram_account = None;
            assert!(
                app.managed_batch_status().is_none(),
                "old account status is hidden"
            );
        });
    }
}
