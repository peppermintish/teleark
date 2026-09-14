//! Session loss is an event, followed by durable pause acknowledgments and local
//! session retirement. Never poll the service or resume work after a pause error.
use super::*;
use gpui_kit::StatefulInteractiveElement as _;
use std::{collections::VecDeque, time::Instant};
use teleark_runtime::{AuthorizationPhase, AuthorizationSnapshot};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Phase {
    Pausing,
    Retiring,
    Failed,
    SignedOut,
}

pub(crate) struct SessionLoss {
    pub expected: AuthorizationSnapshot,
    pub(super) phase: Phase,
    pub quit_after_pause: bool,
    history: VecDeque<(Phase, Instant)>,
    omitted: usize,
    show_history: bool,
}
impl SessionLoss {
    pub(super) fn new(expected: AuthorizationSnapshot, phase: Phase) -> Self {
        Self {
            expected,
            phase,
            quit_after_pause: false,
            history: VecDeque::from([(phase, Instant::now())]),
            omitted: 0,
            show_history: false,
        }
    }
    fn advance(&mut self, phase: Phase) {
        self.phase = phase;
        if self.history.back().is_some_and(|(last, _)| *last == phase) {
            return;
        }
        if self.history.len() == 16 {
            self.history.pop_front();
            self.omitted += 1;
        }
        self.history.push_back((phase, Instant::now()));
    }
}
impl Phase {
    fn message(self) -> &'static str {
        match self {
            Self::Pausing => "session-loss-pausing",
            Self::Retiring => "session-loss-retiring",
            Self::Failed => "session-loss-failed",
            Self::SignedOut => "session-loss-paused",
        }
    }
}

impl TeleArkApp {
    pub(crate) fn session_loss_pending(&self) -> bool {
        self.session_loss
            .as_ref()
            .is_some_and(|loss| loss.phase != Phase::SignedOut)
    }

    pub(super) fn start_authorization_observer(&mut self, cx: &mut Context<Self>) {
        let Some(telegram) = self.telegram.clone() else {
            return;
        };
        let mut updates = telegram.authorization_updates();
        self.authorization_task = Some(cx.spawn(async move |this, cx| {
            loop {
                let snapshot = telegram.authorization_snapshot();
                let _ = this.update(cx, |app, cx| app.observe_authorization(snapshot, cx));
                if updates.changed().await.is_none() {
                    break;
                }
            }
        }));
    }

    pub(super) fn observe_authorization(
        &mut self,
        event: AuthorizationSnapshot,
        cx: &mut Context<Self>,
    ) {
        if self
            .telegram
            .as_ref()
            .is_some_and(|telegram| telegram.authorization_snapshot() != event)
        {
            return;
        }
        if self.authorization_snapshot != Some(event) {
            self.authorization_snapshot = Some(event);
            cx.notify();
        }
        if event.phase != AuthorizationPhase::Revoked
            || self.telegram_account.as_ref().map(|account| account.id) != event.account_id
            || self
                .session_loss
                .as_ref()
                .is_some_and(|loss| loss.expected.generation >= event.generation)
        {
            return;
        }
        self.session_loss = Some(SessionLoss::new(event, Phase::Pausing));
        // Fence navigation, old auth/dialog callbacks, future admission and auxiliary
        // windows immediately. Existing transfer owners keep their checkpoints.
        self.telegram_login_generation = self.telegram_login_generation.wrapping_add(1);
        self.vault_session_generation = self.vault_session_generation.wrapping_add(1);
        self.telegram_task = None;
        self.qr_poll_task = None;
        self.account_restoring = false;
        self.telegram_activity = TelegramActivity::Working;
        self.transfers_account_ready = false;
        self.login_proxy_open = false;
        self.transition = None;
        self.shutdown_task = None;
        self.confirm_account_switch = false;
        self.show_account_switch = false;
        self.show_upload = false;
        self.unlock_intent = None;
        self.channel_sync_details = false;
        self.dialogs.details = false;
        self.speed_limits.open = false;
        self.show_telegram_api_id_prompt = false;
        self.storage_retry_task = None;
        if let Some(progress) = &self.storage_maintenance {
            progress.cancel();
        }
        if let Some(progress) = &self.vault_key_progress {
            progress.cancel();
        }
        self.cancel_dialog_load(cx);
        self.cancel_telegram_file_load(cx);
        self.cancel_managed_scan();
        self.library_scan_cancellation.cancel();
        self.library_query_generation = self.library_query_generation.wrapping_add(1);
        self.library_batch_cancellation
            .store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(progress) = &self.upload_preparation_progress {
            progress.cancel();
        }
        if let Some(sync) = self.channel_sync.take() {
            sync.stop();
        }
        self.channel_sync_task = None;
        self.transfer_batch_window_request = None;
        if let Some((_, handle)) = self.transfer_batch_window.take() {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }
        self.pause_revoked_session(cx);
        cx.notify();
    }

