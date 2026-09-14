//! Whole-window access gate. Locking never mutates a runtime owner or its keys.
use super::*;
use gpui_kit::Focusable as _;
use gpui_kit::component::{
    Disableable as _, Icon, IconName, button::ButtonVariants as _, input::Input,
    scroll::ScrollableElement as _,
};
use teleark_runtime::AppPinRecord;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PinOperation {
    Unlock,
    Save,
    Disable,
}

struct PinRequest {
    operation: PinOperation,
    pin: String,
    next: String,
    record: Option<AppPinRecord>,
    library: Option<DesktopLibrary>,
    generation: u64,
}
pub(crate) struct AppLockUi {
    pending: Option<PinRequest>,
    pub record: Option<AppPinRecord>,
    pub locked: bool,
    pub load_failed: bool,
    pub show_proxy: bool,
    pub pin: Entity<InputState>,
    pub new_pin: Entity<InputState>,
    pub confirmation: Entity<InputState>,
    pub busy: bool,
    menus_locked: bool,
    pub message: Option<&'static str>,
    generation: u64,
    failures: u8,
    retry_at: Option<std::time::Instant>,
    task: Option<Task<()>>,
}
impl AppLockUi {
    pub fn new(
        record: Result<Option<AppPinRecord>, teleark_core::ApplicationErrorKind>,
        preview: bool,
        window: &mut Window,
        cx: &mut Context<TeleArkApp>,
    ) -> Self {
        let load_failed = record.is_err() && !preview;
        let record = record.ok().flatten();
        Self {
            locked: record.is_some() || load_failed,
            record,
            load_failed,
            show_proxy: false,
            pin: cx.new(|cx| InputState::new(window, cx).masked(true)),
            new_pin: cx.new(|cx| InputState::new(window, cx).masked(true)),
            confirmation: cx.new(|cx| InputState::new(window, cx).masked(true)),
            menus_locked: false,
            pending: None,
            busy: false,
            message: None,
            generation: 0,
            failures: 0,
            retry_at: None,
            task: None,
        }
    }
}

impl TeleArkApp {
    pub(crate) fn app_is_locked(&self) -> bool {
        self.app_lock.locked
    }

