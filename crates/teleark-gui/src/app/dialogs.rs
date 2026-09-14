//! Catalog activity is independent from authentication and selected-file work.
use super::*;
use gpui_kit::component::{Sizable as _, button::ButtonVariants as _};
use teleark_core::ApplicationErrorKind;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Phase {
    #[default]
    Idle,
    RestoringUploads,
    Reading,
    Waiting,
    Saving,
    Complete,
    Failed,
    Cancelled,
}

#[derive(Default)]
pub(crate) struct DialogLoad {
    pub(crate) ready: bool,
    pub(crate) details: bool,
    history_expanded: bool,
    scroll: gpui_kit::ScrollHandle,
    phase: Phase,
    error: Option<ApplicationErrorKind>,
    attempt: u8,
    generation: u64,
    changed: Option<std::time::Instant>,
    pub(super) history:
        std::collections::VecDeque<(Phase, Option<ApplicationErrorKind>, std::time::Instant)>,
    started: Option<std::time::Instant>,
    cancellation: teleark_runtime::TelegramScanCancellation,
}
impl Drop for DialogLoad {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}
impl DialogLoad {
    pub(super) fn phase(&self) -> Phase {
        self.phase
    }
    pub(crate) fn active(&self) -> bool {
        matches!(
            self.phase,
            Phase::RestoringUploads | Phase::Reading | Phase::Waiting | Phase::Saving
        )
    }
    pub(crate) fn needs_initial_load(&self) -> bool {
        self.phase == Phase::Idle
    }
    pub(crate) fn has_activity(&self) -> bool {
        self.phase != Phase::Idle
    }
    pub(super) fn transition(&mut self, phase: Phase, error: Option<ApplicationErrorKind>) {
        let now = std::time::Instant::now();
        self.started.get_or_insert(now);
        self.phase = phase;
        self.error = error;
        self.changed = Some(now);
        self.history.push_back((phase, error, now));
        // Three attempts produce at most eight events; each new manual run resets the journal.
        while self.history.len() > 16 {
            self.history.pop_front();
        }
    }
    fn accepts(&self, generation: u64) -> bool {
        self.generation == generation && self.active() && !self.cancellation.is_cancelled()
    }
}
fn retry_delay(attempt: u8, kind: ApplicationErrorKind) -> Option<Duration> {
    (attempt < 2
        && matches!(
            kind,
            ApplicationErrorKind::Server | ApplicationErrorKind::Network
        ))
    .then(|| Duration::from_secs(2_u64 << attempt))
}
pub(super) fn phase_id(phase: Phase) -> &'static str {
    match phase {
        Phase::RestoringUploads => "transfer-history-restoring",
        Phase::Idle | Phase::Reading => "dialogs-reading",
        Phase::Waiting => "dialogs-waiting",
        Phase::Saving => "dialogs-saving",
        Phase::Complete => "dialogs-complete",
        Phase::Failed => "dialogs-failed",
        Phase::Cancelled => "dialogs-cancelled",
    }
}
fn phase_presentation(phase: Phase) -> (&'static str, components::Tone, IconName) {
    use components::Tone;
    match phase {
        Phase::Idle => (
            "activity-state-queued",
            Tone::Neutral,
            IconName::LoaderCircle,
        ),
        Phase::RestoringUploads | Phase::Reading => {
            ("activity-state-running", Tone::Blue, IconName::LoaderCircle)
        }
        Phase::Waiting => ("activity-state-waiting", Tone::Amber, IconName::Redo2),
        Phase::Saving => ("activity-state-saving", Tone::Blue, IconName::HardDrive),
        Phase::Complete => (
            "activity-state-complete",
            Tone::Green,
            IconName::CircleCheck,
        ),
        Phase::Failed => (
            "activity-state-failed",
            Tone::Amber,
            IconName::TriangleAlert,
        ),
        Phase::Cancelled => ("activity-state-cancelled", Tone::Neutral, IconName::CircleX),
    }
}