    pub(super) fn pause_revoked_session(&mut self, cx: &mut Context<Self>) {
        let Some(loss) = &mut self.session_loss else {
            return;
        };
        loss.advance(Phase::Pausing);
        let expected = loss.expected;
        if self.session_loss_clock.is_none() {
            // Presentation only: elapsed phase time. No service calls or checks.
            self.session_loss_clock = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    let Ok(running) = this.update(cx, |app, cx| {
                        let running = app.session_loss.as_ref().is_some_and(|loss| {
                            matches!(loss.phase, Phase::Pausing | Phase::Retiring)
                        });
                        cx.notify();
                        if !running {
                            app.session_loss_clock = None;
                        }
                        running
                    }) else {
                        break;
                    };
                    if !running {
                        break;
                    }
                }
            }));
        }
        let entity = cx.weak_entity();
        let main = self.main_window;
        // First paint the reason; disk/network/settlement work starts afterward.
        cx.defer(move |cx| {
            let _ = main.update(cx, |_, window, _| {
                window.on_next_frame(move |window, cx| {
                    let _ = entity.update(cx, |app, cx| {
                        if !app.session_loss_matches(expected, Phase::Pausing) {
                            return;
                        }
                        app.clear_vault_inputs(window, cx);
                        app.vault_recovery_secret = None;
                        app.recovery_visible = false;
                        let transfers = app.transfers.clone();
                        let vault = app.vault.clone();
                        let native = cx.background_spawn(async move {
                            transfers.map_or(Ok(()), |owner| owner.suspend_account())
                        });
                        let encrypted = cx.background_spawn(async move {
                            vault
                                .as_ref()
                                .map_or(Ok(()), DesktopVault::pause_for_shutdown)
                        });
                        app.session_loss_task = Some(cx.spawn(async move |this, cx| {
                            let native = native.await;
                            let encrypted = encrypted.await;
                            let _ = this.update(cx, |app, cx| {
                                app.finish_session_pause(expected, native.and(encrypted), cx)
                            });
                        }));
                    });
                });
            });
        });
        cx.notify();
    }

    fn session_loss_matches(&self, expected: AuthorizationSnapshot, phase: Phase) -> bool {
        self.session_loss
            .as_ref()
            .is_some_and(|loss| loss.expected == expected && loss.phase == phase)
    }

    fn finish_session_pause(
        &mut self,
        expected: AuthorizationSnapshot,
        result: Result<(), ApplicationError>,
        cx: &mut Context<Self>,
    ) {
        if !self.session_loss_matches(expected, Phase::Pausing) {
            return;
        }
        if result.is_err() {
            self.session_loss
                .as_mut()
                .expect("session loss")
                .advance(Phase::Failed);
            cx.notify();
            return;
        }
        self.session_loss
            .as_mut()
            .expect("session loss")
            .advance(Phase::Retiring);
        let telegram = self.telegram.clone();
        let vault = self.vault.clone();
        let work = cx.background_spawn(async move {
            if let Some(vault) = &vault {
                vault.lock()?;
            }
            if let Some(telegram) = telegram {
                telegram.forget_revoked_session(expected)?;
            }
            if let Some(vault) = &vault {
                vault.abandon_shutdown();
            }
            Ok(())
        });
        self.session_loss_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |app, cx| {
                app.finish_session_retirement(expected, result, cx)
            });
        }));
        cx.notify();
    }

    fn finish_session_retirement(
        &mut self,
        expected: AuthorizationSnapshot,
        result: Result<(), ApplicationError>,
        cx: &mut Context<Self>,
    ) {
        if !self.session_loss_matches(expected, Phase::Retiring) {
            return;
        }
        let loss = self.session_loss.as_mut().expect("session loss");
        if result.is_err() {
            loss.advance(Phase::Failed);
        } else {
            loss.advance(Phase::SignedOut);
            if loss.quit_after_pause {
                cx.quit();
                return;
            }
            self.upload_in_flight = false;
            self.vault_download_in_flight = false;
            self.upload_preparing = false;
            self.qr_login_error = None;
            self.vault_status.locked = true;
            self.vault_status.active_key_locked = true;
            self.vault_status.historical_key_unlocked = false;
            self.channel_sync_snapshot = None;
            self.channel_view_cache.clear();
            self.finish_telegram_sign_out(cx);
        }
        cx.notify();
    }

    fn session_loss_message(&self) -> &'static str {
        self.session_loss
            .as_ref()
            .map_or(Phase::SignedOut, |loss| loss.phase)
            .message()
    }

    pub(crate) fn render_session_loss_status(&self) -> AnyElement {
        div()
            .debug_selector(|| "session-loss-status".into())
            .h(px(40.0))
            .flex_none()
            .px_4()
            .flex()
            .items_center()
            .gap_2()
            .text_xs()
            .text_color(theme::text_secondary())
            .child(Icon::new(IconName::Pause).size(px(12.0)))
            .child(self.tr(self.session_loss_message()))
            .into_any_element()
    }

    pub(crate) fn render_session_loss_body(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .debug_selector(|| "session-loss-page".into())
            .max_w(px(480.0))
            .flex()
            .flex_col()
            .items_center()
            .gap_4()
            .child(Icon::new(IconName::Pause).size(px(48.0)))
            .child(
                div()
                    .text_lg()
                    .text_center()
                    .child(self.tr(self.session_loss_message())),
            )
            .child(
                div()
                    .text_sm()
                    .text_center()
                    .text_color(theme::text_secondary())
                    .child(self.tr("session-loss-description")),
            )
            .when_some(self.session_loss.as_ref(), |body, loss| {
                let elapsed = loss
                    .history
                    .back()
                    .map_or(0, |(_, time)| time.elapsed().as_millis() as u64);
                body.when(
                    matches!(loss.phase, Phase::Pausing | Phase::Retiring),
                    |body| {
                        body.child(div().text_xs().text_color(theme::text_secondary()).child(
                            self.tr_with(
                                "session-loss-step-time",
                                MessageArgs::new().with(
                                    "duration",
                                    teleark_i18n::format::format_duration_millis(
                                        self.locale(),
                                        elapsed,
                                    ),
                                ),
                            ),
                        ))
                    },
                )
                .child(
                    components::button(
                        "session-loss-details",
                        self.tr("common-details"),
                        None,
                        false,
                    )
                    .on_click(cx.listener(|app, _, _, cx| {
                        if let Some(loss) = &mut app.session_loss {
                            loss.show_history = !loss.show_history;
                        }
                        cx.notify();
                    })),
                )
                .when(loss.show_history, |body| {
                    body.child(
                        div()
                            .id("session-loss-history")
                            .w_full()
                            .max_h(px(96.0))
                            .overflow_y_scroll()
                            .children(loss.history.iter().map(|(phase, _)| {
                                div().h(px(24.0)).text_xs().child(self.tr(phase.message()))
                            }))
                            .when(loss.omitted > 0, |list| {
                                list.child(self.tr_with(
                                    "session-loss-history-omitted",
                                    MessageArgs::new().with("count", loss.omitted.to_string()),
                                ))
                            }),
                    )
                })
            })
            .when(
                self.session_loss
                    .as_ref()
                    .is_some_and(|loss| loss.phase == Phase::Failed),
                |body| {
                    body.child(
                        components::button(
                            "session-loss-retry",
                            self.tr("common-retry"),
                            None,
                            true,
                        )
                        .debug_selector(|| "session-loss-retry".into())
                        .on_click(cx.listener(|app, _, _, cx| app.pause_revoked_session(cx))),
                    )
                },
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::lifecycle::TransitionAction;
    use gpui_kit as gpui;

    fn revoked() -> AuthorizationSnapshot {
        AuthorizationSnapshot {
            generation: 5,
            network_generation: 0,
            account_id: Some(1),
            phase: AuthorizationPhase::Revoked,
        }
    }

    #[gpui::test]
    fn loss_fences_work_before_pause_and_failed_pause_waits_for_explicit_retry(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Storage);
        app.update(cx, |app, cx| {
            let login = app.telegram_login_generation;
            app.observe_authorization(revoked(), cx);
            assert!(!app.telegram_is_authorized());
            assert!(app.session_loss_matches(revoked(), Phase::Pausing));
            assert!(
                app.session_loss_task.is_none(),
                "acknowledgment precedes the next-frame work"
            );
            assert!(!app.transfers_account_ready);
            let revision = app.telegram_login_generation;
            app.observe_authorization(revoked(), cx);
            assert_eq!(
                app.telegram_login_generation, revision,
                "duplicate event has no second pause"
            );
            app.apply_telegram_login_result(
                login,
                Ok(TelegramAuthState::Authorized(TelegramAccount {
                    id: 1,
                    display_name: "Late reply".into(),
                    username: None,
                })),
                cx,
            );
            app.finish_session_pause(
                revoked(),
                Err(ApplicationError::new(
                    teleark_core::ApplicationErrorKind::Persistence,
                )),
                cx,
            );
            assert!(app.session_loss_matches(revoked(), Phase::Failed));
            assert!(
                app.telegram_account.is_some(),
                "no session retirement before durable pause"
            );
            assert!(app.qr_poll_task.is_none());
            app.finish_session_retirement(revoked(), Ok(()), cx);
            assert!(
                app.session_loss_matches(revoked(), Phase::Failed),
                "stale completion ignored"
            );
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
        assert!(cx.debug_bounds("session-loss-retry").is_some());
        assert!(cx.debug_bounds("account-qr-code").is_none());
        assert!(cx.debug_bounds("account-login-proxy").is_none());
        app.read_with(cx, |app, _| {
            assert!(app.session_loss_matches(revoked(), Phase::Failed))
        });
        let retry = cx
            .debug_bounds("session-loss-retry")
            .expect("session-loss fixture")
            .center();
        cx.simulate_click(retry, gpui::Modifiers::default());
        cx.run_until_parked();
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
        app.read_with(cx, |app, _| {
            assert!(app.session_loss_matches(revoked(), Phase::SignedOut));
            assert!(app.telegram_account.is_none());
            assert_eq!(app.page, Page::Account);
            assert_eq!(app.telegram_auth, TelegramAuthState::Unauthorized);
            assert!(!app.transfers_account_ready);
        });
        assert!(cx.debug_bounds("session-loss-status").is_some());
        app.update(cx, |app, cx| {
            app.apply_telegram_auth_result(
                Ok(TelegramAuthState::Authorized(TelegramAccount {
                    id: 1,
                    display_name: "New login".into(),
                    username: None,
                })),
                cx,
            );
            assert!(app.session_loss.is_none());
            assert!(app.telegram_is_authorized());
        });
    }

    #[gpui::test]
    fn cleanup_failure_and_stale_events_do_not_resume_or_replace_the_new_session(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Storage);
        app.update(cx, |app, cx| {
            let mut other = revoked();
            other.account_id = Some(2);
            app.observe_authorization(other, cx);
            assert!(app.session_loss.is_none());
            app.session_loss = Some(SessionLoss::new(revoked(), Phase::Retiring));
            app.finish_session_retirement(
                revoked(),
                Err(ApplicationError::new(
                    teleark_core::ApplicationErrorKind::Persistence,
                )),
                cx,
            );
            assert!(app.session_loss_matches(revoked(), Phase::Failed));
            app.request_transition(TransitionAction::ApplyProxy, cx);
            assert!(app.transition.is_none());
            let mut next = revoked();
            next.generation += 1;
            app.session_loss = Some(SessionLoss::new(next, Phase::Pausing));
            app.finish_session_pause(revoked(), Ok(()), cx);
            app.finish_session_retirement(revoked(), Ok(()), cx);
            assert!(app.session_loss_matches(next, Phase::Pausing));
            app.session_loss
                .as_mut()
                .expect("session-loss fixture")
                .advance(Phase::Failed);
        });
    }

    #[gpui::test]
    fn all_session_loss_phases_fit_small_and_native_fullscreen_windows(
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
            for phase in [
                Phase::Pausing,
                Phase::Retiring,
                Phase::Failed,
                Phase::SignedOut,
            ] {
                app.update(cx, |app, cx| {
                    app.session_loss = Some(SessionLoss::new(revoked(), phase));
                    app.session_loss.as_mut().expect("fixture").show_history = true;
                    app.telegram_auth = TelegramAuthState::Unauthorized;
                    if phase == Phase::SignedOut {
                        app.telegram_account = None;
                    }
                    cx.notify();
                });
                cx.run_until_parked();
                let status = cx
                    .debug_bounds("session-loss-status")
                    .expect("persistent reason at bottom");
                cx.update(|window, _| assert!(status.bottom() <= window.viewport_size().height));
                if phase != Phase::SignedOut {
                    let body = cx
                        .debug_bounds("session-loss-page")
                        .expect("pause progress");
                    assert!(body.bottom() <= status.top());
                    assert!(cx.debug_bounds("account-qr-code").is_none());
                }
                if phase == Phase::Failed {
                    let retry = cx
                        .debug_bounds("session-loss-retry")
                        .expect("retry reachable");
                    assert!(retry.bottom() <= status.top());
                }
                assert!(cx.debug_bounds("account-preferences").is_none());
            }
        }
    }

    #[test]
    fn repeated_failures_keep_a_bounded_visible_timeline() {
        let mut loss = SessionLoss::new(revoked(), Phase::Pausing);
        for _ in 0..100 {
            loss.advance(Phase::Failed);
            loss.advance(Phase::Pausing);
        }
        assert_eq!(loss.history.len(), 16);
        assert_eq!(loss.omitted, 185);
        assert_eq!(
            loss.history.back().expect("session-loss fixture").0,
            Phase::Pausing
        );
    }
}
