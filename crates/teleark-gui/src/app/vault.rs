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
        let selected = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(self.tr("upload-file-picker-prompt")),
        });
        self.upload_picker_task = Some(cx.spawn(async move |this, cx| {
            let path = match selected.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                Ok(Ok(None)) => None,
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
            let Some(path) = path else { return };
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| {
                this.upload_source = Some(path);
                this.vault_activity = VaultActivity::Idle;
                cx.notify();
            });
        }));
    }

    pub(crate) fn enqueue_vault_upload(&mut self, cx: &mut Context<Self>) {
        let Some(source) = self.upload_source.clone() else {
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
        self.vault_activity = VaultActivity::Working;
        self.nav_selection = "nav-uploads";
        self.set_page(Page::Transfers, cx);
        let work =
            cx.background_spawn(async move { vault.upload_file(account_id, chat_id, source) });
        self.vault_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| {
                match result {
                    Ok(file) => {
                        this.managed_vault_files
                            .retain(|item| item.package_numeric_id != file.package_numeric_id);
                        this.managed_vault_files.insert(0, file);
                        this.upload_source = None;
                        this.vault_activity = VaultActivity::Succeeded;
                    }
                    Err(error) => this.vault_activity = VaultActivity::Failed(error.kind()),
                }
                cx.notify();
            });
        }));
    }
}
