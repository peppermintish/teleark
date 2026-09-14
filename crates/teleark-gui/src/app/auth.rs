//! Auth presentation owner. Business operations stay in the runtime.

use super::*;

impl TeleArkApp {
    pub(crate) fn telegram_is_authorized(&self) -> bool {
        matches!(&self.telegram_auth, TelegramAuthState::Authorized(account)
            if self.telegram_account.as_ref().is_some_and(|current| current.id == account.id))
    }

    pub(super) fn render_signed_out_screen(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let layout = LayoutPolicy::from_window(window);
        if window.focused(cx).is_none() {
            window.focus(&self.main_focus, cx);
        }
        div()
            .debug_selector(|| "signed-out-page".into())
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(theme::canvas())
            .text_color(theme::text_primary())
            .font_family(".SystemUIFont")
            .track_focus(&self.main_focus)
            .on_action(cx.listener(|app, _: &DismissOverlay, window, cx| {
                if app.transition.is_some() {
                    app.cancel_transition(cx);
                } else if app.show_telegram_api_id_prompt {
                    app.skip_telegram_api_id_prompt(cx);
                } else if app.login_proxy_open {
                    app.login_proxy_open = false;
                    cx.notify();
                } else if window.is_fullscreen() {
                    window.toggle_fullscreen();
                }
            }))
            .on_action(cx.listener(|_, _: &ShowSettings, _, _| {}))
            .on_action(cx.listener(|_, _: &ShowAbout, _, _| {}))
            .on_action(cx.listener(|_, _: &ShowTransfers, _, _| {}))
            .on_action(cx.listener(|_, _: &ShowStorage, _, _| {}))
            .on_action(cx.listener(|_, _: &UploadFile, _, _| {}))
            .on_action(cx.listener(|_, _: &FocusSearch, _, _| {}))
            .child(self.render_account(window, layout, cx))
            .when(self.proxy.show_banner(), |root| {
                root.child(self.render_network_banner(cx))
            })
            .when(self.show_telegram_api_id_prompt, |root| {
                root.child(screens::settings::render_telegram_api_id_prompt(self, cx))
            })
            .when(self.transition.is_some(), |root| {
                root.child(self.render_transition_dialog(cx))
            })
            .into_any_element()
    }

    pub(crate) fn ensure_telegram_qr_login(&mut self, cx: &mut Context<Self>) {
        if !self.visual_preview
            && !self.phone_login
            && !self.account_restoring
            && self.configured_telegram_api_id.is_some()
            && self.telegram_activity == TelegramActivity::Idle
            && matches!(self.telegram_auth, TelegramAuthState::Unauthorized)
        {
            self.begin_telegram_qr_login(cx);
        }
    }

    pub(crate) fn change_telegram_login_method(
        &mut self,
        phone: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.phone_login = phone;
        self.reset_telegram_login(window, cx);
    }

    pub(crate) fn request_account_switch(&mut self, cx: &mut Context<Self>) {
        if self.telegram_account.is_some() && self.telegram_activity != TelegramActivity::Working {
            self.confirm_account_switch = true;
            cx.notify();
        }
    }

