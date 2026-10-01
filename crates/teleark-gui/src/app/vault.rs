//! Vault presentation owner. Business operations stay in the runtime.

use super::*;
use crate::screens::transfers::{TransferAction, vault_transfer_selection_key};

impl TeleArkApp {
    pub(crate) fn apply_vault_transfer_action(
        &mut self,
        id: u64,
        action: TransferAction,
        cx: &mut Context<Self>,
    ) {
        let Some(account) = self.telegram_account.as_ref().map(|account| account.id) else {
            return;
        };
        let Some(vault) = self.vault.clone() else {
            return;
        };
        let Some(snapshot) = self.vault_transfer_snapshot(id) else {
            return;
        };
        if snapshot.account_id != account {
            return;
        }
        let control = matches!(
            action,
            TransferAction::Pause | TransferAction::Cancel | TransferAction::Delete
        );
        let key = (account, id, control);
        if self.vault_transfer_jobs.contains_key(&key)
            || self.vault_transfer_jobs.len() >= 64
            || self.visual_preview
        {
            return;
        }
        let generation = self.telegram_login_generation;
        let chat_id = snapshot.chat_id;
        let upload = snapshot.direction == teleark_runtime::VaultTransferDirection::Upload;
        self.transfer_action_error = None;
        let work = cx.background_spawn(async move {
            match action {
                TransferAction::Pause => vault
                    .submit_transfer_control(
                        account,
                        id,
                        teleark_runtime::VaultTransferControl::Pause,
                    )?
                    .wait()
                    .map(|()| None),
                TransferAction::Cancel => vault
                    .submit_transfer_control(
                        account,
                        id,
                        teleark_runtime::VaultTransferControl::Cancel,
                    )?
                    .wait()
                    .map(|()| None),
                TransferAction::Resume | TransferAction::Retry => {
                    if upload {
                        vault.submit_resume_upload(account, id)?.wait().map(Some)
                    } else {
                        vault
                            .submit_resume_download(account, id)?
                            .wait()
                            .map(|_| None)
                    }
                }
                TransferAction::Delete => vault.delete_transfer(account, id).map(|()| None),
            }
        });
        let task = cx.spawn(async move |this, cx| {
            let result = work.await;
            let Some(this) = this.upgrade() else {
                return;
            };
            this.update(cx, |this, cx| {
                this.vault_transfer_jobs.remove(&key);
                if this.telegram_login_generation != generation
                    || this.telegram_account.as_ref().map(|account| account.id) != Some(account)
                {
                    return;
                }
                match result {
                    Ok(Some(file)) => {
                        this.apply_completed_vault_uploads(account, chat_id, vec![file])
                    }
                    Ok(None) if action == TransferAction::Delete => {
                        this.show_transfer_detail = false;
                        this.selected_transfer_keys
                            .remove(&vault_transfer_selection_key(id));
                    }
                    Ok(None) => {}
                    Err(error) if error.kind() == teleark_core::ApplicationErrorKind::Cancelled => {
                    }
                    Err(error) => this.transfer_action_error = Some(error.kind()),
                }
                cx.notify();
            });
        });
        self.vault_transfer_jobs.insert(key, task);
        cx.notify();
    }

    pub(crate) fn open_vault_action(&mut self, intent: VaultAction, cx: &mut Context<Self>) {
        // Navigation and file selection never require a second unlock.
        if intent == VaultAction::Upload {
            self.show_upload = true;
            self.upload_queued = false;
        } else if !self.vault_locked {
            self.pending_vault_action = Some(intent);
            self.resume_pending_vault_action(cx);
        } else {
            self.pending_vault_action = Some(intent);
        }
        if self.vault_status.active_key_locked {
            self.prepare_vault_key(cx);
        }
        cx.notify();
    }

    pub(crate) fn prepare_vault_key(&mut self, cx: &mut Context<Self>) {
        if self.visual_preview || self.vault_activity == VaultActivity::Working {
            return;
        }
        let Some(vault) = self.vault.clone() else {
            return;
        };
        let progress = teleark_runtime::VaultKeyProgress::new();
        let admission_progress = progress.clone();
        self.finish_managed_key_operation(
            move |_| vault.submit_prepare_key(admission_progress),
            progress,
            false,
            cx,
        );
    }