impl TeleArkApp {
    fn apply_dialog_failure(
        &mut self,
        generation: u64,
        account_id: i64,
        attempt: u8,
        kind: ApplicationErrorKind,
        cx: &mut Context<Self>,
    ) -> Option<Duration> {
        if !self.dialogs.accepts(generation)
            || self.telegram_account.as_ref().map(|a| a.id) != Some(account_id)
        {
            return None;
        }
        let delay = retry_delay(attempt, kind);
        self.dialogs.attempt = attempt;
        self.dialogs.transition(
            if delay.is_some() {
                Phase::Waiting
            } else {
                Phase::Failed
            },
            Some(kind),
        );
        cx.notify();
        delay
    }
    pub(crate) fn reset_dialog_load(&mut self, cx: &mut Context<Self>) {
        self.cancel_dialog_load(cx);
        let generation = self.dialogs.generation;
        self.dialogs = DialogLoad::default();
        self.dialogs.generation = generation;
    }
    pub(crate) fn cancel_dialog_load(&mut self, cx: &mut Context<Self>) {
        self.dialogs.cancellation.cancel();
        if self.dialogs.active() {
            self.dialogs.transition(Phase::Cancelled, None);
        }
        self.dialogs.generation = self.dialogs.generation.wrapping_add(1);
        cx.notify();
    }
    pub(crate) fn load_telegram_dialogs(&mut self, cx: &mut Context<Self>) {
        if self.visual_preview
            || self.telegram.is_none()
            || self.library.is_none()
            || self.dialogs.active()
            || self.telegram_activity == TelegramActivity::Working
        {
            return;
        }
        let Some(account) = self.telegram_account.clone() else {
            return;
        };
        self.dialogs.generation = self.dialogs.generation.wrapping_add(1);
        let generation = self.dialogs.generation;
        let account_id = account.id;
        self.dialogs.cancellation = Default::default();
        self.dialogs.history.clear();
        self.dialogs.started = None;
        self.dialogs.attempt = 0;
        self.dialogs.transition(
            if self.vault.is_some() {
                Phase::RestoringUploads
            } else {
                Phase::Reading
            },
            None,
        );
        self.start_channel_sync(cx);
        let transfers = self.transfers.clone();

        // Acknowledge before dispatching any filesystem/network work.
        cx.notify();
        // Submit in frontend generation order so a delayed waiter cannot replace a newer account's history.
        let history_job = self
            .vault
            .as_ref()
            .map(|vault| vault.submit_upload_history_restore(account_id));
        self.dialogs_task = Some(cx.spawn(async move |this, cx| {
            if let Some(job) = history_job {
                let result = cx.background_spawn(async move { job?.wait() }).await;
                let Some(entity) = this.upgrade() else { return };
                let ready = entity.update(cx, |app, cx| {
                    if !app.dialogs.accepts(generation)
                        || app.telegram_account.as_ref().map(|a| a.id) != Some(account_id)
                    {
                        return false;
                    }
                    match result {
                        Ok(()) => {
                            app.dialogs.transition(Phase::Reading, None);
                            app.resume_durable_uploads(cx);
                            cx.notify();
                            true
                        }
                        Err(error) => {
                            app.dialogs.transition(Phase::Failed, Some(error.kind()));
                            cx.notify();
                            false
                        }
                    }
                });
                if !ready {
                    return;
                }
            }
            for attempt in 0..3 {
                if let Some(entity) = this.upgrade() {
                    entity.update(cx, |app, cx| {
                        if app.dialogs.accepts(generation) {
                            app.dialogs.transition(Phase::Saving, None);
                            cx.notify();
                        }
                    });
                }
                let transfers = transfers.clone();
                let result = cx
                    .background_spawn(async move {
                        if let Some(transfers) = transfers {
                            transfers.activate_account(account_id)?;
                        }
                        Ok::<_, ApplicationError>(())
                    })
                    .await;
                let Some(entity) = this.upgrade() else { return };
                let current = entity.update(cx, |app, _| {
                    app.dialogs.accepts(generation)
                        && app.telegram_account.as_ref().map(|a| a.id) == Some(account_id)
                });
                if !current {
                    return;
                }
                match result {
                    Err(error) => {
                        let delay = entity.update(cx, |app, cx| {
                            app.apply_dialog_failure(
                                generation,
                                account_id,
                                attempt,
                                error.kind(),
                                cx,
                            )
                        });
                        let Some(delay) = delay else { return };
                        drop(entity);
                        cx.background_executor().timer(delay).await;
                        let Some(entity) = this.upgrade() else { return };
                        if !entity.update(cx, |app, cx| {
                            if !app.dialogs.accepts(generation)
                                || app.telegram_account.as_ref().map(|a| a.id) != Some(account_id)
                            {
                                return false;
                            }
                            app.dialogs.attempt = attempt + 1;
                            app.dialogs.transition(Phase::Reading, None);
                            cx.notify();
                            true
                        }) {
                            return;
                        }
                    }
                    Ok(()) => {
                        entity.update(cx, |app, cx| {
                            if !app.dialogs.accepts(generation)
                                || app.telegram_account.as_ref().map(|a| a.id) != Some(account_id)
                            {
                                return;
                            }
                            app.dialogs.ready = true;
                            app.dialogs.transition(Phase::Complete, None);
                            app.transfers_account_ready = true;
                            app.refresh_storage_channel(cx);
                            cx.notify();
                        });
                        return;
                    }
                }
            }
        }));
    }
    fn dialog_timing(&self) -> gpui_kit::SharedString {
        self.dialogs.changed.map_or_else(
            || self.tr("sync-no-events"),
            |at| {
                self.tr_with(
                    "sync-event-time",
                    MessageArgs::new().with("time", self.sync_event_time(at).to_string()),
                )
            },
        )
    }

