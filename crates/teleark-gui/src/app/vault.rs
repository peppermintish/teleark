//! Vault presentation owner. Business operations stay in the runtime.

use super::*;

impl TeleArkApp {
    pub(crate) fn request_vault_unlock(&mut self, intent: UnlockIntent, cx: &mut Context<Self>) {
        if self.vault_activity == VaultActivity::Working {
            return;
        }
        if self.vault.is_none() && !self.visual_preview {
            self.vault_activity =
                VaultActivity::Failed(teleark_core::ApplicationErrorKind::Persistence);
            self.unlock_intent = None;
            cx.notify();
            return;
        }
        self.sync_vault_status();
        self.vault_key_progress = None;
        self.vault_new_epoch_confirmation = false;
        self.unlock_intent = Some(intent);
        self.vault_activity = VaultActivity::Idle;
        self.vault_advanced_expanded = false;
        if !self.vault_locked
            && !(intent == UnlockIntent::Upload && self.vault_status.active_key_locked)
        {
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
        self.vault_new_epoch_confirmation = false;
        self.vault_key_progress = None;
        self.hide_vault_recovery_key(cx);
        self.vault_activity = VaultActivity::Idle;
        cx.notify();
    }

    pub(crate) fn resume_unlock_intent(&mut self, cx: &mut Context<Self>) {
        if self.vault_locked
            || self.vault_recovery_secret.is_some()
            || (self.unlock_intent == Some(UnlockIntent::Upload)
                && self.vault_status.active_key_locked)
        {
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

    pub(crate) fn request_new_key_epoch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.vault_activity == VaultActivity::Working {
            return;
        }
        self.cancel_managed_scan();
        self.clear_vault_inputs(window, cx);
        self.vault_key_progress = None;
        self.vault_new_epoch_confirmation = true;
        self.vault_advanced_expanded = false;
        self.unlock_intent = Some(UnlockIntent::Upload);
        self.vault_activity = VaultActivity::Idle;
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
        let new_epoch = self.vault_new_epoch_confirmation;
        let key_progress = teleark_runtime::VaultKeyProgress::new();
        self.vault_key_progress = new_epoch.then(|| key_progress.clone());
        if new_epoch {
            self.vault_key_presentation = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(200))
                        .await;
                    let Some(entity) = this.upgrade() else { return };
                    let done = entity.update(cx, |this, cx| {
                        cx.notify();
                        this.vault_key_progress
                            .as_ref()
                            .is_none_or(|progress| progress.snapshot().finished)
                    });
                    if done {
                        return;
                    }
                }
            }));
        }
        let generation = self.telegram_login_generation;
        let session_generation = self.vault_session_generation;
        cx.notify();
        let work = cx.background_spawn(async move {
            if new_epoch {
                vault.start_new_key_epoch_observed(password, key_progress)
            } else {
                vault.initialize(password)
            }
        });
        self.vault_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| {
                if this.telegram_login_generation != generation
                    || this.vault_session_generation != session_generation
                {
                    return;
                }
                this.vault_key_presentation = None;
                match result {
                    Ok(secret) => {
                        this.vault_new_epoch_confirmation = false;
                        this.managed_vault_files = Default::default();
                        this.managed_health_checked = None;
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
        let generation = (
            self.telegram_login_generation,
            self.vault_session_generation,
        );
        self.vault_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| {
                if generation
                    != (
                        this.telegram_login_generation,
                        this.vault_session_generation,
                    )
                {
                    return;
                }
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
        let generation = (
            self.telegram_login_generation,
            self.vault_session_generation,
        );
        self.vault_task = Some(cx.spawn(async move |this, cx| {
            let path = match selected.await {
                Ok(Ok(Some(path))) => path,
                Ok(Ok(None)) => return,
                Ok(Err(_)) | Err(_) => {
                    let Some(this) = this.upgrade() else { return };
                    this.update(cx, |this, cx| {
                        if generation
                            != (
                                this.telegram_login_generation,
                                this.vault_session_generation,
                            )
                        {
                            return;
                        }
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
                if generation
                    != (
                        this.telegram_login_generation,
                        this.vault_session_generation,
                    )
                {
                    return;
                }
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

    pub(crate) fn lock_vault(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.vault_session_generation = self.vault_session_generation.wrapping_add(1);
        self.clear_vault_inputs(window, cx);
        self.unlock_intent = None;
        self.show_upload = false;
        self.vault_new_epoch_confirmation = false;
        self.show_channel_detail = false;
        *self.managed_projection.borrow_mut() = Default::default();
        self.recovery_visible = false;
        self.vault_recovery_secret = None;
        self.vault_locked = true;
        self.vault_status.locked = true;
        self.vault_status.active_key_locked = true;
        self.vault_status.historical_key_unlocked = false;
        self.managed_vault_files = Default::default();
        self.managed_health_checked = None;
        self.managed_upload_receipts.clear();
        self.managed_vault_rejected = 0;
        let Some(vault) = self.vault.clone() else {
            self.vault_activity = VaultActivity::Idle;
            cx.notify();
            return;
        };
        // Runtime only revokes session admission and wakes idle owners here;
        // it never waits for network, crypto or an ongoing transfer.
        self.vault_activity = vault
            .lock()
            .map(|()| VaultActivity::Idle)
            .unwrap_or_else(|error| VaultActivity::Failed(error.kind()));
        self.sync_vault_status();
        cx.notify();
    }

    pub(super) fn finish_vault_unit_operation(
        &mut self,
        work: Task<Result<(), ApplicationError>>,
        cx: &mut Context<Self>,
    ) {
        let generation = (
            self.telegram_login_generation,
            self.vault_session_generation,
        );
        self.vault_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| {
                if generation
                    != (
                        this.telegram_login_generation,
                        this.vault_session_generation,
                    )
                {
                    return;
                }
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

    pub(crate) fn refresh_managed_vault_files(&mut self, cx: &mut Context<Self>) {
        if self.page == Page::Storage
            && let (Some(sync), Some(chat)) = (&self.channel_sync, self.storage_channel_id())
            && let Err(error) = sync.refresh(chat)
        {
            self.vault_activity = VaultActivity::Failed(error.kind());
        }
        self.scan_vault_files(true, cx);
    }

    pub(crate) fn scan_managed_vault_files(&mut self, cx: &mut Context<Self>) {
        self.scan_vault_files(false, cx);
    }

    fn scan_vault_files(&mut self, verify_health: bool, cx: &mut Context<Self>) {
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
        let cached = self.page == Page::Storage;
        if cached {
            self.managed_display_revision = self
                .channel_sync
                .as_ref()
                .and_then(|sync| sync.changes_since(chat_id, 0).ok())
                .map_or(0, |changes| changes.revision);
            self.managed_catalog_pending = self
                .channel_sync_snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.managed_watch.as_ref())
                .is_none_or(|watch| !watch.catalog_ready);
        }
        let observer = cached
            .then(|| {
                self.channel_sync
                    .as_ref()
                    .map(|sync| sync.observe_managed_scan(chat_id))
            })
            .flatten();
        cx.notify();
        let mode = if verify_health && cached {
            teleark_runtime::ManagedScanMode::CheckHealth
        } else if cached {
            teleark_runtime::ManagedScanMode::Cached
        } else {
            teleark_runtime::ManagedScanMode::Remote
        };
        let job = match vault.submit_managed_scan(
            account_id,
            chat_id,
            mode,
            cancellation,
            observer.clone(),
        ) {
            Ok(job) => job,
            Err(error) => {
                self.managed_scan_loading = false;
                self.managed_scan_cancellation = None;
                self.vault_activity = VaultActivity::Failed(error.kind());
                cx.notify();
                return;
            }
        };
        let work = cx.background_spawn(async move { job.wait() });
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
                {
                    return;
                }
                if this.vault_locked {
                    cx.notify();
                    return;
                }
                let key_activity = this.vault_activity;
                match result {
                    Ok(scan) => {
                        this.managed_upload_receipts.retain(|(a, c, file)| {
                            (*a, *c) == (account_id, chat_id)
                                && !scan.files.iter().any(|seen| {
                                    seen.manifest_message_id == file.manifest_message_id
                                })
                        });
                        this.managed_vault_files = std::sync::Arc::new(scan.files);
                        for (_, _, file) in &this.managed_upload_receipts {
                            std::sync::Arc::make_mut(&mut this.managed_vault_files)
                                .insert(0, file.clone());
                        }
                        this.managed_vault_rejected = scan.rejected_manifests;
                        this.managed_catalog_pending = scan.catalog_pending;
                        this.managed_catalog_limited = scan.catalog_limited;
                        this.managed_health_checked =
                            scan.health_checked_files.or(this.managed_health_checked);
                        this.vault_activity = VaultActivity::Succeeded;
                    }
                    Err(error) => this.vault_activity = VaultActivity::Failed(error.kind()),
                }
                if key_activity == VaultActivity::Working {
                    this.vault_activity = key_activity;
                }
                this.apply_managed_channel_changes(cx);
                cx.notify();
            });
        }));
    }

    pub(crate) fn download_managed_vault_file(&mut self, package_id: u64, cx: &mut Context<Self>) {
        if self.vault_activity == VaultActivity::Working || self.vault_download_in_flight {
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
        self.vault_download_in_flight = true;
        let Some(account_id) = self.telegram_account.as_ref().map(|a| a.id) else {
            return;
        };
        let generation = self.telegram_login_generation;
        let job = match vault.submit_download_file(account_id, chat_id, package_id) {
            Ok(job) => job,
            Err(error) => {
                self.vault_download_in_flight = false;
                self.vault_activity = VaultActivity::Failed(error.kind());
                cx.notify();
                return;
            }
        };
        let work = cx.background_spawn(async move { job.wait() });
        self.vault_download_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| {
                if this.telegram_login_generation != generation {
                    return;
                }
                this.vault_download_in_flight = false;
                let key_activity = this.vault_activity;
                this.vault_activity = result
                    .map(|_| VaultActivity::Succeeded)
                    .unwrap_or_else(|error| VaultActivity::Failed(error.kind()));
                if key_activity == VaultActivity::Working {
                    this.vault_activity = key_activity;
                }
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
        let generation = self.telegram_login_generation;
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
                if generation != this.telegram_login_generation {
                    return;
                }
                this.upload_preparing = false;
                if let Some(result) = result {
                    match result {
                        Ok(sources) => {
                            this.upload_draft_generation =
                                this.upload_draft_generation.wrapping_add(1);
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
        // Preserve the running scan. Its completion merges these authenticated
        // receipts, so an older catalog cannot erase newly uploaded files.
        for file in files {
            self.managed_upload_receipts.retain(|(a, c, old)| {
                (*a, *c) == (account_id, chat_id)
                    && old.manifest_message_id != file.manifest_message_id
            });
            self.managed_upload_receipts
                .push_back((account_id, chat_id, file.clone()));
            while self.managed_upload_receipts.len() > 128 {
                self.managed_upload_receipts.pop_front();
            }
            std::sync::Arc::make_mut(&mut self.managed_vault_files)
                .retain(|item| item.package_numeric_id != file.package_numeric_id);
            std::sync::Arc::make_mut(&mut self.managed_vault_files).insert(0, file);
        }
    }

    fn retain_upload_retry_sources(
        &mut self,
        draft_generation: u64,
        retry_sources: &std::collections::BTreeSet<std::path::PathBuf>,
    ) {
        if self.upload_draft_generation == draft_generation {
            self.upload_sources
                .retain(|source| retry_sources.contains(&source.path));
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
        if vault.status().active_key_locked {
            self.vault_activity =
                VaultActivity::Failed(teleark_core::ApplicationErrorKind::Authorization);
            cx.notify();
            return;
        }
        self.show_upload = false;
        self.upload_queued = false;
        self.upload_in_flight = true;
        self.vault_activity = VaultActivity::Idle;
        self.nav_selection = "nav-uploads";
        self.set_page(Page::Transfers, cx);
        let sources = self
            .upload_sources
            .iter()
            .map(|source| source.path.clone())
            .collect();
        let login_generation = self.telegram_login_generation;
        let draft_generation = self.upload_draft_generation;
        let job = match vault.submit_upload_files(account_id, chat_id, sources) {
            Ok(job) => job,
            Err(error) => {
                self.upload_in_flight = false;
                self.vault_activity = VaultActivity::Failed(error.kind());
                cx.notify();
                return;
            }
        };
        let work = cx.background_spawn(async move { job.wait() });
        self.vault_upload_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| {
                if this.telegram_login_generation != login_generation
                    || this.telegram_account.as_ref().map(|account| account.id) != Some(account_id)
                {
                    return;
                }
                this.upload_in_flight = false;
                let key_activity = this.vault_activity;
                match result {
                    Ok(report) => {
                        this.apply_completed_vault_uploads(account_id, chat_id, report.completed);
                        let retry_sources = report
                            .failed
                            .iter()
                            .map(|failure| failure.source.clone())
                            .chain(report.cancelled.iter().cloned())
                            .collect::<std::collections::BTreeSet<_>>();
                        this.retain_upload_retry_sources(draft_generation, &retry_sources);
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
                if key_activity == VaultActivity::Working {
                    this.vault_activity = key_activity;
                }
                cx.notify();
            });
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn folder_picker_error_is_specific_visible_and_preserves_the_upload_draft(
        cx: &mut gpui::TestAppContext,
    ) {
        use teleark_core::ApplicationErrorKind;
        use teleark_i18n::{Localizer, SupportedLocale};
        use teleark_runtime::AppearancePreference;

        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Storage);
        let source = std::env::current_exe().expect("synthetic executable");
        let folder = source.parent().expect("fixture directory").to_path_buf();
        app.update(cx, |app, cx| {
            app.show_upload = true;
            app.upload_sources =
                teleark_runtime::inspect_upload_sources(std::slice::from_ref(&source))
                    .expect("existing valid draft");
            app.choose_upload_file(cx);
            assert!(
                app.upload_preparing,
                "selection is acknowledged before the result"
            );
        });
        cx.simulate_path_prompt_response(|options| {
            assert!(options.files);
            // AppKit may present an application package as a selectable file.
            Some(vec![folder])
        });
        cx.run_until_parked();
        app.read_with(cx, |app, _| {
            assert_eq!(app.upload_sources.len(), 1);
            assert_eq!(app.upload_sources[0].path, source);
            assert_eq!(
                app.vault_activity,
                VaultActivity::Failed(ApplicationErrorKind::UploadFolderUnsupported),
            );
            assert!(!app.upload_preparing);
            assert!(
                !app.upload_in_flight,
                "invalid selection cannot start a transfer"
            );
        });

        for (width, height) in [(900.0, 600.0), (1120.0, 680.0)] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(height)));
            // Dedicated localization/wrapping checks; ordinary UI tests remain English.
            for locale in SupportedLocale::ALL {
                for appearance in [AppearancePreference::Light, AppearancePreference::Dark] {
                    cx.update(|window, cx| {
                        app.update(cx, |app, cx| {
                            app.localizer = Localizer::new(locale).expect("catalog");
                            crate::theme::apply_appearance(appearance, window, cx);
                            app.upload_body_scroll
                                .set_offset(gpui::point(gpui::px(0.0), gpui::px(-2000.0)));
                            let (message, _) =
                                crate::screens::settings::vault_activity_message(app)
                                    .expect("folder message");
                            assert_eq!(message, app.tr("upload-error-folder"));
                            assert_ne!(message, app.tr("vault-error-source-missing"));
                            assert!(message.contains("ZIP"));
                            cx.notify();
                        });
                    });
                    cx.run_until_parked();
                    let banner = cx
                        .debug_bounds("upload-folder-error")
                        .expect("pinned error");
                    let action = cx.debug_bounds("upload-add-queue").expect("primary action");
                    for bounds in [banner, action] {
                        assert!(
                            bounds.left() >= gpui::px(0.0) && bounds.right() <= gpui::px(width)
                        );
                        assert!(
                            bounds.top() >= gpui::px(0.0) && bounds.bottom() <= gpui::px(height)
                        );
                    }
                    assert!(
                        banner.bottom() < action.top(),
                        "error never covers the footer"
                    );
                }
            }
        }
        app.update(cx, |app, cx| app.choose_upload_file(cx));
        cx.simulate_path_prompt_response(|_| Some(vec![source]));
        cx.run_until_parked();
        app.read_with(cx, |app, _| {
            assert_eq!(app.vault_activity, VaultActivity::Idle)
        });
        assert!(
            cx.debug_bounds("upload-folder-error").is_none(),
            "valid retry clears the error"
        );
    }

    use gpui_kit as gpui;

    #[gpui::test]
    fn previous_batch_completion_does_not_clear_a_new_upload_draft(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Storage);
        app.update(cx, |app, _| {
            app.upload_sources =
                teleark_runtime::inspect_upload_sources(&[
                    std::env::current_exe().expect("synthetic test executable")
                ])
                .expect("source");
            let submitted_generation = app.upload_draft_generation;
            app.upload_draft_generation = app.upload_draft_generation.wrapping_add(1);
            app.retain_upload_retry_sources(submitted_generation, &Default::default());
            assert_eq!(app.upload_sources.len(), 1);
            app.retain_upload_retry_sources(app.upload_draft_generation, &Default::default());
            assert!(app.upload_sources.is_empty());
        });
    }

    #[gpui::test]
    fn late_unlock_callback_does_not_reopen_upload_after_manual_lock(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Storage);
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.unlock_intent = Some(UnlockIntent::Upload);
                app.vault_activity = VaultActivity::Working;
                let result = cx.background_spawn(async { Ok(()) });
                app.finish_vault_unit_operation(result, cx);
                app.lock_vault(window, cx);
            })
        });
        cx.run_until_parked();
        app.update(cx, |app, _| {
            assert!(app.vault_locked);
            assert!(!app.show_upload);
            assert!(app.unlock_intent.is_none());
            assert_eq!(app.vault_activity, VaultActivity::Idle);
        });
    }

    #[gpui::test]
    fn explicit_lock_keeps_submitted_tasks_and_scan_alive_but_clears_visible_secrets(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Storage);
        let cancellation = TelegramScanCancellation::new();
        let scan_cancel = cancellation.clone();
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.vault_locked = false;
                app.vault_status.locked = false;
                app.vault_status.active_key_locked = false;
                app.upload_in_flight = true;
                app.managed_scan_loading = true;
                app.managed_scan_cancellation = Some(scan_cancel);
                app.vault_upload_task =
                    Some(cx.spawn(async move |_, _| std::future::pending::<()>().await));
                app.vault_password.update(cx, |input, cx| {
                    input.set_value("synthetic password", window, cx)
                });
                let generation = app.vault_session_generation;
                app.lock_vault(window, cx);
                assert!(app.vault_locked);
                assert!(app.vault_status.active_key_locked);
                assert!(app.managed_vault_files.is_empty());
                assert!(app.vault_password.read(cx).value().is_empty());
                assert!(app.vault_upload_task.is_some());
                assert!(app.upload_in_flight);
                assert!(app.managed_scan_loading);
                assert_ne!(app.vault_session_generation, generation);
            })
        });
        assert!(!cancellation.is_cancelled());
        cx.run_until_parked();
        assert!(cx.debug_bounds("vault-session-locked-notice").is_some());
    }

    #[gpui::test]
    fn session_unlock_stays_on_current_page_and_upload_resume_requires_confirmation(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Settings);
        app.update(cx, |app, cx| {
            app.vault_locked = false;
            app.vault_status.active_key_locked = false;
            app.show_upload = false;
            app.request_vault_unlock(UnlockIntent::Browse, cx);
            assert_eq!(app.page, Page::Settings);
            assert!(!app.show_upload);
            assert!(app.unlock_intent.is_none());
            app.upload_sources =
                teleark_runtime::inspect_upload_sources(&[
                    std::env::current_exe().expect("synthetic test executable")
                ])
                .expect("test source");
        });
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.lock_vault(window, cx);
                app.request_vault_unlock(UnlockIntent::Upload, cx);
                assert!(!app.show_upload);
                assert_eq!(app.upload_sources.len(), 1);
                app.vault_locked = false;
                app.vault_status.active_key_locked = false;
                app.resume_unlock_intent(cx);
                assert!(app.show_upload);
                assert!(!app.upload_in_flight);
                assert_eq!(app.upload_sources.len(), 1);
                app.dismiss_unlock(window, cx);
                assert_eq!(app.upload_sources.len(), 1);
            })
        });
    }

    #[gpui::test]
    fn managed_projection_reuses_idle_rows_and_invalidates_edits_and_search(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Storage);
        app.update(cx, |app, _| {
            use std::sync::Arc;
            let mut cache = super::super::managed_projection::ManagedProjection::default();
            let first = cache.rows(&app.managed_vault_files, "");
            for _ in 0..100 {
                assert!(Arc::ptr_eq(
                    &first,
                    &cache.rows(&app.managed_vault_files, "")
                ));
            }
            let message = app.managed_vault_files[0].manifest_message_id;
            Arc::make_mut(&mut app.managed_vault_files)[0].logical_name =
                "unique-replacement".into();
            let changed = cache.rows(&app.managed_vault_files, "");
            assert!(!Arc::ptr_eq(&first, &changed));
            assert_eq!(
                cache
                    .selected(Some(message))
                    .expect("selected file")
                    .logical_name,
                "unique-replacement"
            );
            assert_eq!(
                cache
                    .rows(&app.managed_vault_files, "unique-replacement")
                    .len(),
                1
            );
            assert!(
                cache
                    .rows(&app.managed_vault_files, "absent-file-name")
                    .is_empty()
            );
            assert!(cache.selected(Some(message)).is_none());
        });
    }

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
    fn completed_upload_preserves_running_scan_receipts_and_account_and_lock_scope(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Storage);
        cx.run_until_parked();
        app.update(cx, |app, _| {
            let account = app.telegram_account.as_ref().expect("preview account").id;
            let chat = app.active_storage_chat_id().expect("preview storage");
            let file = app.managed_vault_files[0].clone();
            app.managed_vault_files = Default::default();
            app.managed_health_checked = None;
            let cancellation = TelegramScanCancellation::new();
            app.managed_scan_cancellation = Some(cancellation.clone());
            app.managed_scan_loading = true;
            let old_scan = app.managed_scan_generation;
            app.apply_completed_vault_uploads(account, chat, vec![file.clone()]);
            assert!(!cancellation.is_cancelled());
            assert_eq!(app.managed_scan_generation, old_scan);
            assert!(app.managed_scan_loading);
            assert_eq!(*app.managed_vault_files, vec![file.clone()]);
            app.apply_completed_vault_uploads(account, chat, vec![file.clone()]);
            assert_eq!(app.managed_vault_files.len(), 1);
            app.managed_vault_files = Default::default();
            app.managed_health_checked = None;
            app.apply_completed_vault_uploads(account + 1, chat, vec![file.clone()]);
            app.apply_completed_vault_uploads(account, chat + 1, vec![file.clone()]);
            assert!(app.managed_vault_files.is_empty());
            app.vault_locked = true;
            app.apply_completed_vault_uploads(account, chat, vec![file]);
            assert!(app.managed_vault_files.is_empty());
        });
    }
}