    pub(crate) fn reset_telegram_login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.account_restoring = false;
        self.account_restore_retry_at = None;
        self.telegram_login_generation = self.telegram_login_generation.wrapping_add(1);
        self.qr_poll_task = None;
        self.qr_login_error = None;
        self.reset_dialog_load(cx);
        self.telegram_auth = TelegramAuthState::Unauthorized;
        self.telegram_activity = TelegramActivity::Idle;
        for input in [&self.telegram_code, &self.telegram_password] {
            input.update(cx, |input, cx| input.set_value("", window, cx));
        }
        self.ensure_telegram_qr_login(cx);
        cx.notify();
    }

    pub(crate) fn switch_telegram_account(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.confirm_account_switch || self.telegram_activity == TelegramActivity::Working {
            return;
        }
        if self.transition_has_work() {
            self.show_account_switch = true;
            self.request_transition(super::lifecycle::TransitionAction::SwitchAccount, cx);
            return;
        }
        self.confirm_account_switch = false;
        self.show_account_switch = false;
        self.phone_login = false;
        self.telegram_login_generation = self.telegram_login_generation.wrapping_add(1);
        self.reset_dialog_load(cx);
        self.transfers_account_ready = false;
        self.storage_retry_task = None;
        self.storage_loading = false;
        self.storage_notice = None;
        self.library_batch_cancellation
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self.cancel_managed_scan();
        self.cancel_telegram_file_load(cx);
        if let Some(sync) = self.channel_sync.take() {
            sync.stop();
        }
        self.channel_sync_task = None;
        self.channel_sync_snapshot = None;
        self.channel_view_cache.clear();
        self.lock_vault(window, cx);
        self.qr_poll_task = None;
        let Some(telegram) = self.telegram.clone() else {
            if self.visual_preview {
                self.telegram_account = None;
                self.page = Page::Account;
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
                            this.page = Page::Account;
                            this.account_avatar = None;
                            this.telegram_chats.clear();
                            this.telegram_files.clear();
                            this.telegram_index = None;
                            this.selected_chat_id = None;
                            this.storage_status = teleark_runtime::StorageChannelStatus::Missing;
                            if let Some(progress) = this.storage_maintenance.take() {
                                progress.cancel();
                            }
                            this.storage_confirmation = None;
                            this.storage_maintenance_presentation = None;
                            this.storage_loading = false;
                            this.storage_error = None;
                            this.managed_vault_files = Default::default();
                            this.managed_health_checked = None;
                            this.managed_upload_receipts.clear();
                            this.managed_vault_rejected = 0;
                            this.vault_recovery_secret = None;
                            this.vault_locked = true;
                            this.unlock_intent = None;
                            this.upload_sources.clear();
                            this.upload_source_total_bytes = 0;
                            if let Some(progress) = this.upload_preparation_progress.take() {
                                progress.cancel();
                            }
                            this.upload_selection_progress = None;
                            this.last_channel_id = None;
                            this.local_downloads.clear();
                            this.telegram_file_generation =
                                this.telegram_file_generation.wrapping_add(1);
                            this.refresh_channel_file_table(cx);
                            this.ensure_telegram_qr_login(cx);
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
        self.telegram_login_generation = self.telegram_login_generation.wrapping_add(1);
        let generation = self.telegram_login_generation;
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
            this.update(cx, |this, cx| {
                if this.telegram_login_generation == generation {
                    this.apply_telegram_auth_result(result, cx);
                }
            });
        }));
    }

    pub(crate) fn begin_telegram_qr_login(&mut self, cx: &mut Context<Self>) {
        if self.telegram_activity == TelegramActivity::Working {
            return;
        }
        let telegram = self.telegram.clone();
        let library = self.library.clone();
        self.telegram_login_generation = self.telegram_login_generation.wrapping_add(1);
        let generation = self.telegram_login_generation;
        // Retire the old token and poll before starting a replacement request.
        // A failed replacement stays retryable instead of automatically looping.
        self.qr_poll_task = None;
        self.telegram_auth = TelegramAuthState::Unauthorized;
        self.telegram_activity = TelegramActivity::Working;
        cx.notify();
        let work = cx.background_spawn(async move {
            let (Some(telegram), Some(library)) = (telegram, library) else {
                return Err(ApplicationError::new(
                    teleark_core::ApplicationErrorKind::Persistence,
                ));
            };
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
                this.apply_telegram_login_result(generation, result, cx);
            });
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
        let generation = self.telegram_login_generation;
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
            this.update(cx, |this, cx| {
                this.apply_telegram_login_result(generation, result, cx)
            });
        }));
    }

    fn apply_telegram_login_result(
        &mut self,
        generation: u64,
        result: Result<TelegramAuthState, ApplicationError>,
        cx: &mut Context<Self>,
    ) {
        if self.telegram_login_generation == generation {
            self.apply_telegram_auth_result(result, cx);
        }
    }

    pub(super) fn apply_telegram_auth_result(
        &mut self,
        result: Result<TelegramAuthState, ApplicationError>,
        cx: &mut Context<Self>,
    ) {
        let restoring = self.account_restoring;
        self.account_restoring = false;
        self.account_restore_retry_at = None;
        match result {
            Ok(state) => {
                self.telegram_activity = TelegramActivity::Idle;
                if let TelegramAuthState::Authorized(account) = &state {
                    self.qr_login_error = None;
                    self.login_proxy_open = false;
                    self.telegram_account = Some(account.clone());
                }
                self.telegram_auth = state;
                if matches!(self.telegram_auth, TelegramAuthState::QrCode { .. }) {
                    self.schedule_qr_login_poll(cx);
                } else if matches!(self.telegram_auth, TelegramAuthState::Authorized(_)) {
                    self.qr_poll_task = None;
                    self.load_account_avatar(cx);
                    self.load_telegram_dialogs(cx);
                    if !restoring {
                        self.enter_workspace(cx);
                    }
                }
            }
            Err(error) => {
                self.telegram_activity = TelegramActivity::Failed(error.kind());
                if !restoring
                    && !self.phone_login
                    && matches!(self.telegram_auth, TelegramAuthState::QrCode { .. })
                {
                    // Preserve the error while exporting a new token. Never leave
                    // a rejected token on screen with its poll owner stopped.
                    self.qr_login_error = Some(error.kind());
                    self.begin_telegram_qr_login(cx);
                }
            }
        }
        if restoring {
            self.ensure_telegram_qr_login(cx);
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
                if matches!(this.telegram_auth, TelegramAuthState::QrCode { .. }) {
                    this.apply_telegram_login_result(generation, result, cx);
                }
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
        self.account_restore_event_at = Some(std::time::Instant::now());
        self.telegram_activity = TelegramActivity::Working;
        self.account_restore_retry_at = None;
        let generation = self.telegram_login_generation;
        cx.notify();
        self.telegram_task = Some(cx.spawn(async move |this, cx| {
            let mut attempt = 0_u32;
            loop {
                let remote = telegram.clone();
                let storage = library.clone();
                let result = cx
                    .background_spawn(async move { remote.connect_configured(&storage) })
                    .await;
                let Some(entity) = this.upgrade() else { return };
                let retry = entity.update(cx, |app, cx| {
                    if app.telegram_login_generation != generation {
                        return None;
                    }
                    if let Err(error) = &result
                        && matches!(
                            error.kind(),
                            teleark_core::ApplicationErrorKind::Network
                                | teleark_core::ApplicationErrorKind::Server
                                | teleark_core::ApplicationErrorKind::Conflict
                        )
                    {
                        use std::hash::BuildHasher as _;
                        let ceiling = (1_u64 << attempt.min(6)).min(60) * 1000;
                        let jitter =
                            std::collections::hash_map::RandomState::new().hash_one(attempt);
                        let deadline = std::time::Instant::now()
                            + Duration::from_millis(ceiling / 2 + jitter % (ceiling / 2 + 1));
                        app.account_restore_retry_at = Some(deadline);
                        app.account_restore_event_at = Some(std::time::Instant::now());
                        app.telegram_activity = TelegramActivity::Failed(error.kind());
                        cx.notify();
                        Some(deadline)
                    } else {
                        app.apply_telegram_auth_result(result, cx);
                        None
                    }
                });
                drop(entity);
                let Some(deadline) = retry else { return };
                attempt = attempt.saturating_add(1);
                loop {
                    let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                    if remaining.is_zero() {
                        break;
                    }
                    cx.background_executor()
                        .timer(remaining.min(Duration::from_secs(1)))
                        .await;
                    let Some(entity) = this.upgrade() else { return };
                    if !entity.update(cx, |app, cx| {
                        cx.notify();
                        app.telegram_login_generation == generation && app.account_restoring
                    }) {
                        return;
                    }
                }
                let Some(entity) = this.upgrade() else { return };
                if !entity.update(cx, |app, cx| {
                    if app.telegram_login_generation != generation || !app.account_restoring {
                        return false;
                    }
                    app.account_restore_retry_at = None;
                    app.telegram_activity = TelegramActivity::Working;
                    cx.notify();
                    true
                }) {
                    return;
                }
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

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit as gpui;

    fn fixture_qr(label: &str) -> TelegramAuthState {
        TelegramAuthState::QrCode {
            deep_link: format!("TeleArk synthetic QR {label} - not a login token"),
            expires_at_unix_seconds: 1_900_000_000,
        }
    }

    #[gpui::test]
    fn rejected_qr_refreshes_immediately_and_a_second_scan_can_enter(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Account);
        for full in [false, true] {
            cx.simulate_resize(gpui::size(px(900.0), px(600.0)));
            cx.update(|window, _| {
                if window.is_fullscreen() != full {
                    window.toggle_fullscreen();
                }
            });
            app.update(cx, |app, cx| {
                app.page = Page::Account;
                app.telegram_account = None;
                app.telegram_auth = fixture_qr("rejected");
                let old = app.telegram_login_generation;
                app.apply_telegram_login_result(
                    old,
                    Err(ApplicationError::new(
                        teleark_core::ApplicationErrorKind::Authorization,
                    )),
                    cx,
                );
                assert_eq!(app.telegram_activity, TelegramActivity::Working);
                assert_eq!(app.telegram_auth, TelegramAuthState::Unauthorized);
                assert_eq!(app.telegram_login_generation, old.wrapping_add(1));
                assert!(
                    app.telegram_task.is_some(),
                    "replacement request has an owner"
                );
                assert!(app.qr_poll_task.is_none(), "rejected token no longer polls");
                assert_eq!(
                    app.qr_login_error,
                    Some(teleark_core::ApplicationErrorKind::Authorization)
                );
                // Hold the replacement reply to review the waiting surface without
                // creating a real Telegram connection or exposing a real token.
                app.telegram_task = Some(cx.spawn(async |_, _| std::future::pending::<()>().await));
                app.apply_telegram_login_result(old, Ok(fixture_qr("stale")), cx);
                assert_eq!(app.telegram_auth, TelegramAuthState::Unauthorized);
            });
            cx.run_until_parked();
            assert!(cx.debug_bounds("account-qr-code").is_none());
            assert!(cx.debug_bounds("account-qr-placeholder").is_some());
            assert!(cx.debug_bounds("account-login-error").is_some());
            assert!(cx.debug_bounds("account-qr-retry").is_none());
            let method = cx
                .debug_bounds("account-login-method")
                .expect("method while refreshing");
            cx.update(|window, _| {
                assert!(method.bottom() <= window.viewport_size().height - px(40.0))
            });
            app.update(cx, |app, cx| {
                app.apply_telegram_login_result(
                    app.telegram_login_generation,
                    Ok(fixture_qr("replacement")),
                    cx,
                );
                assert_eq!(app.telegram_activity, TelegramActivity::Idle);
                assert!(app.qr_login_error.is_some(), "error survives the refresh");
            });
            cx.run_until_parked();
            assert!(cx.debug_bounds("account-qr-code").is_some());
            assert!(cx.debug_bounds("account-login-error").is_some());
            let method = cx
                .debug_bounds("account-login-method")
                .expect("method after refresh");
            cx.update(|window, _| {
                assert!(method.bottom() <= window.viewport_size().height - px(40.0))
            });
            app.update(cx, |app, cx| {
                app.apply_telegram_login_result(
                    app.telegram_login_generation,
                    Ok(TelegramAuthState::Authorized(TelegramAccount {
                        id: 77,
                        display_name: "Synthetic second scan".into(),
                        username: None,
                    })),
                    cx,
                );
                assert!(app.telegram_is_authorized());
                assert_eq!(app.page, Page::Storage);
                assert!(app.qr_login_error.is_none());
                assert!(app.qr_poll_task.is_none());
                app.telegram_task = None;
            });
        }
    }

    #[gpui::test]
    fn failed_qr_replacement_stops_retrying_and_method_change_fences_late_replies(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Account);
        let generation = app.update(cx, |app, cx| {
            app.telegram_account = None;
            app.telegram_auth = fixture_qr("rejected");
            app.apply_telegram_auth_result(
                Err(ApplicationError::new(
                    teleark_core::ApplicationErrorKind::Network,
                )),
                cx,
            );
            assert_eq!(app.telegram_activity, TelegramActivity::Working);
            app.telegram_login_generation
        });
        // The fixture has no runtime owners: the replacement fails asynchronously.
        cx.run_until_parked();
        app.read_with(cx, |app, _| {
            assert_eq!(
                app.telegram_login_generation, generation,
                "no unbounded retry loop"
            );
            assert_eq!(
                app.telegram_activity,
                TelegramActivity::Failed(teleark_core::ApplicationErrorKind::Persistence)
            );
            assert_eq!(app.telegram_auth, TelegramAuthState::Unauthorized);
        });
        assert!(cx.debug_bounds("account-qr-retry").is_some());
        let retry = cx
            .debug_bounds("account-qr-retry")
            .expect("manual retry")
            .center();
        cx.simulate_click(retry, gpui::Modifiers::default());
        let generation = app.update(cx, |app, _| app.telegram_login_generation);
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.change_telegram_login_method(true, window, cx);
                for result in [
                    Ok(fixture_qr("late refresh")),
                    Ok(TelegramAuthState::Authorized(TelegramAccount {
                        id: 77,
                        display_name: "Stale account".into(),
                        username: None,
                    })),
                    Err(ApplicationError::new(
                        teleark_core::ApplicationErrorKind::Authorization,
                    )),
                ] {
                    app.apply_telegram_login_result(generation, result, cx);
                    assert_eq!(app.telegram_auth, TelegramAuthState::Unauthorized);
                    assert_eq!(app.telegram_activity, TelegramActivity::Idle);
                    assert!(app.telegram_account.is_none());
                    assert!(app.qr_login_error.is_none());
                }
            });
        });
        cx.run_until_parked();
        app.read_with(cx, |app, _| assert!(app.phone_login));
    }

    #[gpui::test]
    fn signed_out_access_is_limited_to_login_and_proxy_at_both_window_sizes(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Account);
        app.update(cx, |app, cx| {
            app.telegram_auth = TelegramAuthState::Unauthorized;
            // Even a retained account record cannot grant workspace access.
            assert!(!app.telegram_is_authorized());
            cx.notify();
        });
        for full in [false, true] {
            cx.simulate_resize(gpui::size(px(900.0), px(600.0)));
            cx.update(|window, _| {
                if window.is_fullscreen() != full {
                    window.toggle_fullscreen();
                }
            });
            cx.run_until_parked();
            assert!(cx.debug_bounds("signed-out-page").is_some());
            assert!(cx.debug_bounds("account-preferences").is_none());
            cx.update(|window, cx| window.dispatch_action(Box::new(ShowAbout), cx));
            for shortcut in ["cmd-,", "cmd-1", "cmd-2", "cmd-u", "cmd-f"] {
                cx.simulate_keystrokes(shortcut);
                cx.run_until_parked();
                app.read_with(cx, |app, _| {
                    assert_eq!(app.page, Page::Account);
                    assert!(!app.show_upload);
                });
            }
            app.update(cx, |app, cx| {
                for page in [
                    Page::Settings,
                    Page::Storage,
                    Page::Channel,
                    Page::Transfers,
                    Page::Library,
                    Page::FileDetail,
                    Page::LegacyRecovery,
                ] {
                    app.set_page(page, cx);
                    assert_eq!(app.page, Page::Account);
                }
                let selected = app.selected_chat_id;
                app.select_channel(1001, cx);
                app.enter_workspace(cx);
                assert_eq!(app.page, Page::Account);
                assert_eq!(app.selected_chat_id, selected);
            });
            let proxy = cx.debug_bounds("account-proxy").expect("proxy entry");
            cx.update(|window, _| assert!(proxy.bottom() <= window.viewport_size().height));
            cx.simulate_click(proxy.center(), gpui::Modifiers::default());
            cx.run_until_parked();
            assert!(cx.debug_bounds("proxy-apply").is_some());
            assert!(cx.debug_bounds("account-qr-code").is_none());
            let enable = cx
                .debug_bounds("proxy-enable")
                .expect("enable proxy")
                .center();
            cx.simulate_click(enable, gpui::Modifiers::default());
            cx.run_until_parked();
            assert!(cx.debug_bounds("proxy-apply").is_some());
            let back = cx
                .debug_bounds("account-proxy")
                .expect("back to login")
                .center();
            cx.simulate_click(back, gpui::Modifiers::default());
            cx.run_until_parked();
            assert!(cx.debug_bounds("account-qr-code").is_some());
            assert!(cx.debug_bounds("proxy-apply").is_none());
            app.update(cx, |app, cx| {
                app.proxy.load_failed = true;
                cx.notify();
            });
            cx.run_until_parked();
            let banner = cx
                .debug_bounds("network-proxy-settings")
                .expect("proxy error action")
                .center();
            cx.simulate_click(banner, gpui::Modifiers::default());
            cx.run_until_parked();
            app.update(cx, |app, cx| {
                assert_eq!(app.page, Page::Account);
                assert!(app.login_proxy_open);
                app.login_proxy_open = false;
                app.proxy.load_failed = false;
                cx.notify();
            });
        }
    }

    #[gpui::test]
    fn account_switch_requires_explicit_confirmation_and_ignores_escape_and_backdrop(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Account);
        app.update(cx, |app, _| {
            assert!(app.telegram_account.is_some());
        });
        cx.run_until_parked();
        let switch = cx.debug_bounds("account-switch").expect("switch").center();
        cx.simulate_click(switch, gpui::Modifiers::default());
        cx.run_until_parked();
        app.update(cx, |app, _| {
            assert!(app.confirm_account_switch);
            assert!(app.telegram_account.is_some());
        });
        cx.simulate_keystrokes("escape");
        cx.simulate_click(gpui::point(px(10.0), px(10.0)), gpui::Modifiers::default());
        cx.run_until_parked();
        app.update(cx, |app, _| {
            assert!(app.confirm_account_switch);
            assert!(app.telegram_account.is_some());
        });
        let cancel = cx
            .debug_bounds("account-switch-cancel")
            .expect("cancel")
            .center();
        cx.simulate_click(cancel, gpui::Modifiers::default());
        cx.run_until_parked();
        app.update(cx, |app, cx| {
            assert!(!app.confirm_account_switch);
            assert!(app.telegram_account.is_some());
            app.request_account_switch(cx);
        });
        cx.run_until_parked();
        let confirm = cx
            .debug_bounds("account-switch-confirm")
            .expect("confirm")
            .center();
        cx.simulate_click(confirm, gpui::Modifiers::default());
        app.update(cx, |app, _| {
            assert!(!app.confirm_account_switch);
            assert!(app.telegram_account.is_none());
            assert!(!app.phone_login);
        });
    }

    #[gpui::test]
    fn qr_is_default_and_method_switch_invalidates_pending_auth(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Account);
        cx.simulate_resize(gpui::size(px(900.0), px(600.0)));
        app.update(cx, |app, cx| {
            app.telegram_account = None;
            app.telegram_auth = TelegramAuthState::Unauthorized;
            cx.notify();
        });
        cx.run_until_parked();
        let qr = cx
            .debug_bounds("account-qr-code")
            .expect("automatic preview QR");
        assert!(qr.size.width >= px(250.0));
        let method = cx
            .debug_bounds("account-login-method")
            .expect("phone link")
            .center();
        let generation = app.update(cx, |app, _| app.telegram_login_generation);
        cx.simulate_click(method, gpui::Modifiers::default());
        cx.run_until_parked();
        app.update(cx, |app, _| {
            assert!(app.phone_login);
            assert!(app.telegram_login_generation > generation);
        });
        assert!(cx.debug_bounds("account-qr-code").is_none());
        let method = cx
            .debug_bounds("account-login-method")
            .expect("QR link")
            .center();
        cx.simulate_click(method, gpui::Modifiers::default());
        cx.run_until_parked();
        assert!(cx.debug_bounds("account-qr-code").is_some());
    }

    #[gpui::test]
    fn unauthorized_restore_starts_qr_without_user_action_but_phone_mode_does_not(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Account);
        app.update(cx, |app, cx| {
            // Starting is acknowledged before the missing fixture owners are read
            // in the background; phone mode never starts a QR request.
            app.visual_preview = false;
            app.telegram_account = None;
            app.account_restoring = true;
            app.apply_telegram_auth_result(Ok(TelegramAuthState::Unauthorized), cx);
            assert_eq!(app.telegram_activity, TelegramActivity::Working);
            app.phone_login = true;
            app.account_restoring = true;
            app.apply_telegram_auth_result(Ok(TelegramAuthState::Unauthorized), cx);
            assert_eq!(app.telegram_activity, TelegramActivity::Idle);
            app.visual_preview = true;
        });
    }
}