    pub(crate) fn select_channel_key(
        &mut self,
        account: i64,
        chat: i64,
        revision: i64,
        cx: &mut Context<Self>,
    ) {
        let Some(vault) = self.vault.clone() else {
            return;
        };
        let progress = teleark_runtime::VaultKeyProgress::new();
        self.vault_key_selection_scope = Some((account, chat, revision));
        self.cancel_managed_scan();
        self.vault_key_progress = Some(progress.clone());
        self.vault_activity = VaultActivity::Working;
        // Keep the current account/channel projection during revalidation.
        // Only an authenticated result or an explicit lock may replace it.
        if self.managed_projection_scope != Some((account, chat)) {
            self.managed_vault_files = Default::default();
            self.selected_telegram_message_id = None;
            self.show_channel_detail = false;
        }
        cx.notify();
        // Paint the phase before Keychain, database, network or crypto work starts.
        let observer = self
            .channel_sync
            .as_ref()
            .map(|sync| sync.observe_managed_scan(chat));
        let work_progress = progress.clone();
        let work = cx.background_spawn(async move {
            vault.synchronize_channel_key(account, chat, work_progress, observer)
        });
        self.sync_vault_status();
        let mut events = progress.subscribe();
        self.vault_key_presentation = Some(cx.spawn(async move |this, cx| {
            loop {
                tokio::select! {
                    changed = events.changed() => if changed.is_err() { return; },
                    () = cx.background_executor().timer(Duration::from_secs(1)) => {},
                }
                let Some(entity) = this.upgrade() else { return };
                if entity.update(cx, |app, cx| {
                    cx.notify();
                    app.vault_key_progress
                        .as_ref()
                        .is_some_and(|p| p.snapshot().finished)
                }) {
                    return;
                }
            }
        }));
        let generation = self.telegram_login_generation;
        self.vault_key_selection_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let Some(entity) = this.upgrade() else { return };
            entity.update(cx, |app, cx| {
                if app.telegram_login_generation != generation
                    || app.vault_key_selection_scope != Some((account, chat, revision))
                {
                    return;
                }
                app.vault_key_selection_task = None;
                app.vault_key_presentation = None;
                app.sync_vault_status();
                app.vault_activity = match result {
                    Ok(_) => VaultActivity::Succeeded,
                    Err(error) => VaultActivity::Failed(error.kind()),
                };
                if app.finish_pending_channel_key_reselection() {
                    app.apply_managed_channel_changes(cx);
                    cx.notify();
                    return;
                }
                app.apply_managed_channel_changes(cx);
                if app.vault_status.key_selection == Some(teleark_runtime::VaultKeySelection::Ready)
                {
                    app.resume_pending_vault_action(cx);
                    app.resume_durable_uploads(cx);
                }
                cx.notify();
            });
        }));
    }

    pub(crate) fn retry_channel_key_selection(&mut self, cx: &mut Context<Self>) {
        if self.vault_key_selection_task.is_some() {
            return;
        }
        self.vault_key_selection_scope = None;
        self.apply_managed_channel_changes(cx);
    }

    pub(crate) fn request_channel_key_reselection(&mut self, account: i64, chat: i64) {
        self.vault_key_reselection_pending = Some((account, chat));
        self.finish_pending_channel_key_reselection();
    }

    pub(crate) fn finish_pending_channel_key_reselection(&mut self) -> bool {
        if self.vault_key_selection_task.is_some() {
            return false;
        }
        let Some((account, chat)) = self.vault_key_reselection_pending.take() else {
            return false;
        };
        if self.telegram_account.as_ref().map(|current| current.id) != Some(account)
            || self.storage_channel_id() != Some(chat)
        {
            return false;
        }
        self.vault_key_selection_scope = None;
        true
    }

    fn finish_managed_key_operation(
        &mut self,
        admit: impl FnOnce(
            &Self,
        ) -> Result<
            teleark_runtime::VaultJob<teleark_runtime::VaultRecoverySecret>,
            ApplicationError,
        >,
        progress: teleark_runtime::VaultKeyProgress,
        reveal_recovery: bool,
        cx: &mut Context<Self>,
    ) {
        let refresh_files = !reveal_recovery || self.vault_status.active_key_locked;
        let mut changes = progress.subscribe();
        self.vault_key_progress = Some(progress);
        self.vault_key_details = false;
        self.vault_activity = VaultActivity::Working;
        self.vault_key_presentation = Some(cx.spawn(async move |this, cx| {
            loop {
                let event = tokio::select! {
                    changed = changes.changed() => if changed.is_err() { return; } else { true },
                    () = cx.background_executor().timer(Duration::from_secs(1)) => false,
                };
                if event {
                    cx.background_executor()
                        .timer(Duration::from_millis(100))
                        .await;
                }
                let Some(entity) = this.upgrade() else { return };
                if entity.update(cx, |this, cx| {
                    cx.notify();
                    this.vault_key_progress
                        .as_ref()
                        .is_none_or(|p| p.snapshot().finished)
                }) {
                    return;
                }
            }
        }));
        let generation = (
            self.telegram_login_generation,
            self.vault_session_generation,
        );
        cx.notify();
        // Publish acknowledgment before the owner can begin crypto or Keychain I/O.
        let job = admit(self);
        let work = cx.background_spawn(async move { job.and_then(|job| job.wait()) });
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
                this.vault_key_presentation = None;
                match result {
                    Ok(secret) => {
                        this.vault_activity = VaultActivity::Succeeded;
                        this.vault_new_epoch_confirmation = false;
                        if reveal_recovery && !secret.is_empty() && !this.app_lock.locked {
                            this.vault_recovery_secret = Some(secret);
                            this.export_vault_recovery_key(cx);
                        }
                        this.sync_vault_status();
                        if refresh_files {
                            this.vault_key_selection_scope = None;
                            this.apply_managed_channel_changes(cx);
                        }
                    }
                    Err(error) => {
                        this.vault_activity = VaultActivity::Failed(error.kind());
                        this.pending_vault_action = None;
                        this.sync_vault_status();
                    }
                }
                cx.notify();
            });
        }));
    }

    pub(crate) fn clear_vault_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.vault_recovery_key
            .update(cx, |input, cx| input.set_value("", window, cx));
    }

    pub(crate) fn dismiss_unlock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.clear_vault_inputs(window, cx);
        self.pending_vault_action = None;
        self.vault_new_epoch_confirmation = false;
        self.vault_key_progress = None;
        self.vault_key_details = false;
        self.hide_vault_recovery_key(cx);
        self.vault_activity = VaultActivity::Idle;
        cx.notify();
    }

    pub(crate) fn resume_pending_vault_action(&mut self, cx: &mut Context<Self>) {
        if self.vault_locked
            || (self.pending_vault_action == Some(VaultAction::Upload)
                && self.vault_status.active_key_locked)
        {
            return;
        }
        match self.pending_vault_action.take() {
            Some(VaultAction::Upload) => {
                self.show_upload = true;
                self.upload_queued = false;
            }
            Some(VaultAction::QueueUpload) => self.enqueue_vault_upload(cx),
            Some(VaultAction::Download(id)) => self.download_managed_vault_file(id, cx),
            None => {}
        }
        cx.notify();
    }

    pub(crate) fn initialize_vault(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.vault_new_epoch_confirmation || self.vault_activity == VaultActivity::Working {
            return;
        }
        let Some(vault) = self.vault.clone() else {
            return;
        };
        let progress = teleark_runtime::VaultKeyProgress::new();
        let admission_progress = progress.clone();
        self.finish_managed_key_operation(
            move |_| vault.submit_new_managed_key(admission_progress),
            progress,
            false,
            cx,
        );
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
        let progress = teleark_runtime::VaultKeyProgress::new();
        let admission_progress = progress.clone();
        self.finish_managed_key_operation(
            move |_| vault.submit_recovery_import(recovery, admission_progress),
            progress,
            false,
            cx,
        );
    }

    pub(crate) fn show_vault_recovery_key(&mut self, cx: &mut Context<Self>) {
        if self.vault_activity == VaultActivity::Working {
            return;
        }
        let Some(vault) = self.vault.clone() else {
            return;
        };
        let progress = teleark_runtime::VaultKeyProgress::new();
        let admission_progress = progress.clone();
        self.finish_managed_key_operation(
            move |_| vault.submit_recovery_export(admission_progress),
            progress,
            true,
            cx,
        );
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
                    this.apply_managed_channel_changes(cx);
                    this.vault_recovery_secret = None;
                    this.recovery_visible = false;
                    this.resume_pending_vault_action(cx);
                }
                cx.notify();
            });
        }));
    }

    pub(crate) fn lock_vault(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.vault_session_generation = self.vault_session_generation.wrapping_add(1);
        if let Some(progress) = &self.vault_key_progress {
            progress.cancel();
        }
        self.vault_key_progress = None;
        self.vault_key_details = false;
        self.vault_key_presentation = None;
        self.vault_key_selection_task = None;
        self.vault_key_selection_scope = None;
        self.vault_key_reselection_pending = None;
        self.clear_vault_inputs(window, cx);
        self.pending_vault_action = None;
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
        self.managed_projection_scope = None;
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

    pub(super) fn resume_durable_uploads(&mut self, cx: &mut Context<Self>) {
        if self.visual_preview || self.vault_locked || self.vault_recovery_scope.is_some() {
            return;
        }
        let Some(account_id) = self.telegram_account.as_ref().map(|account| account.id) else {
            return;
        };
        let Some(vault) = self.vault.clone() else {
            return;
        };
        let scope = (
            self.telegram_login_generation,
            self.vault_session_generation,
        );
        let job = vault.submit_resume_queued_transfers(account_id);
        self.vault_recovery_scope = Some(scope);
        self.vault_recovery_task = Some(cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { job?.wait() }).await;
            let Some(this) = this.upgrade() else {
                return;
            };
            this.update(cx, |this, cx| {
                if this.vault_recovery_scope != Some(scope) {
                    return;
                }
                this.vault_recovery_scope = None;
                this.vault_recovery_task = None;
                if scope
                    != (
                        this.telegram_login_generation,
                        this.vault_session_generation,
                    )
                {
                    this.resume_durable_uploads(cx);
                    return;
                }
                if let Err(error) = result {
                    this.vault_activity = VaultActivity::Failed(error.kind());
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
        let chat = self.storage_channel_id();
        let (Some(vault), Some(chat_id)) = (self.vault.clone(), chat) else {
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
        self.managed_projection_scope = Some((account_id, chat_id));
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
        let observer = self
            .channel_sync
            .as_ref()
            .map(|sync| sync.observe_managed_scan(chat_id));
        cx.notify();
        let mode = if verify_health {
            teleark_runtime::ManagedScanMode::CheckHealth
        } else {
            teleark_runtime::ManagedScanMode::Cached
        };
        let work = if !verify_health {
            cx.background_spawn(async move {
                vault.synchronize_managed_files(account_id, chat_id, cancellation, observer)
            })
        } else {
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
            cx.background_spawn(async move { job.wait() })
        };
        self.vault_scan_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| {
                if this.managed_scan_generation != generation {
                    return;
                }
                this.managed_scan_loading = false;
                this.managed_scan_cancellation = None;
                if this.telegram_account.as_ref().map(|a| a.id) != Some(account_id)
                    || this.storage_channel_id() != Some(chat_id)
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
                        if verify_health {
                            this.vault_activity = VaultActivity::Succeeded;
                        }
                    }
                    Err(error) => {
                        if verify_health {
                            this.vault_activity = VaultActivity::Failed(error.kind());
                        }
                    }
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
            self.open_vault_action(VaultAction::Download(package_id), cx);
            return;
        }
        let (Some(vault), Some(chat_id)) = (self.vault.clone(), self.active_storage_chat_id())
        else {
            return;
        };
        if let Some(file) = self.managed_vault_files.iter().find(|file| {
            file.package_numeric_id == package_id
                && file.health == teleark_runtime::VaultFileHealth::PendingUpload
        }) {
            self.resume_pending_remote_file(file.manifest_message_id, cx);
            return;
        }
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
                this.advance_transition(cx);
                let key_activity = this.vault_activity;
                this.vault_activity = match result {
                    Ok(_) => VaultActivity::Succeeded,
                    Err(error) if error.kind() == teleark_core::ApplicationErrorKind::Cancelled => {
                        VaultActivity::Idle
                    }
                    Err(error) => VaultActivity::Failed(error.kind()),
                };
                if key_activity == VaultActivity::Working {
                    this.vault_activity = key_activity;
                }
                cx.notify();
            });
        }));
    }

    fn resume_pending_remote_file(&mut self, message_id: i64, cx: &mut Context<Self>) {
        let (Some(vault), Some(chat_id), Some(account_id)) = (
            self.vault.clone(),
            self.active_storage_chat_id(),
            self.telegram_account.as_ref().map(|a| a.id),
        ) else {
            return;
        };
        let selected = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(self.tr("vault-pending-select-source")),
        });
        let generation = self.telegram_login_generation;
        self.vault_download_in_flight = true;
        self.vault_activity = VaultActivity::Working;
        cx.notify();
        self.vault_download_task = Some(cx.spawn(async move |this, cx| {
            let source = match selected.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                _ => None,
            };
            let Some(source) = source else {
                let _ = this.update(cx, |this, cx| {
                    this.vault_download_in_flight = false;
                    this.vault_activity = VaultActivity::Idle;
                    cx.notify();
                });
                return;
            };
            let current = this
                .update(cx, |this, _| this.telegram_login_generation == generation)
                .unwrap_or(false);
            if !current {
                return;
            }
            let result = cx
                .background_spawn(async move {
                    vault
                        .submit_resume_remote_upload(account_id, chat_id, message_id, source)?
                        .wait()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.telegram_login_generation != generation {
                    return;
                }
                this.vault_download_in_flight = false;
                this.vault_activity = match result {
                    Ok(file) => {
                        let files = std::sync::Arc::make_mut(&mut this.managed_vault_files);
                        files.retain(|existing| {
                            existing.package_numeric_id != file.package_numeric_id
                        });
                        files.insert(0, file);
                        VaultActivity::Succeeded
                    }
                    Err(error) => VaultActivity::Failed(error.kind()),
                };
                this.advance_transition(cx);
                cx.notify();
            });
        }));
    }

    fn start_upload_progress_presentation(&mut self, preparation: bool, cx: &mut Context<Self>) {
        // Presentation only: display elapsed phase time and cancellation while
        // the retained worker owns filesystem/network work.
        let task = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let Some(this) = this.upgrade() else { return };
                let done = this.update(cx, |this, cx| {
                    cx.notify();
                    if preparation {
                        !this.upload_preparing
                    } else {
                        !this.upload_in_flight
                    }
                });
                if done {
                    return;
                }
            }
        });
        if preparation {
            self.upload_preparation_presentation = Some(task);
        } else {
            self.upload_selection_presentation = Some(task);
        }
    }

    pub(crate) fn remove_upload_source(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.upload_preparing || index >= self.upload_sources.len() {
            return;
        }
        let removed = self.upload_sources.remove(index);
        self.upload_source_total_bytes = self
            .upload_source_total_bytes
            .saturating_sub(removed.size_bytes);
        self.upload_draft_generation = self.upload_draft_generation.wrapping_add(1);
        cx.notify();
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
        self.prepare_upload_selection(
            async move {
                match selected.await {
                    Ok(Ok(paths)) => Ok(paths),
                    Ok(Err(_)) | Err(_) => Err(ApplicationError::new(
                        teleark_core::ApplicationErrorKind::PermissionDenied,
                    )),
                }
            },
            false,
            cx,
        );
    }

    pub(crate) fn can_accept_upload_files(&self) -> bool {
        self.show_upload && !self.upload_preparing && self.vault_activity != VaultActivity::Working
    }

    pub(crate) fn drop_upload_files(
        &mut self,
        paths: &[std::path::PathBuf],
        cx: &mut Context<Self>,
    ) {
        if !self.can_accept_upload_files() || paths.is_empty() {
            return;
        }
        let paths = paths.to_vec();
        self.prepare_upload_selection(async move { Ok(Some(paths)) }, true, cx);
    }

    fn prepare_upload_selection(
        &mut self,
        selected: impl std::future::Future<
            Output = Result<Option<Vec<std::path::PathBuf>>, ApplicationError>,
        > + 'static,
        append: bool,
        cx: &mut Context<Self>,
    ) {
        self.upload_preparing = true;
        self.upload_draft_generation = self.upload_draft_generation.wrapping_add(1);
        self.vault_activity = VaultActivity::Idle;
        let generation = self.telegram_login_generation;
        let progress = teleark_runtime::VaultUploadSelectionProgress::new(0);
        self.upload_preparation_progress = Some(progress.clone());
        self.start_upload_progress_presentation(true, cx);
        let existing = if append {
            self.upload_sources
                .iter()
                .map(|source| source.path.clone())
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        // Acknowledgment is rendered before awaiting the picker or touching the filesystem.
        cx.notify();
        self.upload_picker_task = Some(cx.spawn(async move |this, cx| {
            let result = match selected.await {
                Ok(Some(paths)) => Some(
                    cx.background_spawn(async move {
                        let mut combined = existing;
                        combined.extend(paths);
                        // Exact duplicates need no extra filesystem work and should
                        // not be added again on a repeated drop.
                        let mut seen = std::collections::BTreeSet::new();
                        combined.retain(|path| seen.insert(path.clone()));
                        teleark_runtime::inspect_upload_sources_observed(&combined, Some(&progress))
                            .map(|sources| {
                                let total_bytes =
                                    sources.iter().map(|source| source.size_bytes).sum::<u64>();
                                (sources, total_bytes)
                            })
                    })
                    .await,
                ),
                Ok(None) => None,
                Err(error) => Some(Err(error)),
            };
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| {
                this.upload_preparing = false;
                this.advance_transition(cx);
                this.upload_preparation_presentation = None;
                if let Some(progress) = &this.upload_preparation_progress {
                    progress.finish(
                        result
                            .as_ref()
                            .and_then(|r| r.as_ref().err())
                            .map(ApplicationError::kind),
                    );
                }
                if generation != this.telegram_login_generation {
                    cx.notify();
                    return;
                }
                if let Some(result) = result {
                    match result {
                        Ok((sources, total_bytes)) => {
                            this.upload_source_total_bytes = total_bytes;
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
            if self.managed_vault_files.len() > 1_000 {
                std::sync::Arc::make_mut(&mut self.managed_vault_files).truncate(1_000);
                self.managed_catalog_limited = true;
            }
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
            self.upload_source_total_bytes = self
                .upload_sources
                .iter()
                .map(|source| source.size_bytes)
                .sum();
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
            self.pending_vault_action = Some(VaultAction::QueueUpload);
            self.prepare_vault_key(cx);
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
        let progress =
            teleark_runtime::VaultUploadSelectionProgress::new(self.upload_sources.len());
        self.upload_selection_progress = Some(progress.clone());
        self.start_upload_progress_presentation(false, cx);
        let job = match vault.submit_upload_files_observed(
            account_id,
            chat_id,
            sources,
            progress.clone(),
        ) {
            Ok(job) => job,
            Err(error) => {
                self.upload_in_flight = false;
                self.upload_selection_presentation = None;
                progress.finish(Some(error.kind()));
                self.vault_activity = VaultActivity::Failed(error.kind());
                cx.notify();
                return;
            }
        };
        let work = cx.background_spawn(async move {
            job.wait().map(|report| {
                let retry_sources = report
                    .failed
                    .iter()
                    .map(|failure| failure.source.clone())
                    .chain(report.cancelled.iter().cloned())
                    .collect::<std::collections::BTreeSet<_>>();
                (report, retry_sources)
            })
        });
        self.vault_upload_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            // Also closes the progress owner if session admission rejected the
            // command before the worker could run it.
            let error = result
                .as_ref()
                .err()
                .map(ApplicationError::kind)
                .or_else(|| {
                    result.as_ref().ok().and_then(|(report, _)| {
                        report.failed.first().map(|f| f.kind).or_else(|| {
                            (!report.cancelled.is_empty())
                                .then_some(teleark_core::ApplicationErrorKind::Cancelled)
                        })
                    })
                });
            progress.finish(error);
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| {
                if this.telegram_login_generation != login_generation
                    || this.telegram_account.as_ref().map(|account| account.id) != Some(account_id)
                {
                    return;
                }
                this.upload_in_flight = false;
                this.advance_transition(cx);
                this.upload_selection_presentation = None;
                let key_activity = this.vault_activity;
                match result {
                    Ok((report, retry_sources)) => {
                        let receipts_limited = report.completed_count > report.completed.len();
                        this.apply_completed_vault_uploads(account_id, chat_id, report.completed);
                        if receipts_limited {
                            this.scan_managed_vault_files(cx);
                        }
                        this.retain_upload_retry_sources(draft_generation, &retry_sources);
                        this.vault_activity = report.failed.first().map_or_else(
                            || {
                                if report.paused_count > 0 && report.cancelled.is_empty() {
                                    VaultActivity::Idle
                                } else if report.cancelled.is_empty() {
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
    fn completed_channel_setup_rechecks_key_at_the_same_catalog_revision(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Storage);
        app.update(cx, |app, cx| {
            let account = app.telegram_account.as_ref().expect("preview account").id;
            let chat = app.storage_channel_id().expect("preview channel");
            let old_scope = Some((account, chat, 7));
            app.vault_key_selection_scope = old_scope;
            app.request_channel_key_reselection(account, chat);
            assert_eq!(app.vault_key_selection_scope, None);
            assert_eq!(app.vault_key_reselection_pending, None);

            app.vault_key_selection_scope = old_scope;
            app.vault_key_selection_task = Some(cx.spawn(async move |_, _| {
                std::future::pending::<()>().await;
            }));
            app.request_channel_key_reselection(account, chat);
            assert_eq!(app.vault_key_selection_scope, old_scope);
            assert_eq!(app.vault_key_reselection_pending, Some((account, chat)));
            app.vault_key_selection_task = None;
            assert!(app.finish_pending_channel_key_reselection());
            assert_eq!(app.vault_key_selection_scope, None);

            app.vault_key_selection_scope = old_scope;
            app.request_channel_key_reselection(account + 1, chat);
            assert_eq!(app.vault_key_selection_scope, old_scope);
            assert_eq!(app.vault_key_reselection_pending, None);
        });
    }

    #[gpui::test]
    fn key_readiness_never_gates_pages_or_the_upload_picker(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Storage);
        for (width, height) in [(900.0, 600.0), (1440.0, 900.0)] {
            cx.simulate_resize(gpui_kit::size(px(width), px(height)));
            for configured in [false, true] {
                for page in [
                    Page::Storage,
                    Page::Channel,
                    Page::Transfers,
                    Page::Settings,
                ] {
                    app.update(cx, |app, cx| {
                        app.app_lock.locked = false;
                        app.vault_locked = true;
                        app.vault_status.configured = configured;
                        app.vault_status.active_key_locked = true;
                        app.vault_activity = VaultActivity::Working;
                        app.page = page;
                        app.settings_section = SettingsSection::KeyVault;
                        app.vault_advanced_expanded = true;
                        app.show_upload = false;
                        app.pending_vault_action = Some(VaultAction::QueueUpload);
                        cx.notify();
                    });
                    cx.run_until_parked();
                    assert!(cx.debug_bounds("storage-locked-viewport").is_none());
                    assert!(cx.debug_bounds("unlock-popup").is_none());
                    assert!(cx.debug_bounds("new-key-confirmation").is_none());
                    app.update(cx, |app, cx| app.open_vault_action(VaultAction::Upload, cx));
                    cx.run_until_parked();
                    let queue = cx
                        .debug_bounds("upload-add-queue")
                        .expect("normal queue action");
                    assert!(queue.bottom() <= px(height));
                    app.update(cx, |app, _| {
                        assert_eq!(app.page, page);
                        assert!(app.show_upload);
                        assert!(!app.upload_in_flight);
                        assert!(!app.vault_new_epoch_confirmation);
                    });
                }
            }
        }
    }

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
            // Interface verification uses English only, including wrapping checks.
            {
                let locale = SupportedLocale::EnUs;
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

    fn send_file_drop(cx: &mut gpui::VisualTestContext, paths: Vec<std::path::PathBuf>) {
        use gpui::InputEvent as _;
        let position = cx
            .debug_bounds("upload-drop-target")
            .expect("drop target")
            .center();
        cx.update(|window, cx| {
            window.dispatch_event(
                gpui::FileDropEvent::Entered {
                    position,
                    paths: gpui::ExternalPaths(paths.into_iter().collect()),
                }
                .to_platform_input(),
                cx,
            );
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.dispatch_event(
                gpui::FileDropEvent::Submit { position }.to_platform_input(),
                cx,
            );
        });
        cx.run_until_parked();
    }

    #[gpui::test]
    fn native_file_drop_adds_deduplicates_and_preserves_selection_on_invalid_input(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Storage);
        app.update(cx, |app, cx| {
            app.show_upload = true;
            app.upload_sources.clear();
            cx.notify();
        });
        cx.run_until_parked();
        let source = std::env::current_exe().expect("synthetic executable");
        send_file_drop(cx, vec![source.clone(), source.clone()]);
        app.read_with(cx, |app, _| {
            assert_eq!(app.upload_sources.len(), 1);
            assert_eq!(app.upload_sources[0].path, source);
            assert!(!app.upload_preparing);
            assert!(!app.upload_in_flight, "drop only prepares the composer");
        });
        send_file_drop(cx, vec![source.clone()]);
        app.read_with(cx, |app, _| assert_eq!(app.upload_sources.len(), 1));
        send_file_drop(cx, vec![source.parent().expect("directory").to_path_buf()]);
        app.read_with(cx, |app, _| {
            assert_eq!(app.upload_sources.len(), 1);
            assert_eq!(
                app.vault_activity,
                VaultActivity::Failed(teleark_core::ApplicationErrorKind::UploadFolderUnsupported),
            );
        });
        send_file_drop(cx, vec![source; 4096]);
        app.read_with(cx, |app, _| {
            assert_eq!(app.upload_sources.len(), 1);
            assert_eq!(app.vault_activity, VaultActivity::Idle);
        });
    }

    #[gpui::test]
    fn pending_file_selection_acknowledges_and_rejects_overlap_and_stale_account(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Storage);
        let source = std::env::current_exe().expect("synthetic executable");
        app.update(cx, |app, cx| {
            app.show_upload = true;
            app.upload_sources.clear();
            let timer = cx.background_executor().timer(Duration::from_secs(5));
            let source = source.clone();
            app.prepare_upload_selection(
                async move {
                    timer.await;
                    Ok(Some(vec![source]))
                },
                true,
                cx,
            );
            assert!(app.upload_preparing, "feedback precedes the wait");
            assert!(!app.can_accept_upload_files());
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("upload-drop-feedback").is_some());
        app.update(cx, |app, cx| {
            app.drop_upload_files(std::slice::from_ref(&source), cx);
            assert!(app.upload_sources.is_empty());
            app.telegram_login_generation = app.telegram_login_generation.wrapping_add(1);
        });
        cx.background_executor.advance_clock(Duration::from_secs(5));
        cx.run_until_parked();
        app.read_with(cx, |app, _| {
            assert!(!app.upload_preparing);
            assert!(
                app.upload_sources.is_empty(),
                "previous account callback rejected"
            );
        });
    }

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
    fn late_managed_key_callback_cannot_reopen_upload_or_replace_logout_status(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Storage);
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.pending_vault_action = Some(VaultAction::Upload);
                app.vault_activity = VaultActivity::Working;
                app.finish_managed_key_operation(
                    |view| {
                        assert_eq!(view.vault_activity, VaultActivity::Working);
                        assert!(view.vault_key_progress.is_some());
                        Err(ApplicationError::new(
                            teleark_core::ApplicationErrorKind::PermissionDenied,
                        ))
                    },
                    teleark_runtime::VaultKeyProgress::new(),
                    false,
                    cx,
                );
                app.lock_vault(window, cx);
            })
        });
        cx.run_until_parked();
        app.update(cx, |app, _| {
            assert!(app.vault_locked);
            assert!(!app.show_upload);
            assert!(app.pending_vault_action.is_none());
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
                app.vault_recovery_key.update(cx, |input, cx| {
                    input.set_value("synthetic password", window, cx)
                });
                let generation = app.vault_session_generation;
                app.lock_vault(window, cx);
                assert!(app.vault_locked);
                assert!(app.vault_status.active_key_locked);
                assert!(app.managed_vault_files.is_empty());
                assert!(app.vault_recovery_key.read(cx).value().is_empty());
                assert!(app.vault_upload_task.is_some());
                assert!(app.upload_in_flight);
                assert!(app.managed_scan_loading);
                assert_ne!(app.vault_session_generation, generation);
            })
        });
        assert!(!cancellation.is_cancelled());
        cx.run_until_parked();
        assert!(cx.debug_bounds("managed-key-status-notice").is_some());
    }

    #[gpui::test]
    fn upload_picker_opens_without_a_key_or_pin_and_keeps_the_draft(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Settings);
        app.update(cx, |app, _| {
            app.vault_locked = false;
            app.vault_status.active_key_locked = false;
            app.show_upload = false;
            assert_eq!(app.page, Page::Settings);
            assert!(!app.show_upload);
            assert!(app.pending_vault_action.is_none());
            app.upload_sources =
                teleark_runtime::inspect_upload_sources(&[
                    std::env::current_exe().expect("synthetic test executable")
                ])
                .expect("test source");
        });
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.lock_vault(window, cx);
                app.open_vault_action(VaultAction::Upload, cx);
                assert!(app.show_upload);
                assert!(!app.vault_new_epoch_confirmation);
                assert!(app.pending_vault_action.is_none());
                assert_eq!(app.upload_sources.len(), 1);
                app.vault_locked = false;
                app.vault_status.active_key_locked = false;
                app.resume_pending_vault_action(cx);
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
