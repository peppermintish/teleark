//! Non-blocking key progress and explicit key-replacement confirmation.
use crate::{
    app::{TeleArkApp, VaultActivity},
    components,
    layout::LayoutPolicy,
    theme,
};
use gpui_kit::component::IconName;
use gpui_kit::component::{Disableable as _, button::ButtonVariants as _};
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, div, prelude::FluentBuilder as _, px,
};

impl TeleArkApp {
    pub(crate) fn render_vault_key_progress(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(progress) = &self.vault_key_progress else {
            return div().into_any_element();
        };
        let state = progress.snapshot();
        let elapsed = if state.finished {
            state.last_activity.duration_since(state.phase_since)
        } else {
            state.phase_since.elapsed()
        };
        div()
            .p_3()
            .id("vault-key-progress")
            .debug_selector(|| "vault-key-progress".into())
            .flex()
            .flex_col()
            .gap_2()
            .text_sm()
            .child(self.tr(key_phase_id(state.phase)))
            .child(
                div().text_xs().text_color(theme::text_secondary()).child(
                    self.tr_with(
                        "vault-key-phase-time",
                        teleark_i18n::MessageArgs::new()
                            .with("seconds", elapsed.as_secs().to_string())
                            .with("idle", state.last_activity.elapsed().as_secs().to_string()),
                    ),
                ),
            )
            .child(
                div()
                    .mt_2()
                    .pt_2()
                    .border_t_1()
                    .border_color(theme::border_subtle())
                    .flex()
                    .flex_col()
                    .gap_1()
                    .text_xs()
                    .child(self.tr("vault-key-timeline-title"))
                    .children(state.timeline.iter().map(|(phase, millis)| {
                        div().text_color(theme::text_secondary()).child(
                            self.tr_with(
                                "vault-key-timeline-event",
                                teleark_i18n::MessageArgs::new()
                                    .with("phase", self.tr(key_phase_id(*phase)).to_string())
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
            .when_some(state.error, |body, error| {
                body.child(
                    div()
                        .text_sm()
                        .text_color(theme::red())
                        .child(self.tr(match error {
                            teleark_core::ApplicationErrorKind::PermissionDenied => {
                                "managed-key-store-error"
                            }
                            teleark_core::ApplicationErrorKind::VaultKeyUnavailable => {
                                "managed-key-unavailable-help"
                            }
                            teleark_core::ApplicationErrorKind::Cancelled => {
                                "vault-error-cancelled"
                            }
                            _ => "vault-error-persistence",
                        })),
                )
            })
            .when(state.finished, |body| {
                body.child(
                    components::button(
                        "dismiss-key-progress",
                        self.tr("storage-guide-done"),
                        None,
                        false,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.vault_key_progress = None;
                        this.vault_key_details = false;
                        cx.notify();
                    })),
                )
            })
            .when(!state.finished, |body| {
                body.child(
                    components::button(
                        "cancel-key-creation",
                        self.tr("common-cancel"),
                        None,
                        false,
                    )
                    .mt_2()
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(progress) = &this.vault_key_progress {
                            progress.cancel();
                        }
                        cx.notify();
                    })),
                )
            })
            .into_any_element()
    }

    pub(crate) fn render_vault_key_details(&self, cx: &mut Context<Self>) -> AnyElement {
        components::inspector_panel("vault-key-inspector", 360.0)
            .absolute()
            .right_0()
            .top_0()
            .bottom(px(theme::STATUS_BAR_HEIGHT))
            .h_auto()
            .shadow_lg()
            .debug_selector(|| "vault-key-inspector".into())
            .child(
                div()
                    .flex_none()
                    .px_4()
                    .py_3()
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(theme::border_subtle())
                    .child(self.tr("vault-key-details-title"))
                    .child(
                        components::icon_button(
                            "vault-key-close-details",
                            IconName::Close,
                            self.tr("action-close-details"),
                        )
                        .ghost()
                        .debug_selector(|| "vault-key-close-details".into())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.vault_key_details = false;
                            cx.notify();
                        })),
                    ),
            )
            .child(components::inspector_body(
                "vault-key-events",
                &self.vault_key_scroll,
                self.render_vault_key_progress(cx),
            ))
            .into_any_element()
    }

    pub(crate) fn render_unlock_dialog(
        &self,
        layout: LayoutPolicy,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let working = self.vault_activity == VaultActivity::Working;
        let popup = components::card()
            .w(px(460.0))
            .max_h(px(layout.upload_dialog_height()))
            .id("new-key-confirmation")
            .overflow_y_scroll()
            .p_6()
            .child(div().text_lg().child(self.tr("vault-epoch-confirm-action")))
            .child(
                div()
                    .mt_3()
                    .text_sm()
                    .child(self.tr("vault-epoch-confirm-description")),
            )
            .when(working, |body| {
                body.child(self.render_vault_key_progress(cx))
            })
            .when_some(
                super::settings::vault_activity_message(self),
                |body, (message, tone)| {
                    body.child(
                        div()
                            .mt_3()
                            .text_sm()
                            .text_color(tone.foreground())
                            .child(message),
                    )
                },
            )
            .child(
                div()
                    .mt_4()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        components::button("new-key-cancel", self.tr("common-cancel"), None, false)
                            .on_click(cx.listener(|this, _, window, cx| {
                                if let Some(progress) = &this.vault_key_progress {
                                    progress.cancel();
                                }
                                this.vault_new_epoch_confirmation = false;
                                this.clear_vault_inputs(window, cx);
                                cx.notify();
                            })),
                    )
                    .child(
                        components::button(
                            "new-key-confirm",
                            self.tr("vault-epoch-confirm-action"),
                            None,
                            true,
                        )
                        .disabled(working)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.initialize_vault(window, cx)),
                        ),
                    ),
            );
        gpui_kit::base::Dialog::new(cx)
            .focus_handle(self.modal_focus.clone())
            .flex()
            .items_center()
            .justify_center()
            .backdrop(div().absolute().inset_0().bg(gpui_kit::rgba(0x10182060)))
            .popup(popup)
            .close_on_backdrop_press(false)
            .into_any_element()
    }
}