    pub(crate) fn lock_application(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.app_lock.record.is_none() && !self.app_lock.load_failed {
            return;
        }
        self.app_lock.locked = true;
        self.app_lock.generation = self.app_lock.generation.wrapping_add(1);
        self.app_lock.show_proxy = false;
        self.app_lock.message = None;
        self.clear_app_pin_inputs(window, cx);
        self.clear_vault_inputs(window, cx);
        self.recovery_visible = false;
        self.vault_recovery_secret = None;
        self.pending_vault_action = None;
        self.show_upload = false;
        self.channel_sync_details = false;
        self.dialogs.details = false;
        self.speed_limits.open = false;
        self.show_transfer_detail = false;
        self.transfer_batch_window_request = None;
        if let Some((_, handle)) = self.transfer_batch_window.take() {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }
        self.app_lock
            .pin
            .update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    fn clear_app_pin_inputs(&self, window: &mut Window, cx: &mut Context<Self>) {
        for input in [
            &self.app_lock.pin,
            &self.app_lock.new_pin,
            &self.app_lock.confirmation,
        ] {
            input.update(cx, |input, cx| input.set_value("", window, cx));
        }
    }

    pub(crate) fn submit_app_pin(
        &mut self,
        operation: PinOperation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.app_lock.busy || self.app_lock.load_failed {
            return;
        }
        if self
            .app_lock
            .retry_at
            .is_some_and(|at| at > std::time::Instant::now())
        {
            self.app_lock.message = Some("app-pin-rate-limited");
            cx.notify();
            return;
        }
        if operation != PinOperation::Unlock && self.app_is_locked() {
            return;
        }
        let pin = self.app_lock.pin.read(cx).value().to_string();
        let next = self.app_lock.new_pin.read(cx).value().to_string();
        if operation == PinOperation::Save
            && next != self.app_lock.confirmation.read(cx).value().as_ref()
        {
            self.app_lock.message = Some("app-pin-mismatch");
            cx.notify();
            return;
        }
        let record = self.app_lock.record.clone();
        let library = self.library.clone();
        let generation = self.app_lock.generation;
        self.clear_app_pin_inputs(window, cx);
        self.app_lock.busy = true;
        self.app_lock.message = Some(if operation == PinOperation::Unlock {
            "app-pin-checking"
        } else {
            "app-pin-saving"
        });
        cx.notify();
        self.app_lock.pending = Some(PinRequest {
            operation,
            pin,
            next,
            record,
            library,
            generation,
        });
    }

    pub(super) fn sync_access_menus(&mut self, cx: &mut Context<Self>) {
        let restricted = self.app_is_locked() || !self.telegram_is_authorized();
        if self.app_lock.menus_locked != restricted {
            self.app_lock.menus_locked = restricted;
            cx.set_menus(crate::menus::menus_for_access(&self.localizer, restricted));
        }
    }

    pub(super) fn schedule_pin_work(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(PinRequest {
            operation,
            pin,
            next,
            record,
            library,
            generation,
        }) = self.app_lock.pending.take()
        else {
            return;
        };
        // Paint acknowledgment before scheduling the memory-hard verifier or storage call.
        let entity = cx.weak_entity();
        window.on_next_frame(move |_, cx| {
            let _ = entity.update(cx, |app, cx| {
                let work = cx.background_spawn(async move {
                    if let Some(record) = &record
                        && !record.verify(pin)?
                    {
                        return Err(ApplicationError::new(
                            teleark_core::ApplicationErrorKind::Authorization,
                        ));
                    }
                    let next = match operation {
                        PinOperation::Unlock => return Ok(record),
                        PinOperation::Save => Some(AppPinRecord::create(next)?),
                        PinOperation::Disable => None,
                    };
                    let library = library.ok_or_else(|| {
                        ApplicationError::new(teleark_core::ApplicationErrorKind::Persistence)
                    })?;
                    library.replace_app_pin(record, next.clone())?;
                    Ok(next)
                });
                app.app_lock.task = Some(cx.spawn(async move |this, cx| {
                    let result = work.await;
                    let Some(this) = this.upgrade() else { return };
                    this.update(cx, |app, cx| {
                        app.app_lock.busy = false;
                        match result {
                            Ok(record) => {
                                app.app_lock.record = record;
                                app.app_lock.failures = 0;
                                app.app_lock.retry_at = None;
                                if generation == app.app_lock.generation
                                    || app.app_lock.record.is_none()
                                {
                                    app.app_lock.locked = false;
                                }
                                app.app_lock.show_proxy = false;
                                app.app_lock.message =
                                    (operation != PinOperation::Unlock).then_some("app-pin-saved");
                                if operation == PinOperation::Unlock
                                    && !app.app_is_locked()
                                    && app.page == Page::Account
                                    && app.telegram_account.is_some()
                                {
                                    app.enter_workspace(cx);
                                }
                            }
                            Err(error) => {
                                app.app_lock.message = Some(match error.kind() {
                                    teleark_core::ApplicationErrorKind::Authorization => {
                                        app.app_lock.failures =
                                            app.app_lock.failures.saturating_add(1);
                                        if app.app_lock.failures >= 5 {
                                            app.app_lock.retry_at = Some(
                                                std::time::Instant::now() + Duration::from_secs(30),
                                            );
                                            "app-pin-rate-limited"
                                        } else {
                                            "app-pin-incorrect"
                                        }
                                    }
                                    teleark_core::ApplicationErrorKind::InvalidRequest => {
                                        "app-pin-format"
                                    }
                                    _ => "app-pin-save-failed",
                                });
                            }
                        }
                        cx.notify();
                    });
                }));
            });
        });
        window.request_animation_frame();
    }

    fn pin_field(&self, label: &'static str, input: &Entity<InputState>) -> AnyElement {
        div()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(theme::text_secondary())
                    .child(self.tr(label)),
            )
            .child(
                Input::new(input)
                    .h(theme::FORM_CONTROL_HEIGHT)
                    .disabled(self.app_lock.busy),
            )
            .into_any_element()
    }

