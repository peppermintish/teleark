//! Auth presentation owner. Business operations stay in the runtime.

use super::*;

impl TeleArkApp {
    pub(crate) fn reset_telegram_login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.telegram_login_generation = self.telegram_login_generation.wrapping_add(1);
        self.qr_poll_task = None;
        self.telegram_auth = TelegramAuthState::Unauthorized;
        self.telegram_activity = TelegramActivity::Idle;
        for input in [&self.telegram_code, &self.telegram_password] {
            input.update(cx, |input, cx| input.set_value("", window, cx));
        }
        cx.notify();
    }

    pub(crate) fn switch_telegram_account(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.telegram_activity == TelegramActivity::Working {
            return;
        }
        if self.vault.as_ref().is_some_and(|vault| {
            vault
                .transfers()
                .iter()
                .any(|transfer| transfer.state == teleark_runtime::VaultTransferState::Running)
        }) {
            self.show_account_switch = true;
            cx.notify();
            return;
        }
        self.telegram_login_generation = self.telegram_login_generation.wrapping_add(1);
        self.transfers_account_ready = false;
        self.cancel_managed_scan();
        self.cancel_telegram_file_load(cx);
        self.clear_vault_inputs(window, cx);
        self.qr_poll_task = None;
        let Some(telegram) = self.telegram.clone() else {
            if self.visual_preview {
                self.telegram_account = None;
                self.account_avatar = None;
                self.reset_telegram_login(window, cx);
            }
            return;
        };
        let transfers = self.transfers.clone();
        let vault = self.vault.clone();
        self.telegram_activity = TelegramActivity::Working;
        self.show_account_switch = false;
        let work = cx.background_spawn(async move {
            if let Some(transfers) = transfers {
                transfers.suspend_account()?;
            }
            if let Some(vault) = vault {
                vault.lock()?;
            }
            telegram.sign_out()
        });
        self.telegram_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            if let Some(this) = this.upgrade() {
                this.update(cx, |this, cx| {
                    match result {
                        Ok(()) => {
                            this.telegram_auth = TelegramAuthState::Unauthorized;
                            this.telegram_activity = TelegramActivity::Idle;
                            this.telegram_account = None;
                            this.account_avatar = None;
                            this.telegram_chats.clear();
                            this.telegram_files.clear();
                            this.telegram_index = None;
                            this.selected_chat_id = None;
                            this.storage_status = teleark_runtime::StorageChannelStatus::Missing;
                            this.storage_loading = false;
                            this.storage_error = None;
                            this.managed_vault_files.clear();
                            this.managed_vault_rejected = 0;
                            this.vault_recovery_secret = None;
                            this.vault_locked = true;
                            this.unlock_intent = None;
                            this.upload_source = None;
                            this.telegram_file_generation =
                                this.telegram_file_generation.wrapping_add(1);
                            this.refresh_channel_file_table(cx);
                        }
                        Err(error) => {
                            this.telegram_activity = TelegramActivity::Failed(error.kind())
                        }
                    }
                    cx.notify();
                });
            }
        }));
        cx.notify();
    }

    pub(crate) fn begin_telegram_login(&mut self, cx: &mut Context<Self>) {
        if self.telegram_activity == TelegramActivity::Working {
            return;
        }
        let Some(telegram) = self.telegram.clone() else {
            self.telegram_activity =
                TelegramActivity::Failed(teleark_core::ApplicationErrorKind::Persistence);
            cx.notify();
            return;
        };
        let Some(library) = self.library.clone() else {
            self.telegram_activity =
                TelegramActivity::Failed(teleark_core::ApplicationErrorKind::Persistence);
            cx.notify();
            return;
        };
        let phone = self.telegram_phone.read(cx).value().to_string();
        self.telegram_activity = TelegramActivity::Working;
        cx.notify();
        let work = cx.background_spawn(async move {
            match telegram.connect_configured(&library)? {
                TelegramAuthState::Authorized(account) => {
                    Ok::<_, ApplicationError>(TelegramAuthState::Authorized(account))
                }
                TelegramAuthState::Unauthorized => {
                    telegram.request_login_code_configured(&library, phone)
                }
                _ => Err(ApplicationError::new(
                    teleark_core::ApplicationErrorKind::Conflict,
                )),
            }
        });
        self.telegram_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| this.apply_telegram_auth_result(result, cx));
        }));
    }

    pub(crate) fn begin_telegram_qr_login(&mut self, cx: &mut Context<Self>) {
        if self.telegram_activity == TelegramActivity::Working {
            return;
        }
        let Some(telegram) = self.telegram.clone() else {
            self.telegram_activity =
                TelegramActivity::Failed(teleark_core::ApplicationErrorKind::Persistence);
            cx.notify();
            return;
        };
        let Some(library) = self.library.clone() else {
            self.telegram_activity =
                TelegramActivity::Failed(teleark_core::ApplicationErrorKind::Persistence);
            cx.notify();
            return;
        };
        self.telegram_login_generation = self.telegram_login_generation.wrapping_add(1);
        let generation = self.telegram_login_generation;
        self.telegram_activity = TelegramActivity::Working;
        cx.notify();
        let work = cx.background_spawn(async move {
            match telegram.connect_configured(&library)? {
                TelegramAuthState::Authorized(account) => {
                    Ok::<_, ApplicationError>(TelegramAuthState::Authorized(account))
                }
                TelegramAuthState::Unauthorized => telegram.begin_qr_login_configured(&library),
                _ => Err(ApplicationError::new(
                    teleark_core::ApplicationErrorKind::Conflict,
                )),
            }
        });
        self.telegram_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| {
                if this.telegram_login_generation == generation {
                    this.apply_telegram_auth_result(result, cx);
                }
            });
        }));
    }

    pub(crate) fn refresh_telegram_qr_login(&mut self, cx: &mut Context<Self>) {
        if self.telegram_activity == TelegramActivity::Working {
            return;
        }
        let Some(telegram) = self.telegram.clone() else {
            return;
        };
        let Some(library) = self.library.clone() else {
            self.telegram_activity =
                TelegramActivity::Failed(teleark_core::ApplicationErrorKind::Persistence);
            cx.notify();
            return;
        };
        self.telegram_activity = TelegramActivity::Working;
        cx.notify();
        let work = cx.background_spawn(async move { telegram.begin_qr_login_configured(&library) });
        self.telegram_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| this.apply_telegram_auth_result(result, cx));
        }));
    }

    pub(crate) fn submit_telegram_code(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let code = self.telegram_code.read(cx).value().to_string();
        self.telegram_code
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.submit_telegram_secret(code.into_bytes(), false, cx);
    }

    pub(crate) fn submit_telegram_password(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let password = self.telegram_password.read(cx).value().to_string();
        self.telegram_password
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.submit_telegram_secret(password.into_bytes(), true, cx);
    }

    pub(super) fn submit_telegram_secret(
        &mut self,
        secret: Vec<u8>,
        password: bool,
        cx: &mut Context<Self>,
    ) {
        if self.telegram_activity == TelegramActivity::Working {
            return;
        }
        let Some(telegram) = self.telegram.clone() else {
            return;
        };
        self.telegram_activity = TelegramActivity::Working;
        cx.notify();
        let work = cx.background_spawn(async move {
            if password {
                telegram.submit_password(secret)
            } else {
                telegram.submit_code(String::from_utf8_lossy(&secret).into_owned())
            }
        });
        self.telegram_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| this.apply_telegram_auth_result(result, cx));
        }));
    }

    pub(super) fn apply_telegram_auth_result(
        &mut self,
        result: Result<TelegramAuthState, ApplicationError>,
        cx: &mut Context<Self>,
    ) {
        let restoring = self.account_restoring;
        self.account_restoring = false;
        match result {
            Ok(state) => {
                self.telegram_activity = TelegramActivity::Idle;
                if let TelegramAuthState::Authorized(account) = &state {
                    self.telegram_account = Some(account.clone());
                }
                self.telegram_auth = state;
                if matches!(self.telegram_auth, TelegramAuthState::QrCode { .. }) {
                    self.schedule_qr_login_poll(cx);
                } else if matches!(self.telegram_auth, TelegramAuthState::Authorized(_)) {
                    self.qr_poll_task = None;
                    self.load_account_avatar(cx);
                    if !restoring {
                        self.enter_workspace(cx);
                    }
                }
            }
            Err(error) => self.telegram_activity = TelegramActivity::Failed(error.kind()),
        }
        cx.notify();
    }

    pub(super) fn schedule_qr_login_poll(&mut self, cx: &mut Context<Self>) {
        let Some(telegram) = self.telegram.clone() else {
            return;
        };
        let generation = self.telegram_login_generation;
        self.qr_poll_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(750))
                .await;
            let poll = cx.background_spawn(async move { telegram.poll_qr_login() });
            let result = poll.await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| {
                if this.telegram_login_generation == generation
                    && matches!(this.telegram_auth, TelegramAuthState::QrCode { .. })
                {
                    this.apply_telegram_auth_result(result, cx);
                }
            });
        }));
    }

    pub(crate) fn load_telegram_dialogs(&mut self, cx: &mut Context<Self>) {
        if self.telegram_activity == TelegramActivity::Working {
            return;
        }
        let (Some(telegram), Some(library), Some(account)) = (
            self.telegram.clone(),
            self.library.clone(),
            self.telegram_account.clone(),
        ) else {
            return;
        };
        let account_id = account.id;
        let generation = self.telegram_login_generation;
        let transfers = self.transfers.clone();
        self.telegram_activity = TelegramActivity::Working;
        let work = cx.background_spawn(async move {
            let chats = telegram.list_dialogs(account_id)?;
            library.save_telegram_sources(&account, &chats)?;
            if let Some(transfers) = transfers {
                transfers.activate_account(account_id)?;
            }
            Ok::<_, ApplicationError>(chats)
        });
        self.telegram_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| {
                if this.telegram_login_generation != generation
                    || this.telegram_account.as_ref().map(|a| a.id) != Some(account_id)
                {
                    return;
                }
                match result {
                    Ok(chats) => {
                        // Refresh data without resetting the route, selection,
                        // scroll owner, filters, or a populated file table.
                        this.telegram_chats = chats;
                        this.transfers_account_ready = true;
                        this.telegram_activity = TelegramActivity::Idle;
                        if this.page == Page::Channel
                            && this.selected_chat_id.is_none()
                            && let Some(chat) = this
                                .telegram_chats
                                .iter()
                                .find(|chat| chat.kind == TelegramChatKind::Channel)
                        {
                            this.select_telegram_chat(chat.id, cx);
                        }
                        this.refresh_storage_channel(cx);
                    }
                    Err(error) => this.telegram_activity = TelegramActivity::Failed(error.kind()),
                }
                cx.notify();
            });
        }));
    }

    pub(super) fn restore_telegram_session(&mut self, cx: &mut Context<Self>) {
        if self.visual_preview || self.configured_telegram_api_id.is_none() {
            return;
        }
        let (Some(telegram), Some(library)) = (self.telegram.clone(), self.library.clone()) else {
            return;
        };
        self.account_restoring = true;
        self.telegram_activity = TelegramActivity::Working;
        let work = cx.background_spawn(async move { telegram.connect_configured(&library) });
        self.telegram_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            if let Some(this) = this.upgrade() {
                this.update(cx, |this, cx| this.apply_telegram_auth_result(result, cx));
            }
        }));
    }

    pub(crate) fn enter_workspace(&mut self, cx: &mut Context<Self>) {
        self.select_storage(StorageView::Files, cx);
        if !self.visual_preview {
            self.load_telegram_dialogs(cx);
        }
    }

    pub(super) fn load_account_avatar(&mut self, cx: &mut Context<Self>) {
        let Some(telegram) = self.telegram.clone() else {
            return;
        };
        let Some(account_id) = self.telegram_account.as_ref().map(|account| account.id) else {
            return;
        };
        let work = cx.background_spawn(async move { telegram.account_avatar(account_id) });
        self.avatar_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            if let Some(this) = this.upgrade() {
                this.update(cx, |this, cx| {
                    if this.telegram_account.as_ref().map(|account| account.id) != Some(account_id)
                    {
                        return;
                    }
                    if let Ok(Some(bytes)) = result {
                        this.account_avatar = Some(std::sync::Arc::new(
                            gpui_kit::Image::from_bytes(gpui_kit::ImageFormat::Jpeg, bytes),
                        ));
                        cx.notify();
                    }
                });
            }
        }));
    }
}