pub(crate) fn key_phase_id(phase: teleark_runtime::VaultKeyPhase) -> &'static str {
    use teleark_runtime::VaultKeyPhase;
    match phase {
        VaultKeyPhase::Queued => "vault-key-phase-queued",
        VaultKeyPhase::Loading => "managed-key-phase-loading",
        VaultKeyPhase::CheckingChannel => "managed-key-phase-checking-channel",
        VaultKeyPhase::Securing => "managed-key-phase-securing",
        VaultKeyPhase::Generating => "vault-key-phase-generating",
        VaultKeyPhase::WrappingPassword => "vault-key-phase-password",
        VaultKeyPhase::WrappingRecovery => "vault-key-phase-recovery",
        VaultKeyPhase::Saving => "vault-key-phase-saving",
        VaultKeyPhase::Completed => "vault-key-phase-completed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui_kit::test]
    fn key_activity_opens_from_the_status_bar_without_a_bottom_strip(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, crate::app::Page::Transfers);
        app.update(cx, |app, cx| {
            assert_eq!(app.locale(), teleark_i18n::SupportedLocale::EnUs);
            app.vault_key_progress = Some(teleark_runtime::VaultKeyProgress::new());
            app.vault_activity = VaultActivity::Working;
            cx.notify();
        });
        for (width, height) in [(900.0, 600.0), (1920.0, 1080.0)] {
            cx.simulate_resize(gpui_kit::size(px(width), px(height)));
            cx.run_until_parked();
            let bar = cx
                .debug_bounds("global-background-status")
                .expect("status bar");
            let trigger = cx
                .debug_bounds("shell-key-progress")
                .expect("key status button");
            assert!(trigger.top() >= bar.top() && trigger.bottom() <= bar.bottom());
            assert!(cx.debug_bounds("vault-key-inspector").is_none());
            cx.simulate_click(trigger.center(), gpui_kit::Modifiers::default());
            cx.run_until_parked();
            let panel = cx
                .debug_bounds("vault-key-inspector")
                .expect("key inspector");
            let events = cx
                .debug_bounds("vault-key-progress")
                .expect("phase timeline");
            assert_eq!(panel.bottom(), bar.top());
            assert!(panel.right() <= px(width));
            assert!(events.top() >= panel.top() && events.bottom() <= panel.bottom());
            let close = cx
                .debug_bounds("vault-key-close-details")
                .expect("close key inspector");
            cx.simulate_click(close.center(), gpui_kit::Modifiers::default());
            cx.run_until_parked();
            assert!(cx.debug_bounds("vault-key-inspector").is_none());
            assert!(cx.debug_bounds("shell-key-progress").is_some());
        }
    }
}