    pub(super) fn render_dialog_sync_activity(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(theme::text_primary())
                    .child(self.tr(phase_id(self.dialogs.phase))),
            )
            .child(self.dialog_timing())
            .when_some(self.dialogs.error, |body, error| {
                body.child(self.application_error_message(error))
            })
            .child(
                components::button(
                    "sync-dialogs-action",
                    self.tr(if self.dialogs.active() {
                        "common-cancel"
                    } else {
                        "common-retry"
                    }),
                    None,
                    false,
                )
                .on_click(cx.listener(|app, _, _, cx| {
                    if app.dialogs.active() {
                        app.cancel_dialog_load(cx);
                    } else {
                        app.load_telegram_dialogs(cx);
                    }
                })),
            )
            .into_any_element()
    }
    pub(crate) fn render_dialog_details(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        components::inspector_panel("dialogs-inspector", 340.0)
            .absolute()
            .right_0()
            .top_0()
            .bottom(px(28.0))
            .h_auto()
            .shadow_lg()
            .debug_selector(|| "dialogs-inspector".into())
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .p_2()
                    .child(self.tr("dialogs-details"))
                    .child(
                        components::icon_button(
                            "dialogs-close",
                            IconName::Close,
                            self.tr("action-close-details"),
                        )
                        .ghost()
                        .on_click(cx.listener(|app, _, _, cx| {
                            app.dialogs.details = false;
                            cx.notify();
                        })),
                    ),
            )
            .child(components::inspector_body(
                "dialogs-events",
                &self.dialogs.scroll,
                div().p_3().child(self.render_dialog_activity(cx)),
            ))
            .into_any_element()
    }
    pub(crate) fn render_dialog_activity(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let (state, tone, icon) = phase_presentation(self.dialogs.phase);
        let action = components::button(
            "dialogs-action",
            self.tr(if self.dialogs.active() {
                "common-cancel"
            } else if self.dialogs.phase == Phase::Complete {
                "activity-refresh"
            } else {
                "common-retry"
            }),
            Some(if self.dialogs.active() {
                IconName::Close
            } else {
                IconName::Redo2
            }),
            !self.dialogs.active(),
        )
        .flex_none()
        .debug_selector(|| "dialogs-action".into())
        .on_click(cx.listener(|app, _, _, cx| {
            if app.dialogs.active() {
                app.cancel_dialog_load(cx);
            } else {
                app.load_telegram_dialogs(cx);
            }
        }));
        let current =
            components::activity_card(self.tr("dialogs-task-title"), self.tr(state), tone, icon)
                .debug_selector(|| "dialogs-current-card".into())
                .child(
                    div()
                        .px_4()
                        .pb_4()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme::text_primary())
                                .child(self.tr(phase_id(self.dialogs.phase))),
                        )
                        .when_some(self.dialogs.error, |body, error| {
                            body.child(
                                div()
                                    .rounded(theme::RADIUS_MEDIUM)
                                    .bg(theme::canvas())
                                    .border_1()
                                    .border_color(theme::border_subtle())
                                    .p_3()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .debug_selector(|| "dialogs-reason".into())
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(theme::text_muted())
                                            .child(self.tr("activity-last-response")),
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(theme::text_secondary())
                                            .child(self.application_error_message(error)),
                                    ),
                            )
                        }),
                )
                .child(
                    div()
                        .px_4()
                        .py_3()
                        .border_t_1()
                        .border_color(theme::border_subtle())
                        .bg(theme::canvas())
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .text_xs()
                                .text_color(theme::text_secondary())
                                .child(self.dialog_timing()),
                        )
                        .when(self.dialogs.phase != Phase::Complete, |footer| {
                            footer.child(action)
                        }),
                );
        div()
            .flex()
            .flex_col()
            .gap_3()
            .min_w_0()
            .debug_selector(|| "dialogs-activity".into())
            .child(current)
            .when(!self.dialogs.history.is_empty(), |body| {
                body.child(self.render_dialog_history(cx))
            })
            .into_any_element()
    }

    fn render_dialog_history(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let count = self.dialogs.history.len();
        let visible = if self.dialogs.history_expanded {
            count
        } else {
            count.min(3)
        };
        components::card()
            .rounded(theme::RADIUS_LARGE)
            .overflow_hidden()
            .debug_selector(|| "dialogs-history-card".into())
            .child(
                div()
                    .px_4()
                    .py_3()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                    .text_color(theme::text_primary())
                                    .child(self.tr("activity-history-title")),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme::text_muted())
                                    .child(self.tr("sync-event-times-local")),
                            ),
                    ),
            )
            .children(
                self.dialogs
                    .history
                    .iter()
                    .enumerate()
                    .rev()
                    .take(visible)
                    .map(|(index, (phase, error, at))| {
                        let (_, tone, _) = phase_presentation(*phase);
                        let summary = format!(
                            "{} · {}{}",
                            self.sync_event_time(*at),
                            self.tr(phase_id(*phase)),
                            error.map_or_else(String::new, |error| format!(
                                " · {}",
                                self.application_error_message(error)
                            ))
                        );
                        components::list_summary(("dialogs-history-event", index), summary)
                            .debug_selector(move || format!("dialogs-history-event-{index}"))
                            .px_4()
                            .border_t_1()
                            .border_color(theme::border_subtle())
                            .text_color(tone.foreground())
                    }),
            )
            .when(count > 3, |card| {
                card.child(
                    components::list_footer("dialogs-history-footer")
                        .border_t_1()
                        .border_color(theme::border_subtle())
                        .child(
                            components::button(
                                "dialogs-history-toggle",
                                self.tr_with(
                                    if self.dialogs.history_expanded {
                                        "activity-history-show-less"
                                    } else {
                                        "activity-history-show-all"
                                    },
                                    MessageArgs::new().with(
                                        "count",
                                        teleark_i18n::format::format_integer(
                                            self.locale(),
                                            count as u64,
                                        ),
                                    ),
                                ),
                                Some(if self.dialogs.history_expanded {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                }),
                                false,
                            )
                            .ghost()
                            .xsmall()
                            .h(theme::LIST_CONTROL_SIZE)
                            .w_full()
                            .debug_selector(|| "dialogs-history-toggle".into())
                            .on_click(cx.listener(|app, _, _, cx| {
                                app.dialogs.history_expanded = !app.dialogs.history_expanded;
                                cx.notify();
                            })),
                        ),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::gpui;
    #[test]
    fn retries_are_bounded_and_permanent_errors_stop() {
        for kind in [ApplicationErrorKind::Network, ApplicationErrorKind::Server] {
            assert_eq!(retry_delay(0, kind), Some(Duration::from_secs(2)));
            assert_eq!(retry_delay(1, kind), Some(Duration::from_secs(4)));
            assert_eq!(retry_delay(2, kind), None);
        }
        for kind in [
            ApplicationErrorKind::Authorization,
            ApplicationErrorKind::Persistence,
            ApplicationErrorKind::Cancelled,
        ] {
            assert_eq!(retry_delay(0, kind), None);
        }
    }
    #[gpui::test]
    fn catalog_failure_preserves_login_and_data_and_cancel_rejects_old_result(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Account);
        app.update(cx, |app, cx| {
            let account = app.telegram_account.clone();
            let chats = app.telegram_chats.clone();
            app.dialogs.transition(Phase::Reading, None);
            assert!(app.dialogs.active());
            let generation = app.dialogs.generation;
            let account_id = account.as_ref().expect("preview account").id;
            assert_eq!(
                app.apply_dialog_failure(
                    generation,
                    account_id + 1,
                    0,
                    ApplicationErrorKind::Server,
                    cx
                ),
                None
            );
            assert_eq!(app.dialogs.phase, Phase::Reading);
            assert_eq!(
                app.apply_dialog_failure(
                    generation,
                    account_id,
                    0,
                    ApplicationErrorKind::Server,
                    cx
                ),
                Some(Duration::from_secs(2))
            );
            app.cancel_dialog_load(cx);
            assert!(!app.dialogs.accepts(generation));
            let events = app.dialogs.history.len();
            assert_eq!(
                app.apply_dialog_failure(
                    generation,
                    account_id,
                    2,
                    ApplicationErrorKind::Server,
                    cx
                ),
                None
            );
            assert_eq!(app.dialogs.history.len(), events);
            assert_eq!(app.telegram_account, account);
            assert_eq!(app.telegram_chats, chats);
            assert_eq!(app.telegram_activity, TelegramActivity::Idle);
            assert!(app.telegram_error_message().is_none());
            assert_eq!(app.dialogs.history.len(), 3);
            app.reset_dialog_load(cx);
            app.dialogs.generation = app.dialogs.generation.wrapping_add(1);
            app.dialogs.transition(Phase::Reading, None);
            assert!(
                !app.dialogs.accepts(generation),
                "same-account relogin cannot reuse an old request generation"
            );
        });
    }
    #[gpui::test]
    fn catalog_status_and_cancel_are_reachable_in_compact_locales(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Transfers);
        cx.simulate_resize(gpui::size(px(900.0), px(600.0)));
        {
            let locale = SupportedLocale::EnUs;
            app.update(cx, |app, cx| {
                app.localizer = Localizer::new(locale).expect("catalog");
                app.dialogs.cancellation = Default::default();
                app.dialogs.details = false;
                app.dialogs
                    .transition(Phase::Waiting, Some(ApplicationErrorKind::Server));
                cx.notify();
            });
            cx.run_until_parked();
            let status = cx
                .debug_bounds("dialogs-status")
                .expect("visible on Transfers");
            assert!(status.size.width > px(0.0));
            cx.simulate_click(status.center(), gpui::Modifiers::default());
            cx.run_until_parked();
            assert!(
                cx.debug_bounds("dialogs-inspector").is_none(),
                "status is display-only"
            );
            app.update(cx, |app, cx| {
                app.dialogs.details = true;
                cx.notify();
            });
            cx.run_until_parked();
            assert!(cx.debug_bounds("dialogs-inspector").is_some());
            let action = cx
                .debug_bounds("dialogs-action")
                .expect("visible cancel")
                .center();
            assert!(action.x < px(900.0) && action.y < px(600.0));
            cx.simulate_click(action, gpui::Modifiers::default());
            cx.run_until_parked();
            app.update(cx, |app, _| {
                assert_eq!(app.dialogs.phase, Phase::Cancelled);
                assert_eq!(app.page, Page::Transfers);
                app.dialogs.history.clear();
            });
        }
    }
    #[gpui::test]
    fn storage_activity_cards_separate_current_reason_actions_and_history(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Storage);
        cx.simulate_resize(gpui::size(px(900.0), px(600.0)));
        {
            let locale = SupportedLocale::EnUs;
            for appearance in [AppearancePreference::Light, AppearancePreference::Dark] {
                for phase in [
                    Phase::Reading,
                    Phase::Waiting,
                    Phase::Saving,
                    Phase::Failed,
                    Phase::Complete,
                    Phase::Cancelled,
                ] {
                    cx.update(|window, cx| {
                        app.update(cx, |app, cx| {
                            app.localizer = Localizer::new(locale).expect("catalog");
                            theme::apply_appearance(appearance, window, cx);
                            app.storage_status = teleark_runtime::StorageChannelStatus::Missing;
                            app.storage_loading = false;
                            app.storage_error = None;
                            app.dialogs.ready = false;
                            app.dialogs.details = false;
                            app.dialogs.history.clear();
                            app.dialogs.transition(Phase::Reading, None);
                            app.dialogs.transition(
                                phase,
                                matches!(phase, Phase::Waiting | Phase::Failed)
                                    .then_some(ApplicationErrorKind::Server),
                            );
                            cx.notify();
                        })
                    });
                    cx.run_until_parked();
                    let current = cx
                        .debug_bounds("dialogs-current-card")
                        .expect("current task card");
                    let history = cx
                        .debug_bounds("dialogs-history-card")
                        .expect("separate history card");
                    if phase == Phase::Complete {
                        assert!(
                            cx.debug_bounds("dialogs-action").is_none(),
                            "completed automatic work has no refresh button"
                        );
                        continue;
                    }
                    let action = cx.debug_bounds("dialogs-action").expect("task action");
                    assert!(
                        current.bottom() < history.top(),
                        "clear gap between current work and history"
                    );
                    assert!(
                        action.bottom() <= px(600.0) && action.right() <= px(900.0),
                        "primary task action stays visible in the initial compact viewport"
                    );
                    assert!(action.top() >= current.top() && action.bottom() <= current.bottom());
                    if matches!(phase, Phase::Waiting | Phase::Failed) {
                        let reason = cx
                            .debug_bounds("dialogs-reason")
                            .expect("separate response panel");
                        assert!(reason.top() > current.top() && reason.bottom() < action.top());
                    }
                }
            }
        }
    }
    #[gpui::test]
    fn expanding_activity_history_keeps_current_phase_and_retained_events(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Transfers);
        cx.simulate_resize(gpui::size(px(900.0), px(600.0)));
        app.update(cx, |app, cx| {
            app.dialogs.details = true;
            for phase in [
                Phase::Reading,
                Phase::Waiting,
                Phase::Reading,
                Phase::Waiting,
                Phase::Reading,
                Phase::Failed,
            ] {
                app.dialogs.transition(phase, None);
            }
            cx.notify();
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("dialogs-history-event-5").is_some());
        assert!(cx.debug_bounds("dialogs-history-event-0").is_none());
        let inspector = cx.debug_bounds("dialogs-inspector").expect("inspector");
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: inspector.center(),
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.0), px(-1600.0))),
            ..Default::default()
        });
        cx.run_until_parked();
        let toggle = cx
            .debug_bounds("dialogs-history-toggle")
            .expect("history disclosure");
        assert_eq!(toggle.size.height, theme::LIST_CONTROL_SIZE);
        assert_eq!(
            cx.debug_bounds("dialogs-history-footer")
                .expect("fixed footer")
                .size
                .height,
            theme::ROW_HEIGHT
        );
        assert_eq!(
            cx.debug_bounds("dialogs-history-event-5")
                .expect("history row")
                .size
                .height,
            theme::ROW_HEIGHT
        );
        cx.simulate_click(toggle.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        assert!(cx.debug_bounds("dialogs-history-event-0").is_some());
        app.update(cx, |app, _| {
            assert!(app.dialogs.history_expanded);
            assert_eq!(app.dialogs.history.len(), 6);
            assert_eq!(app.dialogs.phase, Phase::Failed);
            assert_eq!(app.page, Page::Transfers);
        });
    }
}
