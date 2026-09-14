//! Native exit pauses/checkpoints work; account/proxy changes use the event-driven drain gate.
use super::*;
use gpui_kit::component::Disableable as _;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TransitionAction {
    Quit,
    SwitchAccount,
    ApplyProxy,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransitionPhase {
    Confirm,
    Waiting,
    Pausing,
    Failed,
    Executing,
}
pub(crate) struct Transition {
    pub action: TransitionAction,
    pub phase: TransitionPhase,
    pub started: std::time::Instant,
}

impl TeleArkApp {
    pub(super) fn install_lifecycle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        crate::window_commands::register(self.main_window, cx.weak_entity(), cx);
        let weak = cx.weak_entity();
        window.on_window_should_close(cx, move |_, cx| {
            weak.update(cx, |app, cx| {
                if app.visual_preview {
                    return true;
                }
                app.request_transition(TransitionAction::Quit, cx);
                false
            })
            .unwrap_or(true)
        });
    }

    pub(crate) fn transition_has_work(&self) -> bool {
        self.upload_in_flight
            || self.vault_download_in_flight
            || self.upload_preparing
            || self.has_active_transfer()
            || self.vault_transfer_view.items.iter().any(|row| {
                matches!(
                    row.state,
                    teleark_runtime::VaultTransferState::Pausing
                        | teleark_runtime::VaultTransferState::Cancelling
                )
            })
            || self
                .vault
                .as_ref()
                .is_some_and(DesktopVault::has_active_transfers)
    }

    pub(crate) fn request_transition(&mut self, action: TransitionAction, cx: &mut Context<Self>) {
        if action == TransitionAction::Quit && self.visual_preview {
            cx.quit();
            return;
        }
        let main = self.main_window;
        cx.defer(move |cx| {
            let _ = main.update(cx, |_, window, _| window.activate_window());
        });
        if let Some(pending) = &self.transition {
            if action != TransitionAction::Quit || pending.action == TransitionAction::Quit {
                return;
            }
            // Closing the app can replace a wait for account/proxy changes.
            self.confirm_account_switch = false;
            self.show_account_switch = false;
        }
        self.transition = Some(Transition {
            action,
            started: std::time::Instant::now(),
            phase: if self.transition_has_work() {
                TransitionPhase::Confirm
            } else {
                TransitionPhase::Waiting
            },
        });
        self.advance_transition(cx);
        cx.notify();
    }

    /// Called by transfer revision notifications and preparation/completion callbacks.
    /// No polling timer, no repeated database reads, no work dispatched from render.
    pub(super) fn advance_transition(&mut self, cx: &mut Context<Self>) {
        if self
            .transition
            .as_ref()
            .is_none_or(|t| t.phase != TransitionPhase::Waiting)
        {
            return;
        }
        if self
            .transition
            .as_ref()
            .is_some_and(|t| t.action == TransitionAction::Quit)
        {
            self.begin_shutdown(cx);
            return;
        }
        if self.transition_has_work() {
            return;
        }
        let Some(transition) = &mut self.transition else {
            return;
        };
        if transition.phase != TransitionPhase::Waiting {
            return;
        }
        transition.phase = TransitionPhase::Executing;
        let expected_action = transition.action;
        let entity = cx.weak_entity();
        let window = self.main_window;
        cx.defer(move |cx| {
            let _ = window.update(cx, |_, window, cx| {
                let _ = entity.update(cx, |app, cx| {
                    if app.transition.as_ref().is_none_or(|t| {
                        t.action != expected_action || t.phase != TransitionPhase::Executing
                    }) {
                        return;
                    }
                    let Some(transition) = app.transition.take() else {
                        return;
                    };
                    // Recheck admission immediately before changing a runtime generation.
                    if app.transition_has_work() {
                        app.transition = Some(Transition {
                            phase: TransitionPhase::Waiting,
                            ..transition
                        });
                        cx.notify();
                        return;
                    }
                    match transition.action {
                        TransitionAction::Quit => cx.quit(),
                        TransitionAction::SwitchAccount => app.switch_telegram_account(window, cx),
                        TransitionAction::ApplyProxy => app.apply_proxy_after_drain(cx),
                    }
                });
            });
        });
    }

    pub(super) fn cancel_transition(&mut self, cx: &mut Context<Self>) {
        if self
            .transition
            .as_ref()
            .is_some_and(|t| t.phase == TransitionPhase::Pausing)
        {
            return;
        }
        self.transition = None;
        self.confirm_account_switch = false;
        self.show_account_switch = false;
        cx.notify();
    }

    fn begin_shutdown(&mut self, cx: &mut Context<Self>) {
        let Some(transition) = &mut self.transition else {
            return;
        };
        transition.phase = TransitionPhase::Pausing;
        if let Some(progress) = &self.upload_preparation_progress {
            progress.cancel();
        }
        self.cancel_managed_scan();
        let vault = self.vault.clone();
        let transfers = self.transfers.clone();
        let entity = cx.weak_entity();
        // Retain the owner and first paint the pause acknowledgment.
        let main = self.main_window;
        cx.defer(move |cx| {
            let _ = main.update(cx, |_, window, _| {
                window.on_next_frame(move |_, cx| {
                    let _ = entity.update(cx, |app, cx| {
                        // Independent owners receive pause concurrently: a blocked native
                        // writer cannot prevent encrypted transfers receiving their controls.
                        let native = cx.background_spawn(async move {
                            transfers.map_or(Ok(()), |t| t.pause_for_shutdown())
                        });
                        let encrypted = cx.background_spawn(async move {
                            vault
                                .as_ref()
                                .map_or(Ok(()), DesktopVault::pause_for_shutdown)
                        });
                        app.shutdown_task = Some(cx.spawn(async move |this, cx| {
                            let native_result = native.await;
                            let vault_result = encrypted.await;
                            let _ = this.update(cx, |app, cx| {
                                app.finish_shutdown(native_result.and(vault_result), cx);
                            });
                        }));
                    });
                });
            });
        });
        cx.notify();
    }

    fn finish_shutdown(&mut self, result: Result<(), ApplicationError>, cx: &mut Context<Self>) {
        if self
            .transition
            .as_ref()
            .is_none_or(|t| t.phase != TransitionPhase::Pausing)
        {
            return;
        }
        if result.is_ok() {
            cx.quit();
            return;
        }
        if let (Some(transfers), Some(account)) = (&self.transfers, &self.telegram_account) {
            transfers.abandon_shutdown(account.id);
        }
        if let Some(vault) = &self.vault {
            vault.abandon_shutdown();
        }
        self.transition.as_mut().expect("pending exit").phase = TransitionPhase::Failed;
        cx.notify();
    }

    pub(crate) fn render_transition_dialog(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(transition) = &self.transition else {
            return div().into_any_element();
        };
        let quitting = transition.action == TransitionAction::Quit;
        let failed = transition.phase == TransitionPhase::Failed;
        let pausing = transition.phase == TransitionPhase::Pausing;
        let waiting = !matches!(
            transition.phase,
            TransitionPhase::Confirm | TransitionPhase::Failed
        );
        let title = match transition.action {
            TransitionAction::Quit => "transition-quit",
            TransitionAction::SwitchAccount => "transition-account",
            TransitionAction::ApplyProxy => "transition-proxy",
        };
        let popup =
            components::confirmation_surface("transition-dialog")
                .child(components::confirmation_heading(
                    self.tr(title),
                    self.tr(if quitting {
                        "transition-quit-description"
                    } else {
                        "transition-description"
                    }),
                    match transition.action {
                        TransitionAction::Quit => IconName::Close,
                        TransitionAction::SwitchAccount => IconName::CircleUser,
                        TransitionAction::ApplyProxy => IconName::Globe,
                    },
                ))
                .child(
                    div()
                        .mx_5()
                        .mb_5()
                        .p_3()
                        .rounded(theme::RADIUS_MEDIUM)
                        .bg(theme::blue_pale())
                        .flex()
                        .items_start()
                        .gap_2()
                        .child(
                            gpui_kit::component::Icon::new(crate::assets::Symbol::Transfer)
                                .size(px(16.0))
                                .text_color(theme::blue())
                                .flex_none(),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(px(12.0))
                                .line_height(px(18.0))
                                .child(div().font_weight(gpui_kit::FontWeight::MEDIUM).child(
                                    self.tr(if failed {
                                        "transition-pause-failed-title"
                                    } else if pausing {
                                        "transition-pausing-title"
                                    } else if waiting {
                                        "transition-waiting-title"
                                    } else {
                                        "transition-active"
                                    }),
                                ))
                                .child(div().mt_1().text_color(theme::text_secondary()).child(
                                    self.tr(if failed {
                                        "transition-pause-failed"
                                    } else if pausing {
                                        "transition-pausing"
                                    } else if quitting {
                                        "transition-quit-preserved"
                                    } else if waiting {
                                        "transition-waiting"
                                    } else {
                                        "transition-preserved"
                                    }),
                                ))
                                .when(waiting, |body| {
                                    body.child(
                                        div().mt_2().text_color(theme::text_muted()).child(
                                            self.tr_with(
                                                "transition-started",
                                                MessageArgs::new().with(
                                                    "time",
                                                    self.sync_event_time(transition.started)
                                                        .to_string(),
                                                ),
                                            ),
                                        ),
                                    )
                                }),
                        ),
                )
                .child(
                    components::confirmation_actions()
                        .child(
                            components::button(
                                "transition-cancel",
                                self.tr("common-cancel"),
                                None,
                                false,
                            )
                            .debug_selector(|| "transition-cancel".into())
                            .disabled(pausing)
                            .on_click(cx.listener(|app, _, _, cx| {
                                app.cancel_transition(cx);
                            })),
                        )
                        .child(
                            components::button(
                                "transition-wait",
                                self.tr(if failed {
                                    "transition-pause-retry"
                                } else if quitting {
                                    "transition-pause-quit"
                                } else {
                                    "transition-wait"
                                }),
                                None,
                                true,
                            )
                            .debug_selector(|| "transition-wait".into())
                            .disabled(waiting)
                            .on_click(cx.listener(|app, _, _, cx| {
                                if let Some(transition) = &mut app.transition {
                                    transition.phase = TransitionPhase::Waiting;
                                }
                                app.advance_transition(cx);
                                cx.notify();
                            })),
                        ),
                );
        let entity = cx.weak_entity();
        gpui_kit::base::Dialog::new(cx)
            .focus_handle(self.modal_focus.clone())
            .flex()
            .items_center()
            .justify_center()
            .backdrop(div().absolute().inset_0().bg(theme::modal_backdrop()))
            .popup(popup)
            .close_on_backdrop_press(false)
            .on_cancel(move |_, _, cx| {
                let _ = entity.update(cx, |app, cx| app.cancel_transition(cx));
                false
            })
            .on_ok(|_, _, _| false)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit as gpui;

    #[gpui::test]
    fn native_close_allows_preview_and_prompts_for_real_work(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Storage);
        app.update(cx, |app, _| {
            app.upload_in_flight = true;
            app.app_lock.locked = true;
        });
        assert!(
            cx.simulate_close(),
            "preview permits the native close event"
        );
        app.update(cx, |app, _| app.visual_preview = false);
        assert!(
            !cx.simulate_close(),
            "real work reaches the confirmation before closing"
        );
        app.read_with(cx, |app, _| {
            assert!(
                app.transition
                    .as_ref()
                    .is_some_and(|t| t.phase == TransitionPhase::Confirm)
            )
        });
    }

    #[gpui::test]
    fn preview_does_not_wait_and_failed_or_stale_shutdown_does_not_close_the_session(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Storage);
        app.update(cx, |app, cx| {
            app.upload_in_flight = true;
            app.request_transition(TransitionAction::Quit, cx);
            assert!(
                app.transition.is_none(),
                "synthetic transfer never blocks preview quit"
            );
            app.visual_preview = false;
            app.request_transition(TransitionAction::Quit, cx);
            assert!(
                app.transition
                    .as_ref()
                    .is_some_and(|t| t.phase == TransitionPhase::Confirm)
            );
            app.cancel_transition(cx);
            app.finish_shutdown(Ok(()), cx);
            assert!(app.transition.is_none(), "stale completion has no effect");
            app.request_transition(TransitionAction::Quit, cx);
            app.transition.as_mut().expect("exit").phase = TransitionPhase::Pausing;
            app.cancel_transition(cx);
            assert!(
                app.transition.is_some(),
                "do not abandon a live checkpoint writer"
            );
            app.finish_shutdown(
                Err(ApplicationError::new(
                    teleark_core::ApplicationErrorKind::Persistence,
                )),
                cx,
            );
            assert!(
                app.transition
                    .as_ref()
                    .is_some_and(|t| t.phase == TransitionPhase::Failed)
            );
            app.cancel_transition(cx);
            assert!(app.transition.is_none());
        });
    }

    #[gpui::test]
    fn every_disruptive_action_waits_for_work_and_can_be_cancelled_while_locked(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Storage);
        for locked in [false, true] {
            for action in [
                TransitionAction::SwitchAccount,
                TransitionAction::ApplyProxy,
            ] {
                app.update(cx, |app, cx| {
                    app.app_lock.locked = locked;
                    app.upload_in_flight = true;
                    app.request_transition(action, cx);
                    assert!(
                        app.transition
                            .as_ref()
                            .is_some_and(|t| t.phase == TransitionPhase::Confirm)
                    );
                });
                cx.run_until_parked();
                let wait = cx
                    .debug_bounds("transition-wait")
                    .expect("wait action")
                    .center();
                cx.simulate_click(wait, gpui::Modifiers::default());
                app.read_with(cx, |app, _| {
                    assert!(
                        app.transition
                            .as_ref()
                            .is_some_and(|t| t.phase == TransitionPhase::Waiting)
                    )
                });
                cx.background_executor
                    .advance_clock(Duration::from_secs(120));
                cx.run_until_parked();
                app.read_with(cx, |app, _| assert!(app.upload_in_flight));
                let cancel = cx
                    .debug_bounds("transition-cancel")
                    .expect("cancel remains reachable")
                    .center();
                cx.simulate_click(cancel, gpui::Modifiers::default());
                app.read_with(cx, |app, _| {
                    assert!(app.transition.is_none());
                    assert!(app.telegram_account.is_some());
                    assert!(app.upload_in_flight);
                });
            }
        }
    }

    #[gpui::test]
    fn completion_notification_executes_once_and_native_work_also_blocks(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Storage);
        app.update(cx, |app, cx| {
            app.preview_native_cleanup(false);
            assert!(
                app.transition_has_work(),
                "native writer cleanup is protected"
            );
            app.native_transfer_view.items = Default::default();
            app.upload_in_flight = true;
            app.confirm_account_switch = true;
            app.request_transition(TransitionAction::SwitchAccount, cx);
            app.transition.as_mut().expect("pending").phase = TransitionPhase::Waiting;
            app.advance_transition(cx);
            assert!(app.telegram_account.is_some());
            app.upload_in_flight = false;
            app.advance_transition(cx);
            app.advance_transition(cx);
        });
        cx.run_until_parked();
        app.read_with(cx, |app, _| {
            assert!(app.transition.is_none());
            assert!(app.telegram_account.is_none());
        });
    }
}
