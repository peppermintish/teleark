//! Credential settings and retained migration feedback; all I/O runs in Runtime owners.
use super::*;
use gpui_kit::component::{Disableable as _, Sizable as _, button::ButtonVariants as _};
use teleark_runtime::{KeychainStatus, VaultKeyProgress};

pub(crate) struct KeychainUi {
    pub status: Option<KeychainStatus>,
    pub error: Option<teleark_core::ApplicationErrorKind>,
    pub confirmation: bool,
    pub busy: bool,
    pub progress: Option<VaultKeyProgress>,
    cancel_requested: bool,
    status_check_started: Option<std::time::Instant>,
    task: Option<Task<()>>,
}
impl KeychainUi {
    pub fn new(
        status: Result<KeychainStatus, teleark_core::ApplicationErrorKind>,
        preview: bool,
    ) -> Self {
        let status = if preview {
            Ok(KeychainStatus {
                enabled: cfg!(target_os = "macos"),
                supported: cfg!(target_os = "macos"),
                cleanup_pending: 0,
                unavailable_credentials: 0,
            })
        } else {
            status
        };
        Self {
            status: status.as_ref().ok().copied(),
            error: status.err(),
            confirmation: false,
            busy: false,
            progress: None,
            cancel_requested: false,
            status_check_started: None,
            task: None,
        }
    }
}
impl Drop for KeychainUi {
    fn drop(&mut self) {
        if let Some(progress) = &self.progress {
            progress.cancel();
        }
    }
}
struct KeychainOutcome {
    status: Result<KeychainStatus, ApplicationError>,
    change_error: Option<teleark_core::ApplicationErrorKind>,
}

impl TeleArkApp {
    pub(crate) fn request_keychain_toggle(&mut self, cx: &mut Context<Self>) {
        let Some(status) = self.keychain.status else {
            return;
        };
        if self.keychain.busy || !status.supported {
            return;
        }
        if status.enabled {
            self.keychain.confirmation = true;
            cx.notify();
        } else {
            self.change_keychain(true, cx);
        }
    }

    fn change_keychain(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.keychain.confirmation = false;
        if self.keychain.busy || self.keychain.status.is_none_or(|s| !s.supported) {
            return;
        }
        if self.visual_preview {
            if let Some(status) = &mut self.keychain.status {
                status.enabled = enabled;
            }
            cx.notify();
            return;
        }
        let Some(library) = self.library.clone() else {
            return;
        };
        let progress = VaultKeyProgress::new();
        self.keychain.status_check_started = None;
        self.keychain.cancel_requested = false;
        self.keychain.progress = Some(progress.clone());
        self.keychain.busy = true;
        self.keychain.error = None;
        cx.notify();
        let work = cx.background_spawn(async move {
            let change_error = library
                .set_keychain_enabled(enabled, progress)
                .err()
                .map(|error| error.kind());
            KeychainOutcome {
                status: library.keychain_status(),
                change_error,
            }
        });
        self.observe_keychain_work(work, cx);
    }

    fn refresh_keychain_status(&mut self, cx: &mut Context<Self>) {
        if self.keychain.busy {
            return;
        }
        if self.visual_preview {
            self.keychain =
                KeychainUi::new(Err(teleark_core::ApplicationErrorKind::Persistence), true);
            cx.notify();
            return;
        }
        let Some(library) = self.library.clone() else {
            return;
        };
        self.keychain.busy = true;
        self.keychain.error = None;
        self.keychain.progress = None;
        self.keychain.cancel_requested = false;
        self.keychain.status_check_started = Some(std::time::Instant::now());
        cx.notify();
        let work = cx.background_spawn(async move {
            KeychainOutcome {
                status: library.keychain_status(),
                change_error: None,
            }
        });
        self.observe_keychain_work(work, cx);
    }

    fn finish_keychain_work(&mut self, outcome: KeychainOutcome, cx: &mut Context<Self>) {
        self.keychain.busy = false;
        self.keychain.status_check_started = None;
        self.keychain.cancel_requested = false;
        // Always replace the observed backend. If the follow-up read failed,
        // do not leave a stale On/Off label after a successful storage change.
        self.keychain.status = outcome.status.as_ref().ok().copied();
        self.keychain.error = outcome
            .change_error
            .or_else(|| outcome.status.err().map(|error| error.kind()));
        cx.notify();
    }