    pub(crate) fn render_app_pin_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let enabled = self.app_lock.record.is_some();
        components::card()
            .rounded(theme::RADIUS_LARGE)
            .overflow_hidden()
            .child(
                div()
                    .p_5()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .size(px(36.0))
                                    .flex_none()
                                    .rounded(theme::RADIUS_MEDIUM)
                                    .bg(theme::blue_pale())
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        Icon::new(if enabled {
                                            crate::assets::Symbol::Lock
                                        } else {
                                            crate::assets::Symbol::Unlock
                                        })
                                        .size(px(18.0))
                                        .text_color(theme::blue()),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(16.0))
                                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                    .child(self.tr("app-pin-title")),
                            )
                            .child(components::badge(
                                self.tr(if enabled {
                                    "app-pin-enabled"
                                } else {
                                    "app-pin-disabled"
                                }),
                                if enabled {
                                    components::Tone::Blue
                                } else {
                                    components::Tone::Neutral
                                },
                            )),
                    )
                    .child(
                        div()
                            .text_size(px(13.0))
                            .line_height(px(20.0))
                            .text_color(theme::text_secondary())
                            .child(self.tr("app-pin-description")),
                    )
                    .when(enabled, |body| {
                        body.child(self.pin_field("app-pin-current", &self.app_lock.pin))
                    })
                    .child(
                        div()
                            .grid()
                            .grid_cols(2)
                            .gap_3()
                            .child(self.pin_field("app-pin-new", &self.app_lock.new_pin))
                            .child(self.pin_field("app-pin-confirm", &self.app_lock.confirmation)),
                    )
                    .when_some(self.app_lock.message, |body, message| {
                        body.child(
                            div()
                                .text_size(px(12.0))
                                .text_color(theme::text_secondary())
                                .child(self.tr(message)),
                        )
                    }),
            )
            .child(
                components::confirmation_actions()
                    .when(enabled, |body| {
                        body.child(
                            components::button(
                                "app-pin-disable",
                                self.tr("app-pin-disable"),
                                None,
                                false,
                            )
                            .ghost()
                            .disabled(self.app_lock.busy)
                            .on_click(cx.listener(
                                |app, _, window, cx| {
                                    app.submit_app_pin(PinOperation::Disable, window, cx)
                                },
                            )),
                        )
                        .child(
                            components::button(
                                "app-pin-lock",
                                self.tr("app-pin-lock"),
                                None,
                                false,
                            )
                            .disabled(self.app_lock.busy)
                            .on_click(
                                cx.listener(|app, _, window, cx| app.lock_application(window, cx)),
                            ),
                        )
                    })
                    .child(div().flex_1())
                    .child(
                        components::button("app-pin-save", self.tr("app-pin-save"), None, true)
                            .debug_selector(|| "app-pin-save".into())
                            .disabled(self.app_lock.busy || self.app_lock.load_failed)
                            .on_click(cx.listener(|app, _, window, cx| {
                                app.submit_app_pin(PinOperation::Save, window, cx)
                            })),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn render_app_lock_screen(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let modal_open = self.transition.is_some();
        if modal_open != self.modal_was_open || window.focused(cx).is_none() {
            if modal_open {
                self.modal_focus.focus(window, cx);
            } else {
                self.app_lock
                    .pin
                    .read(cx)
                    .focus_handle(cx)
                    .focus(window, cx);
            }
            self.modal_was_open = modal_open;
        }
        let layout = LayoutPolicy::from_window(window);
        let mut content = div()
            .id("app-lock-page")
            .debug_selector(|| "app-lock-page".into())
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::access_canvas())
            .text_color(theme::text_primary())
            .font_family(".SystemUIFont")
            .track_focus(&self.main_focus)
            .on_action(cx.listener(|app, _: &DismissOverlay, window, cx| {
                if app.transition.is_some() {
                    app.cancel_transition(cx);
                } else if app.app_lock.show_proxy {
                    app.app_lock.show_proxy = false;
                    cx.notify();
                } else if window.is_fullscreen() {
                    window.toggle_fullscreen();
                }
            }))
            .on_action(cx.listener(|_, _: &ShowSettings, _, _| {}))
            .on_action(cx.listener(|_, _: &ShowTransfers, _, _| {}))
            .on_action(cx.listener(|_, _: &ShowStorage, _, _| {}))
            .on_action(cx.listener(|_, _: &UploadFile, _, _| {}))
            .on_action(cx.listener(|_, _: &FocusSearch, _, _| {}))
            .on_action(cx.listener(|_, _: &ShowAbout, _, _| {}));
        content = content.child(
            div()
                .h(px(56.0))
                .flex_none()
                .px_6()
                .flex()
                .items_center()
                .gap_2()
                .child(gpui_kit::img("teleark/app-icon.png").size(px(24.0)))
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                        .child("TeleArk"),
                )
                .child(div().flex_1())
                .child(
                    Icon::new(crate::assets::Symbol::Lock)
                        .size(px(12.0))
                        .text_color(theme::text_muted()),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(theme::text_secondary())
                        .child(self.tr("app-lock-state")),
                ),
        );
        if self.app_lock.show_proxy {
            content = content.child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .px_6()
                    .pb_4()
                    .child(
                        div()
                            .w_full()
                            .max_w(theme::SETTINGS_FORM_WIDTH)
                            .mx_auto()
                            .child(self.render_proxy_settings(layout, cx)),
                    ),
            );
        } else if self.telegram_account.is_none() && !self.app_lock.load_failed {
            content = content.child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_account(window, layout, cx)),
            );
        } else {
            content = content.child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .px_6()
                    .py_3()
                    .child(
                        components::card()
                            .w(theme::AUTH_PANEL_WIDTH)
                            .flex_none()
                            .rounded(px(16.0))
                            .shadow_sm()
                            .p_6()
                            .flex()
                            .flex_col()
                            .items_center()
                            .child(
                                div()
                                    .relative()
                                    .mb_4()
                                    .child(self.account_avatar_element(64.0))
                                    .child(
                                        div()
                                            .absolute()
                                            .right_0()
                                            .bottom_0()
                                            .size(px(22.0))
                                            .border_2()
                                            .border_color(theme::surface())
                                            .rounded_full()
                                            .bg(theme::blue())
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .child(
                                                Icon::new(crate::assets::Symbol::Lock)
                                                    .size(px(11.0))
                                                    .text_color(gpui_kit::rgb(0xffffff)),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(23.0))
                                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                    .max_w_full()
                                    .truncate()
                                    .child(
                                        self.telegram_account
                                            .as_ref()
                                            .map(|a| a.display_name.clone())
                                            .unwrap_or_else(|| "TeleArk".into()),
                                    ),
                            )
                            .child(
                                div()
                                    .mt_1()
                                    .text_size(px(13.0))
                                    .text_color(theme::text_secondary())
                                    .child(self.tr("app-lock-enter")),
                            )
                            .child(
                                div()
                                    .mt_5()
                                    .w_full()
                                    .child(Input::new(&self.app_lock.pin).h(px(38.0))),
                            )
                            .child(
                                components::button(
                                    "app-lock-enter",
                                    self.tr("app-lock-unlock"),
                                    None,
                                    true,
                                )
                                .w_full()
                                .mt_3()
                                .h(px(36.0))
                                .debug_selector(|| "app-lock-enter".into())
                                .disabled(self.app_lock.busy || self.app_lock.load_failed)
                                .on_click(cx.listener(
                                    |app, _, window, cx| {
                                        app.submit_app_pin(PinOperation::Unlock, window, cx)
                                    },
                                )),
                            )
                            .when_some(
                                self.app_lock
                                    .message
                                    .or(self.app_lock.load_failed.then_some("app-pin-load-failed")),
                                |body, id| {
                                    body.child(
                                        div()
                                            .mt_3()
                                            .w_full()
                                            .text_size(px(12.0))
                                            .text_color(theme::text_secondary())
                                            .child(self.tr(id)),
                                    )
                                },
                            )
                            .child(
                                div()
                                    .mt_5()
                                    .pt_4()
                                    .w_full()
                                    .border_t_1()
                                    .border_color(theme::border_subtle())
                                    .flex()
                                    .items_start()
                                    .gap_2()
                                    .text_size(px(12.0))
                                    .line_height(px(18.0))
                                    .text_color(theme::text_secondary())
                                    .child(
                                        Icon::new(crate::assets::Symbol::Transfer)
                                            .size(px(14.0))
                                            .mt(px(2.0))
                                            .flex_none(),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .child(self.tr("app-lock-background")),
                                    ),
                            ),
                    ),
            );
        }
        content
            .child(
                div()
                    .flex()
                    .justify_center()
                    .gap_3()
                    .flex_none()
                    .py_4()
                    .when(self.telegram_account.is_some(), |body| {
                        body.child(
                            components::button(
                                "app-lock-account",
                                self.tr("app-lock-switch"),
                                Some(IconName::CircleUser),
                                false,
                            )
                            .ghost()
                            .debug_selector(|| "app-lock-account".into())
                            .on_click(cx.listener(|app, _, _, cx| app.request_account_switch(cx))),
                        )
                    })
                    .child(
                        components::button(
                            "app-lock-proxy",
                            self.tr(if self.app_lock.show_proxy {
                                "app-lock-back"
                            } else {
                                "proxy-settings-title"
                            }),
                            Some(if self.app_lock.show_proxy {
                                IconName::ArrowLeft
                            } else {
                                IconName::Globe
                            }),
                            false,
                        )
                        .ghost()
                        .debug_selector(|| "app-lock-proxy".into())
                        .on_click(cx.listener(|app, _, _, cx| {
                            app.app_lock.show_proxy = !app.app_lock.show_proxy;
                            cx.notify();
                        })),
                    ),
            )
            .child(self.render_locked_background_status())
            .when(
                self.confirm_account_switch && self.transition.is_none(),
                |body| body.child(self.render_account_switch_dialog(cx)),
            )
            .when(self.transition.is_some(), |body| {
                body.child(self.render_transition_dialog(cx))
            })
            .into_any_element()
    }

    fn render_locked_background_status(&self) -> AnyElement {
        let status = self.shell_sync_status();
        let rates = self.status_rate_cache.borrow_mut().read(
            self.telegram_account.as_ref().map(|a| a.id),
            &self.native_transfer_view,
            &self.vault_transfer_view,
        );
        let rate = |icon, value: Option<u64>| {
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(gpui_kit::component::Icon::new(icon).size(px(11.0)))
                .child(value.map_or_else(
                    || self.tr("transfer-value-unavailable"),
                    |v| format_speed(self.locale(), v).into(),
                ))
        };
        div()
            .debug_selector(|| "locked-background-status".into())
            .h(px(theme::STATUS_BAR_HEIGHT))
            .flex_none()
            .px_3()
            .flex()
            .items_center()
            .gap_3()
            .text_size(px(11.0))
            .border_t_1()
            .border_color(theme::border())
            .child(div().flex_1().child(status.label))
            .child(rate(IconName::ArrowDown, rates.download))
            .child(rate(IconName::ArrowUp, rates.upload))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit as gpui;

    #[gpui::test]
    fn application_gate_preserves_background_owners_keys_and_all_sync_history(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Storage);
        let record = AppPinRecord::create("123456".into()).expect("synthetic PIN");
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.lock_application(window, cx);
                assert!(!app.app_is_locked(), "no PIN means no application lock");
                app.app_lock.record = Some(record);
                app.channel_sync_snapshot =
                    Some(super::super::channel_sync::tests::fixture_snapshot());
                let history = app.sync_history_rows();
                let files = app.managed_vault_files.clone();
                let key_status = app.vault_status;
                let generation = app.vault_session_generation;
                app.upload_in_flight = true;
                app.vault_upload_task =
                    Some(cx.spawn(async |_, _| std::future::pending::<()>().await));
                let scan = TelegramScanCancellation::new();
                app.managed_scan_cancellation = Some(scan.clone());
                app.lock_application(window, cx);
                assert!(app.app_is_locked());
                assert_eq!(app.vault_status.locked, key_status.locked);
                assert_eq!(app.vault_session_generation, generation);
                assert!(std::sync::Arc::ptr_eq(&files, &app.managed_vault_files));
                assert!(app.vault_upload_task.is_some());
                assert!(!scan.is_cancelled());
                assert!(app.sync_history_rows() == history);
            })
        });
        for full in [false, true] {
            cx.simulate_resize(gpui::size(px(900.0), px(600.0)));
            cx.update(|window, _| {
                if window.is_fullscreen() != full {
                    window.toggle_fullscreen();
                }
            });
            cx.run_until_parked();
            assert!(cx.debug_bounds("app-lock-page").is_some());
            for action in ["app-lock-enter", "app-lock-account", "app-lock-proxy"] {
                let bounds = cx.debug_bounds(action).expect("reachable lock action");
                cx.update(|window, _| assert!(bounds.bottom() <= window.viewport_size().height));
            }
            for shortcut in ["cmd-,", "cmd-1", "cmd-2", "cmd-u", "cmd-f", "escape"] {
                cx.simulate_keystrokes(shortcut);
                cx.run_until_parked();
                assert!(cx.debug_bounds("app-lock-page").is_some());
            }
            app.read_with(cx, |app, _| {
                assert!(app.upload_in_flight);
                assert_eq!(app.page, Page::Storage);
                assert!(!app.show_upload);
            });
        }
    }

    #[gpui::test]
    fn pin_form_acknowledges_before_work_and_rejects_stale_unlock(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Storage);
        let record = AppPinRecord::create("123456".into()).expect("synthetic PIN");
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.app_lock.record = Some(record);
                app.lock_application(window, cx);
                app.app_lock
                    .pin
                    .update(cx, |input, cx| input.set_value("123456", window, cx));
                app.submit_app_pin(PinOperation::Unlock, window, cx);
                assert!(app.app_lock.busy);
                assert!(
                    app.app_lock.task.is_none(),
                    "crypto starts after the acknowledgment frame"
                );
                assert!(app.app_lock.pin.read(cx).value().is_empty());
                app.lock_application(window, cx);
            })
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
        });
        cx.run_until_parked();
        app.read_with(cx, |app, _| {
            assert!(!app.app_lock.busy);
            assert!(
                app.app_is_locked(),
                "a late verification never reverses a newer lock"
            );
        });
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.app_lock
                    .pin
                    .update(cx, |input, cx| input.set_value("123456", window, cx));
                app.submit_app_pin(PinOperation::Unlock, window, cx);
            })
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
        });
        cx.run_until_parked();
        app.read_with(cx, |app, _| assert!(!app.app_is_locked()));
    }
}

