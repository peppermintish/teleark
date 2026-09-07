//! Vault presentation owner. Business operations stay in the runtime.

use super::*;

impl TeleArkApp {
    pub(crate) fn request_vault_unlock(&mut self, intent: UnlockIntent, cx: &mut Context<Self>) {
        if self.vault_activity == VaultActivity::Working {
            return;
        }
        self.sync_vault_status();
        self.unlock_intent = Some(intent);
        self.vault_activity = VaultActivity::Idle;
        self.vault_advanced_expanded = false;
        if !self.vault_locked {
            self.resume_unlock_intent(cx);
        }
        cx.notify();
    }

    pub(crate) fn clear_vault_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for input in [
            &self.vault_password,
            &self.vault_new_password,
            &self.vault_recovery_key,
        ] {
            input.update(cx, |input, cx| input.set_value("", window, cx));
        }
    }

    pub(crate) fn dismiss_unlock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.clear_vault_inputs(window, cx);
        self.unlock_intent = None;
        self.hide_vault_recovery_key(cx);
        self.vault_activity = VaultActivity::Idle;
        cx.notify();
    }

    pub(crate) fn resume_unlock_intent(&mut self, cx: &mut Context<Self>) {
        if self.vault_locked || self.vault_recovery_secret.is_some() {
            return;
        }
        match self.unlock_intent.take() {
            Some(UnlockIntent::Browse) => self.scan_managed_vault_files(cx),
            Some(UnlockIntent::Upload) => {
                self.show_upload = true;
                self.upload_queued = false;
            }
            Some(UnlockIntent::Download(id)) => self.download_managed_vault_file(id, cx),
            None => {}
        }
        cx.notify();
    }

    pub(crate) fn initialize_vault(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.vault_activity == VaultActivity::Working {
            return;
        }
        let password = self.vault_password.read(cx).value().to_string();
        let confirmation = self.vault_new_password.read(cx).value().to_string();
        if password.is_empty() || password != confirmation {
            self.vault_activity =
                VaultActivity::Failed(teleark_core::ApplicationErrorKind::InvalidRequest);
            cx.notify();
            return;
        }
        let Some(vault) = self.vault.clone() else {
            self.vault_activity =
                VaultActivity::Failed(teleark_core::ApplicationErrorKind::Persistence);
            cx.notify();
            return;
        };
        self.vault_activity = VaultActivity::Working;
        self.vault_recovery_secret = None;
        self.recovery_visible = false;
        self.vault_password
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.vault_new_password
            .update(cx, |input, cx| input.set_value("", window, cx));
        let work = cx.background_spawn(async move { vault.initialize(password) });
        self.vault_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| {
                match result {
                    Ok(secret) => {
                        this.vault_recovery_secret = Some(secret);
                        this.recovery_visible = true;
                        this.vault_activity = VaultActivity::Succeeded;
                    }
                    Err(error) => this.vault_activity = VaultActivity::Failed(error.kind()),
                }
                this.sync_vault_status();
                cx.notify();
            });
        }));
    }

    pub(crate) fn unlock_vault_with_password(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.vault_activity == VaultActivity::Working {
            return;
        }
        let password = self.vault_password.read(cx).value().to_string();
        if password.is_empty() {
            self.vault_activity =
                VaultActivity::Failed(teleark_core::ApplicationErrorKind::InvalidRequest);
            cx.notify();
            return;
        }
        let Some(vault) = self.vault.clone() else {
            return;
        };
        self.vault_activity = VaultActivity::Working;
        self.vault_password
            .update(cx, |input, cx| input.set_value("", window, cx));
        let work = cx.background_spawn(async move { vault.unlock_with_password(password) });
        self.finish_vault_unit_operation(work, cx);
    }

    pub(crate) fn unlock_vault_with_recovery(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.vault_activity == VaultActivity::Working {
            return;
        }
        let recovery = self.vault_recovery_key.read(cx).value().to_string();
        if recovery.is_empty() {
            self.vault_activity =
                VaultActivity::Failed(teleark_core::ApplicationErrorKind::InvalidRequest);
            cx.notify();
            return;
        }
        let Some(vault) = self.vault.clone() else {
            return;
        };
        self.vault_activity = VaultActivity::Working;
        self.vault_recovery_key
            .update(cx, |input, cx| input.set_value("", window, cx));
        let work = cx.background_spawn(async move { vault.unlock_with_recovery(recovery) });
        self.finish_vault_unit_operation(work, cx);
    }

    pub(crate) fn restore_vault_with_recovery(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.vault_activity == VaultActivity::Working {
            return;
        }
        let password = self.vault_password.read(cx).value().to_string();
        let confirmation = self.vault_new_password.read(cx).value().to_string();
        let recovery = self.vault_recovery_key.read(cx).value().to_string();
        if password.is_empty() || password != confirmation || recovery.is_empty() {
            self.vault_activity =
                VaultActivity::Failed(teleark_core::ApplicationErrorKind::InvalidRequest);
            cx.notify();
            return;
        }
        let Some(vault) = self.vault.clone() else {
            return;
        };
        self.vault_activity = VaultActivity::Working;
        self.vault_password
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.vault_new_password
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.vault_recovery_key
            .update(cx, |input, cx| input.set_value("", window, cx));
        let work =
            cx.background_spawn(async move { vault.restore_with_recovery(recovery, password) });
        self.finish_vault_unit_operation(work, cx);
    }

    pub(crate) fn change_vault_password(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let password = self.vault_password.read(cx).value().to_string();
        let confirmation = self.vault_new_password.read(cx).value().to_string();
        if password.is_empty() || password != confirmation {
            self.vault_activity =
                VaultActivity::Failed(teleark_core::ApplicationErrorKind::InvalidRequest);
            cx.notify();
            return;
        }
        let Some(vault) = self.vault.clone() else {
            return;
        };
        self.vault_activity = VaultActivity::Working;
        self.vault_password
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.vault_new_password
            .update(cx, |input, cx| input.set_value("", window, cx));
        let work = cx.background_spawn(async move { vault.change_password(password) });
        self.finish_vault_unit_operation(work, cx);
    }

    pub(crate) fn rotate_vault_recovery_key(&mut self, cx: &mut Context<Self>) {
        if self.vault_activity == VaultActivity::Working {
            return;
        }
        let Some(vault) = self.vault.clone() else {
            return;
        };
        self.vault_activity = VaultActivity::Working;
        self.vault_recovery_secret = None;
        self.recovery_visible = false;
        let work = cx.background_spawn(async move { vault.rotate_recovery_key() });
        self.vault_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| {
                match result {
                    Ok(secret) => {
                        this.vault_recovery_secret = Some(secret);
                        this.recovery_visible = true;
                        this.vault_activity = VaultActivity::Succeeded;
                    }
                    Err(error) => this.vault_activity = VaultActivity::Failed(error.kind()),
                }
                this.sync_vault_status();
                cx.notify();
            });
        }));
    }

    pub(crate) fn hide_vault_recovery_key(&mut self, cx: &mut Context<Self>) {
        self.recovery_visible = false;
        self.vault_recovery_secret = None;
        cx.notify();
    }

    pub(crate) fn export_vault_recovery_key(&mut self, cx: &mut Context<Self>) {
        let Some(secret) = self.vault_recovery_secret.clone() else {
            return;
        };
        let initial_directory = self
            .preferences
            .managed_files_root
            .clone()
            .or_else(|| {
                teleark_runtime::default_database_path()
                    .and_then(|path| path.parent().map(std::path::Path::to_path_buf))
            })
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let default_name = self.tr("vault-recovery-export-default-name").to_string();
        let selected = cx.prompt_for_new_path(&initial_directory, Some(&default_name));
        self.vault_task = Some(cx.spawn(async move |this, cx| {
            let path = match selected.await {
                Ok(Ok(Some(path))) => path,
                Ok(Ok(None)) => return,
                Ok(Err(_)) | Err(_) => {
                    let Some(this) = this.upgrade() else { return };
                    this.update(cx, |this, cx| {
                        this.vault_activity = VaultActivity::Failed(
                            teleark_core::ApplicationErrorKind::PermissionDenied,
                        );
                        cx.notify();
                    });
                    return;
                }
            };
            let result = cx
                .background_spawn(async move { write_recovery_key_file(&path, &secret) })
                .await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| {
                let succeeded = result.is_ok();
                this.vault_activity = result
                    .map(|()| VaultActivity::Succeeded)
                    .unwrap_or_else(|error| VaultActivity::Failed(error.kind()));
                if succeeded {
                    this.vault_recovery_secret = None;
                    this.recovery_visible = false;
                    this.resume_unlock_intent(cx);
                }
                cx.notify();
            });
        }));
    }

    pub(crate) fn lock_vault(&mut self, cx: &mut Context<Self>) {
        self.cancel_managed_scan();
        self.recovery_visible = false;
        self.vault_recovery_secret = None;
        self.vault_locked = true;
        self.managed_vault_files.clear();
        self.managed_vault_rejected = 0;
        let Some(vault) = self.vault.clone() else {
            self.vault_locked = true;
            return;
        };
        let work = cx.background_spawn(async move { vault.lock() });
        self.vault_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| {
                this.vault_activity = result
                    .map(|()| VaultActivity::Succeeded)
                    .unwrap_or_else(|error| VaultActivity::Failed(error.kind()));
                this.sync_vault_status();
                cx.notify();
            });
        }));
    }

    pub(super) fn finish_vault_unit_operation(
        &mut self,
        work: Task<Result<(), ApplicationError>>,
        cx: &mut Context<Self>,
    ) {
        self.vault_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| {
                this.vault_activity = result
                    .map(|()| VaultActivity::Succeeded)
                    .unwrap_or_else(|error| VaultActivity::Failed(error.kind()));
                this.sync_vault_status();
                if this.vault_activity == VaultActivity::Succeeded {
                    this.resume_unlock_intent(cx);
                }
                cx.notify();
            });
        }));
    }

    pub(super) fn sync_vault_status(&mut self) {
        if let Some(vault) = self.vault.as_ref() {
            self.vault_status = vault.status();
            self.vault_locked = self.vault_status.locked;
        }
    }

    pub(super) fn cancel_managed_scan(&mut self) {
        if let Some(cancel) = self.managed_scan_cancellation.take() {
            cancel.cancel();
        }
        self.managed_scan_generation = self.managed_scan_generation.wrapping_add(1);
        self.managed_scan_loading = false;
    }

    pub(crate) fn scan_managed_vault_files(&mut self, cx: &mut Context<Self>) {
        if self.vault_locked || self.managed_scan_loading {
            return;
        }
        let (Some(vault), Some(chat_id)) = (self.vault.clone(), self.active_storage_chat_id())
        else {
            return;
        };
        let Some(account_id) = self.telegram_account.as_ref().map(|a| a.id) else {
            return;
        };
        self.cancel_managed_scan();
        self.managed_scan_loading = true;
        let generation = self.managed_scan_generation;
        let cancellation = TelegramScanCancellation::new();
        self.managed_scan_cancellation = Some(cancellation.clone());
        let work = cx.background_spawn(async move {
            vault.scan_managed_files(account_id, chat_id, cancellation)
        });
        self.vault_scan_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| {
                if this.managed_scan_generation != generation {
                    return;
                }
                this.managed_scan_loading = false;
                this.managed_scan_cancellation = None;
                if this.active_storage_chat_id() != Some(chat_id)
                    || this.telegram_account.as_ref().map(|a| a.id) != Some(account_id)
                    || this.vault_locked
                {
                    return;
                }
                match result {
                    Ok(scan) => {
                        this.managed_vault_files = scan.files;
                        this.managed_vault_rejected = scan.rejected_manifests;
                        this.vault_activity = VaultActivity::Succeeded;
                    }
                    Err(error) => this.vault_activity = VaultActivity::Failed(error.kind()),
                }
                cx.notify();
            });
        }));
    }

    pub(crate) fn download_managed_vault_file(&mut self, package_id: u64, cx: &mut Context<Self>) {
        if self.vault_activity == VaultActivity::Working {
            return;
        }
        if self.vault_locked {
            self.request_vault_unlock(UnlockIntent::Download(package_id), cx);
            return;
        }
        let (Some(vault), Some(chat_id)) = (self.vault.clone(), self.active_storage_chat_id())
        else {
            return;
        };
        self.vault_activity = VaultActivity::Working;
        let Some(account_id) = self.telegram_account.as_ref().map(|a| a.id) else {
            return;
        };
        let work = cx
            .background_spawn(async move { vault.download_file(account_id, chat_id, package_id) });
        self.vault_download_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| {
                this.vault_activity = result
                    .map(|_| VaultActivity::Succeeded)
                    .unwrap_or_else(|error| VaultActivity::Failed(error.kind()));
                cx.notify();
            });
        }));
    }

    pub(crate) fn choose_upload_file(&mut self, cx: &mut Context<Self>) {
        if self.upload_preparing || self.vault_activity == VaultActivity::Working {
            return;
        }
        let selected = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some(self.tr("upload-file-picker-prompt")),
        });
        self.upload_preparing = true;
        self.upload_picker_task = Some(cx.spawn(async move |this, cx| {
            let result = match selected.await {
                Ok(Ok(Some(paths))) => Some(
                    cx.background_spawn(
                        async move { teleark_runtime::inspect_upload_sources(&paths) },
                    )
                    .await,
                ),
                Ok(Ok(None)) => None,
                Ok(Err(_)) | Err(_) => Some(Err(ApplicationError::new(
                    teleark_core::ApplicationErrorKind::PermissionDenied,
                ))),
            };
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| {
                this.upload_preparing = false;
                if let Some(result) = result {
                    match result {
                        Ok(sources) => {
                            this.upload_sources = sources;
                            this.vault_activity = VaultActivity::Idle;
                        }
                        Err(error) => this.vault_activity = VaultActivity::Failed(error.kind()),
                    }
                }
                cx.notify();
            });
        }));
    }

    fn apply_completed_vault_uploads(
        &mut self,
        account_id: i64,
        chat_id: i64,
        files: Vec<teleark_runtime::ManagedVaultFile>,
    ) {
        if files.is_empty()
            || self.vault_locked
            || self.telegram_account.as_ref().map(|account| account.id) != Some(account_id)
            || self.active_storage_chat_id() != Some(chat_id)
        {
            return;
        }
        // A scan started before publication may complete after this callback.
        // Invalidate its snapshot before installing authenticated upload receipts.
        self.cancel_managed_scan();
        for file in files {
            self.managed_vault_files
                .retain(|item| item.package_numeric_id != file.package_numeric_id);
            self.managed_vault_files.insert(0, file);
        }
    }

    pub(crate) fn enqueue_vault_upload(&mut self, cx: &mut Context<Self>) {
        if self.upload_in_flight
            || self.upload_preparing
            || self.vault_activity == VaultActivity::Working
        {
            return;
        }
        if self.upload_sources.is_empty() {
            self.vault_activity =
                VaultActivity::Failed(teleark_core::ApplicationErrorKind::InvalidRequest);
            cx.notify();
            return;
        };
        let Some(account_id) = self.telegram_account.as_ref().map(|account| account.id) else {
            self.vault_activity =
                VaultActivity::Failed(teleark_core::ApplicationErrorKind::Authorization);
            cx.notify();
            return;
        };
        let (Some(vault), Some(chat_id)) = (self.vault.clone(), self.active_storage_chat_id())
        else {
            self.vault_activity =
                VaultActivity::Failed(teleark_core::ApplicationErrorKind::Authorization);
            cx.notify();
            return;
        };
        if vault.status().locked {
            self.vault_activity =
                VaultActivity::Failed(teleark_core::ApplicationErrorKind::Authorization);
            cx.notify();
            return;
        }
        self.show_upload = false;
        self.upload_queued = false;
        self.upload_in_flight = true;
        self.vault_activity = VaultActivity::Working;
        self.nav_selection = "nav-uploads";
        self.set_page(Page::Transfers, cx);
        let sources = self
            .upload_sources
            .iter()
            .map(|source| source.path.clone())
            .collect();
        let login_generation = self.telegram_login_generation;
        let work =
            cx.background_spawn(async move { vault.upload_files(account_id, chat_id, sources) });
        self.vault_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| {
                if this.telegram_login_generation != login_generation
                    || this.telegram_account.as_ref().map(|account| account.id) != Some(account_id)
                {
                    return;
                }
                this.upload_in_flight = false;
                match result {
                    Ok(report) => {
                        this.apply_completed_vault_uploads(account_id, chat_id, report.completed);
                        let retry_sources = report
                            .failed
                            .iter()
                            .map(|failure| failure.source.clone())
                            .chain(report.cancelled.iter().cloned())
                            .collect::<std::collections::BTreeSet<_>>();
                        this.upload_sources
                            .retain(|source| retry_sources.contains(&source.path));
                        this.vault_activity = report.failed.first().map_or_else(
                            || {
                                if report.cancelled.is_empty() {
                                    VaultActivity::Succeeded
                                } else {
                                    VaultActivity::Failed(
                                        teleark_core::ApplicationErrorKind::Cancelled,
                                    )
                                }
                            },
                            |failure| VaultActivity::Failed(failure.kind),
                        );
                    }
                    Err(error) => this.vault_activity = VaultActivity::Failed(error.kind()),
                }
                cx.notify();
            });
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit as gpui;

    #[gpui::test]
    fn pending_upload_displays_feedback_before_runtime_rows_exist(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Transfers);
        cx.run_until_parked();
        app.update(cx, |app, cx| {
            app.preview_transfer_rows.clear();
            app.upload_in_flight = true;
            app.vault_activity = VaultActivity::Idle;
            app.enqueue_vault_upload(cx);
            assert_eq!(app.vault_activity, VaultActivity::Idle);
            cx.notify();
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("upload-preflight-status").is_some());
        app.update(cx, |app, cx| app.request_account_switch(cx));
        cx.run_until_parked();
        let confirm = cx
            .debug_bounds("account-switch-confirm")
            .expect("confirm")
            .center();
        cx.simulate_click(confirm, gpui::Modifiers::default());
        app.update(cx, |app, _| {
            assert!(app.upload_in_flight);
            assert!(app.telegram_account.is_some());
            assert!(app.confirm_account_switch);
            assert!(app.show_account_switch);
            app.confirm_account_switch = false;
        });
        app.update(cx, |app, cx| {
            app.upload_in_flight = false;
            cx.notify();
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("upload-preflight-status").is_none());
    }

    #[gpui::test]
    fn completed_upload_invalidates_older_scan_and_preserves_account_and_lock_scope(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Storage);
        cx.run_until_parked();
        app.update(cx, |app, _| {
            let account = app.telegram_account.as_ref().expect("preview account").id;
            let chat = app.active_storage_chat_id().expect("preview storage");
            let file = app.managed_vault_files[0].clone();
            app.managed_vault_files.clear();
            let cancellation = TelegramScanCancellation::new();
            app.managed_scan_cancellation = Some(cancellation.clone());
            app.managed_scan_loading = true;
            let old_scan = app.managed_scan_generation;
            app.apply_completed_vault_uploads(account, chat, vec![file.clone()]);
            assert!(cancellation.is_cancelled());
            assert_ne!(app.managed_scan_generation, old_scan);
            assert!(!app.managed_scan_loading);
            assert_eq!(app.managed_vault_files, vec![file.clone()]);
            app.apply_completed_vault_uploads(account, chat, vec![file.clone()]);
            assert_eq!(app.managed_vault_files.len(), 1);
            app.managed_vault_files.clear();
            app.apply_completed_vault_uploads(account + 1, chat, vec![file.clone()]);
            app.apply_completed_vault_uploads(account, chat + 1, vec![file.clone()]);
            assert!(app.managed_vault_files.is_empty());
            app.vault_locked = true;
            app.apply_completed_vault_uploads(account, chat, vec![file]);
            assert!(app.managed_vault_files.is_empty());
        });
    }
}