    fn observe_keychain_work(&mut self, work: Task<KeychainOutcome>, cx: &mut Context<Self>) {
        self.keychain.task = Some(cx.spawn(async move |this, cx| {
            use std::{
                future::{Future as _, poll_fn},
                pin::Pin,
                task::Poll,
            };
            let mut work = work;
            loop {
                let mut timer = cx.background_executor().timer(Duration::from_millis(150));
                let result = poll_fn(|cx| {
                    if let Poll::Ready(result) = Pin::new(&mut work).poll(cx) {
                        Poll::Ready(Some(result))
                    } else {
                        Pin::new(&mut timer).poll(cx).map(|()| None)
                    }
                })
                .await;
                let done = result.is_some();
                let Some(this) = this.upgrade() else { return };
                this.update(cx, |app, cx| {
                    if let Some(result) = result {
                        app.finish_keychain_work(result, cx);
                    }
                    cx.notify();
                });
                if done {
                    break;
                }
            }
        }));
    }

    pub(crate) fn render_keychain_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let enabled = self.keychain.status.is_some_and(|s| s.enabled);
        let supported = self.keychain.status.is_some_and(|s| s.supported);
        components::card()
            .p_5()
            .child(components::section_title(
                self.tr("settings-keychain-title"),
            ))
            .child(
                div()
                    .mt_2()
                    .text_sm()
                    .text_color(theme::text_secondary())
                    .child(self.tr(if cfg!(target_os = "macos") {
                        "settings-keychain-description"
                    } else {
                        "settings-keychain-unavailable"
                    })),
            )
            .child(
                components::button(
                    "settings-keychain-toggle",
                    self.tr(if self.keychain.status.is_none() {
                        "settings-keychain-unknown"
                    } else if enabled {
                        "settings-keychain-on"
                    } else {
                        "settings-keychain-off"
                    }),
                    Some(IconName::Asterisk),
                    enabled,
                )
                .debug_selector(|| "settings-keychain-toggle".into())
                .mt_3()
                .disabled(self.keychain.busy || !supported)
                .on_click(cx.listener(|app, _, _, cx| app.request_keychain_toggle(cx))),
            )
            .when(
                self.keychain.status.is_some_and(|s| s.cleanup_pending > 0),
                |card| {
                    card.child(
                        div()
                            .mt_2()
                            .text_sm()
                            .text_color(theme::amber())
                            .child(self.tr("settings-keychain-cleanup-pending")),
                    )
                },
            )
            .when(
                self.keychain
                    .status
                    .is_some_and(|s| s.unavailable_credentials > 0),
                |card| {
                    card.child(
                        div()
                            .mt_2()
                            .text_sm()
                            .text_color(theme::amber())
                            .child(self.tr("settings-keychain-unavailable-credentials")),
                    )
                },
            )
            .when(self.keychain.error.is_some(), |card| {
                card.child(
                    div()
                        .mt_2()
                        .text_sm()
                        .text_color(theme::amber())
                        .child(self.tr(self.keychain_error_id())),
                )
            })
            .when(
                self.keychain.status.is_none() && self.keychain.error.is_some(),
                |card| {
                    card.child(
                        components::button(
                            "settings-keychain-retry",
                            self.tr("common-retry"),
                            Some(IconName::Redo2),
                            false,
                        )
                        .mt_2()
                        .disabled(
                            self.keychain.busy || self.library.is_none() && !self.visual_preview,
                        )
                        .debug_selector(|| "settings-keychain-retry".into())
                        .on_click(cx.listener(|app, _, _, cx| app.refresh_keychain_status(cx))),
                    )
                },
            )
            .when(self.keychain.status_check_started.is_some(), |card| {
                card.child(self.render_keychain_activity(cx))
            })
            .when_some(self.keychain.progress.as_ref(), |card, progress| {
                let snapshot = progress.snapshot();
                card.child(self.render_keychain_activity(cx)).child(
                    div()
                        .mt_2()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .text_xs()
                        .child(self.tr("vault-key-timeline-title"))
                        .children(snapshot.timeline.iter().map(|(phase, millis)| {
                            div().child(
                                self.tr_with(
                                    "vault-key-timeline-event",
                                    MessageArgs::new()
                                        .with(
                                            "phase",
                                            self.tr(crate::screens::unlock::key_phase_id(*phase))
                                                .to_string(),
                                        )
                                        .with(
                                            "elapsed",
                                            teleark_i18n::format::format_duration_millis(
                                                self.locale(),
                                                *millis,
                                            ),
                                        ),
                                ),
                            )
                        })),
                )
            })
            .into_any_element()
    }

    fn keychain_error_id(&self) -> &'static str {
        if self.keychain.error == Some(teleark_core::ApplicationErrorKind::Cancelled) {
            "settings-keychain-cancelled"
        } else if self.keychain.status.is_none() {
            "settings-keychain-status-failed"
        } else {
            "settings-keychain-failed"
        }
    }

    pub(crate) fn render_keychain_activity(&self, cx: &mut Context<Self>) -> AnyElement {
        if let Some(started) = self.keychain.status_check_started {
            let elapsed = teleark_i18n::format::format_duration_millis(
                self.locale(),
                started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
            );
            return div()
                .px_3()
                .py_2()
                .bg(theme::blue_pale())
                .text_xs()
                .debug_selector(|| "keychain-status-reading".into())
                .child(
                    self.tr_with(
                        "settings-keychain-activity",
                        MessageArgs::new()
                            .with("phase", self.tr("settings-keychain-checking").to_string())
                            .with("duration", elapsed.clone())
                            .with("activity", elapsed),
                    ),
                )
                .into_any_element();
        }
        let Some(progress) = &self.keychain.progress else {
            return div().into_any_element();
        };
        let snapshot = progress.snapshot();
        let phase = if self.keychain.error.is_some() {
            self.tr(self.keychain_error_id())
        } else if snapshot.finished {
            if snapshot.error == Some(teleark_core::ApplicationErrorKind::Cancelled) {
                self.tr("settings-keychain-cancelled")
            } else if snapshot.error.is_some() {
                self.tr("settings-keychain-failed")
            } else {
                self.tr("settings-keychain-saved")
            }
        } else if self.keychain.cancel_requested {
            self.tr("settings-keychain-cancelling")
        } else {
            self.tr(crate::screens::unlock::key_phase_id(snapshot.phase))
        };
        let elapsed = teleark_i18n::format::format_duration_millis(
            self.locale(),
            if snapshot.finished {
                snapshot
                    .last_activity
                    .duration_since(snapshot.phase_since)
                    .as_millis()
            } else {
                snapshot.phase_since.elapsed().as_millis()
            }
            .min(u128::from(u64::MAX)) as u64,
        );
        let activity = teleark_i18n::format::format_duration_millis(
            self.locale(),
            snapshot
                .last_activity
                .elapsed()
                .as_millis()
                .min(u128::from(u64::MAX)) as u64,
        );
        div()
            .px_3()
            .py_2()
            .flex()
            .items_center()
            .gap_2()
            .bg(theme::blue_pale())
            .text_xs()
            .child(
                div().flex_1().min_w_0().child(
                    self.tr_with(
                        "settings-keychain-activity",
                        MessageArgs::new()
                            .with("phase", phase.to_string())
                            .with("duration", elapsed)
                            .with("activity", activity),
                    ),
                ),
            )
            .when(self.keychain.busy, |row| {
                row.child(
                    components::button("keychain-cancel", self.tr("common-cancel"), None, false)
                        .xsmall()
                        .ghost()
                        .disabled(self.keychain.cancel_requested)
                        .debug_selector(|| "keychain-cancel".into())
                        .on_click(cx.listener(|app, _, _, cx| {
                            if let Some(p) = &app.keychain.progress {
                                p.cancel();
                                app.keychain.cancel_requested = true;
                            }
                            cx.notify();
                        })),
                )
            })
            .into_any_element()
    }

    pub(crate) fn render_keychain_confirmation(&self, cx: &mut Context<Self>) -> AnyElement {
        let popup = components::confirmation_surface("keychain-disable-dialog")
            .child(components::confirmation_heading(
                self.tr("settings-keychain-disable-title"),
                self.tr("settings-keychain-disable-warning"),
                IconName::Asterisk,
            ))
            .child(
                components::confirmation_actions()
                    .child(
                        components::button(
                            "keychain-keep",
                            self.tr("settings-keychain-keep"),
                            None,
                            true,
                        )
                        .debug_selector(|| "keychain-keep".into())
                        .on_click(cx.listener(|app, _, _, cx| {
                            app.keychain.confirmation = false;
                            cx.notify();
                        })),
                    )
                    .child(
                        components::button(
                            "keychain-disable-confirm",
                            self.tr("settings-keychain-disable-action"),
                            None,
                            false,
                        )
                        .debug_selector(|| "keychain-disable-confirm".into())
                        .on_click(cx.listener(|app, _, _, cx| app.change_keychain(false, cx))),
                    ),
            );
        gpui_kit::base::Dialog::new(cx)
            .focus_handle(self.modal_focus.clone())
            .flex()
            .items_center()
            .justify_center()
            .backdrop(div().absolute().inset_0().bg(theme::modal_backdrop()))
            .popup(popup)
            .close_on_backdrop_press(false)
            .on_cancel(|_, _, _| false)
            .on_ok(|_, _, _| false)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit as gpui;

    #[gpui::test]
    fn keychain_disable_requires_explicit_confirmation_and_cancel_preserves_default(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Settings);
        cx.simulate_resize(gpui::size(px(900.0), px(600.0)));
        app.update(cx, |app, cx| {
            assert_eq!(
                app.keychain.status.expect("preview status").enabled,
                cfg!(target_os = "macos")
            );
            app.request_keychain_toggle(cx);
            assert_eq!(app.keychain.confirmation, cfg!(target_os = "macos"));
            assert!(!app.keychain.busy);
        });
        if cfg!(target_os = "macos") {
            cx.run_until_parked();
            let keep = cx
                .debug_bounds("keychain-keep")
                .expect("keep recommended action reachable")
                .center();
            cx.simulate_click(keep, gpui::Modifiers::default());
            app.update(cx, |app, cx| {
                assert!(!app.keychain.confirmation);
                assert!(app.keychain.status.expect("preview status").enabled);
                app.request_keychain_toggle(cx);
            });
            cx.run_until_parked();
            let disable = cx
                .debug_bounds("keychain-disable-confirm")
                .expect("explicit disable reachable")
                .center();
            cx.simulate_click(disable, gpui::Modifiers::default());
            app.update(cx, |app, _| {
                assert!(!app.keychain.confirmation);
                assert!(!app.keychain.status.expect("preview status").enabled);
                assert!(
                    app.keychain.progress.is_none(),
                    "preview never writes credentials"
                );
            });
        }
    }

    #[gpui::test]
    fn unknown_status_can_retry_without_claiming_keychain_is_off(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Settings);
        for full in [false, true] {
            let width = if full { 1440.0 } else { 900.0 };
            let height = if full { 900.0 } else { 600.0 };
            cx.simulate_resize(gpui::size(px(width), px(height)));
            cx.update(|window, cx| {
                if window.is_fullscreen() != full {
                    window.toggle_fullscreen();
                }
                assert_eq!(window.is_fullscreen(), full);
                app.update(cx, |app, cx| {
                    theme::apply_appearance(AppearancePreference::Light, window, cx);
                    app.settings_section = SettingsSection::General;
                    app.keychain.status = None;
                    app.keychain.error = Some(teleark_core::ApplicationErrorKind::Persistence);
                    assert_eq!(app.keychain_error_id(), "settings-keychain-status-failed");
                    app.request_keychain_toggle(cx);
                    assert!(
                        !app.keychain.confirmation,
                        "unknown state cannot start migration"
                    );
                    cx.notify();
                });
            });
            cx.run_until_parked();
            let retry = cx
                .debug_bounds("settings-keychain-retry")
                .expect("read failure retry");
            assert!(retry.left() >= px(0.0) && retry.right() <= px(width));
            assert!(retry.top() >= px(0.0) && retry.bottom() <= px(height));
            cx.simulate_click(retry.center(), gpui::Modifiers::default());
            app.read_with(cx, |app, _| {
                assert!(
                    app.keychain.status.is_some(),
                    "preview retry restores known state"
                );
                assert!(app.keychain.error.is_none());
                assert!(
                    app.keychain.progress.is_none(),
                    "status retry never migrates credentials"
                );
            });
        }
    }

    #[gpui::test]
    fn migration_result_replaces_stale_state_and_cancellation_has_distinct_feedback(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Settings);
        app.update(cx, |app, cx| {
            let observed = KeychainStatus {
                enabled: false,
                supported: true,
                cleanup_pending: 0,
                unavailable_credentials: 0,
            };
            app.keychain.status = Some(observed);
            app.keychain.busy = true;
            app.finish_keychain_work(
                KeychainOutcome {
                    status: Err(ApplicationError::new(
                        teleark_core::ApplicationErrorKind::Persistence,
                    )),
                    change_error: None,
                },
                cx,
            );
            assert!(
                app.keychain.status.is_none(),
                "failed post-commit read must not show old On/Off"
            );
            assert!(!app.keychain.busy);
            assert_eq!(app.keychain_error_id(), "settings-keychain-status-failed");
            app.finish_keychain_work(
                KeychainOutcome {
                    status: Ok(observed),
                    change_error: Some(teleark_core::ApplicationErrorKind::Cancelled),
                },
                cx,
            );
            assert_eq!(app.keychain.status, Some(observed));
            assert_eq!(app.keychain_error_id(), "settings-keychain-cancelled");
        });
    }

    #[gpui::test]
    fn confirmation_and_active_cancel_remain_reachable_in_small_and_fullscreen_windows(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Settings);
        for full in [false, true] {
            let width = if full { 1440.0 } else { 900.0 };
            let height = if full { 900.0 } else { 600.0 };
            cx.simulate_resize(gpui::size(px(width), px(height)));
            cx.update(|window, cx| {
                if window.is_fullscreen() != full {
                    window.toggle_fullscreen();
                }
                assert_eq!(window.is_fullscreen(), full);
                app.update(cx, |app, cx| {
                    theme::apply_appearance(AppearancePreference::Light, window, cx);
                    app.keychain.busy = false;
                    app.keychain.status = Some(KeychainStatus {
                        enabled: true,
                        supported: true,
                        cleanup_pending: 0,
                        unavailable_credentials: 0,
                    });
                    app.request_keychain_toggle(cx);
                });
            });
            cx.run_until_parked();
            for selector in ["keychain-keep", "keychain-disable-confirm"] {
                let control = cx.debug_bounds(selector).expect("confirmation action");
                assert!(control.left() >= px(0.0) && control.right() <= px(width));
                assert!(control.top() >= px(0.0) && control.bottom() <= px(height));
            }
            app.update(cx, |app, cx| {
                app.keychain.confirmation = false;
                app.keychain.busy = true;
                app.keychain.cancel_requested = false;
                app.keychain.progress = Some(VaultKeyProgress::new());
                app.set_page(Page::Transfers, cx);
            });
            cx.run_until_parked();
            let cancel = cx
                .debug_bounds("keychain-cancel")
                .expect("global cancellation after navigation");
            assert!(cancel.right() <= px(width) && cancel.bottom() <= px(height));
            cx.simulate_click(cancel.center(), gpui::Modifiers::default());
            app.read_with(cx, |app, _| {
                assert!(
                    app.keychain.cancel_requested,
                    "acknowledge while blocked owner drains"
                );
                assert!(
                    app.keychain.busy,
                    "cancellation must not detach retained owner"
                );
            });
        }
    }

    #[gpui::test]
    fn unsupported_keychain_control_cannot_change_storage(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Settings);
        cx.simulate_resize(gpui::size(px(900.0), px(600.0)));
        app.update(cx, |app, cx| {
            app.keychain.status = Some(KeychainStatus {
                enabled: false,
                supported: false,
                cleanup_pending: 0,
                unavailable_credentials: 0,
            });
            app.request_keychain_toggle(cx);
            assert!(!app.keychain.confirmation);
            assert!(!app.keychain.busy);
            assert!(!app.keychain.status.expect("status").enabled);
        });
    }
}