#[cfg(test)]
mod failure_tests {
    use super::*;
    use gpui_kit as gpui;
    fn finish_frame(cx: &mut gpui::VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
        });
        cx.run_until_parked();
    }

    #[gpui::test]
    fn wrong_pin_cooldown_and_failed_persistence_preserve_protection(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Settings);
        let record = AppPinRecord::create("123456".into()).expect("synthetic PIN");
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.app_lock.record = Some(record);
                app.lock_application(window, cx);
                app.app_lock.failures = 4;
                app.app_lock
                    .pin
                    .update(cx, |input, cx| input.set_value("234567", window, cx));
                app.submit_app_pin(PinOperation::Unlock, window, cx);
            })
        });
        finish_frame(cx);
        app.read_with(cx, |app, _| {
            assert!(app.app_is_locked());
            assert_eq!(app.app_lock.message, Some("app-pin-rate-limited"));
        });
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.app_lock
                    .pin
                    .update(cx, |input, cx| input.set_value("123456", window, cx));
                app.submit_app_pin(PinOperation::Unlock, window, cx);
                assert!(
                    !app.app_lock.busy,
                    "cooldown does not dispatch another expensive verifier"
                );
                app.app_lock.retry_at = None; // controlled expiry, without sleeping
                app.submit_app_pin(PinOperation::Unlock, window, cx);
            })
        });
        finish_frame(cx);
        app.read_with(cx, |app, _| assert!(!app.app_is_locked()));
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.app_lock
                    .pin
                    .update(cx, |input, cx| input.set_value("123456", window, cx));
                app.submit_app_pin(PinOperation::Disable, window, cx);
            })
        });
        finish_frame(cx);
        app.read_with(cx, |app, _| {
            assert!(
                app.app_lock.record.is_some(),
                "unavailable storage cannot disable the persisted PIN"
            );
            assert_eq!(app.app_lock.message, Some("app-pin-save-failed"));
        });
    }
}

