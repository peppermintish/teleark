//! Non-blocking key progress and explicit key-replacement confirmation.
use crate::{
    app::{TeleArkApp, VaultActivity},
    components,
    layout::LayoutPolicy,
};
use gpui_kit::component::Disableable as _;
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
            .max_h(px(140.0))
            .id("vault-key-progress")
            .overflow_y_scroll()
            .text_sm()
            .child(self.tr(key_phase_id(state.phase)))
            .child(
                div().mt_1().text_xs().child(
                    self.tr_with(
                        "vault-key-phase-time",
                        teleark_i18n::MessageArgs::new()
                            .with("seconds", elapsed.as_secs().to_string())
                            .with("idle", state.last_activity.elapsed().as_secs().to_string()),
                    ),
                ),
            )
            .child(
                div().mt_1().text_xs().child(
                    state
                        .timeline
                        .iter()
                        .map(|(phase, millis)| {
                            format!("{} · {}s", self.tr(key_phase_id(*phase)), millis / 1000)
                        })
                        .collect::<Vec<_>>()
                        .join(" → "),
                ),
            )
            .when_some(state.error, |body, error| {
                body.child(div().mt_1().text_sm().child(self.tr(match error {
                    teleark_core::ApplicationErrorKind::PermissionDenied => {
                        "managed-key-store-error"
                    }
                    teleark_core::ApplicationErrorKind::VaultKeyUnavailable => {
                        "managed-key-unavailable-help"
                    }
                    teleark_core::ApplicationErrorKind::Cancelled => "vault-error-cancelled",
                    _ => "vault-error-persistence",
                })))
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

fn key_phase_id(phase: teleark_runtime::VaultKeyPhase) -> &'static str {
    use teleark_runtime::VaultKeyPhase;
    match phase {
        VaultKeyPhase::Queued => "vault-key-phase-queued",
        VaultKeyPhase::Loading => "managed-key-phase-loading",
        VaultKeyPhase::Securing => "managed-key-phase-securing",
        VaultKeyPhase::Generating => "vault-key-phase-generating",
        VaultKeyPhase::WrappingPassword => "vault-key-phase-password",
        VaultKeyPhase::WrappingRecovery => "vault-key-phase-recovery",
        VaultKeyPhase::Saving => "vault-key-phase-saving",
        VaultKeyPhase::Completed => "vault-key-phase-completed",
    }
}