#[cfg(test)]
mod layout_tests {
    use super::*;
    use gpui_kit as gpui;

    #[gpui::test]
    fn pin_settings_and_network_log_entry_remain_reachable_in_window_and_fullscreen(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Settings);
        for full in [false, true] {
            cx.simulate_resize(gpui::size(px(900.0), px(600.0)));
            cx.update(|window, cx| {
                if window.is_fullscreen() != full {
                    window.toggle_fullscreen();
                }
                app.update(cx, |app, cx| {
                    app.app_lock.locked = false;
                    app.page = Page::Settings;
                    app.settings_section = SettingsSection::General;
                    cx.notify();
                });
            });
            cx.run_until_parked();
            let save = cx.debug_bounds("app-pin-save").expect("save PIN");
            cx.update(|window, _| {
                assert!(save.bottom() <= window.viewport_size().height);
                assert!(save.right() <= window.viewport_size().width);
            });
            app.update(cx, |app, cx| {
                app.settings_section = SettingsSection::Network;
                cx.notify();
            });
            cx.run_until_parked();
            let logs = cx
                .debug_bounds("settings-sync-log")
                .expect("sync log entry");
            cx.update(|window, _| assert!(logs.bottom() <= window.viewport_size().height));
            cx.simulate_click(logs.center(), gpui::Modifiers::default());
            cx.run_until_parked();
            assert!(cx.debug_bounds("channel-sync-inspector").is_some());
        }
    }
}
