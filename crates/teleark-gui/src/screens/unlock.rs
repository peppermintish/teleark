//! A focused modal that resumes the action that asked for a key.
use crate::assets::Symbol;
use crate::{
    app::{TeleArkApp, VaultActivity},
    components,
    layout::LayoutPolicy,
    theme,
};
use gpui_kit::component::{
    Disableable as _, Icon, IconName, button::ButtonVariants as _, input::Input,
};
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, div, prelude::FluentBuilder as _, px,
};

impl TeleArkApp {
    pub(crate) fn render_unlock_dialog(
        &self,
        layout: LayoutPolicy,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let working = self.vault_activity == VaultActivity::Working;
        let setup = !self.vault_status.configured;
        let recovery = self.vault_recovery_secret.is_some();
        let title = self.tr(if recovery {
            "vault-recovery-save-now-title"
        } else if setup {
            "vault-create-action"
        } else {
            "vault-unlock-action"
        });
        let popup = components::card()
            .relative()
            .w(px(460.0))
            .max_h(px(layout.upload_dialog_height()))
            .shadow_lg()
            .id("unlock-popup")
            .overflow_y_scroll()
            .h_auto()
            .p_6()
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap_4()
                    .child(
                        div()
                            .size(px(44.0))
                            .flex_shrink_0()
                            .rounded(px(12.0))
                            .bg(theme::blue_soft())
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                Icon::new(Symbol::Lock)
                                    .size(px(23.0))
                                    .text_color(theme::blue()),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_normal()
                            .child(
                                div()
                                    .text_size(px(20.0))
                                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                    .child(title),
                            )
                            .child(
                                div()
                                    .mt_2()
                                    .text_sm()
                                    .text_color(theme::text_secondary())
                                    .child(self.tr("unlock-return-note")),
                            ),
                    ),
            )
            .when(!recovery, |popup| {
                popup
                    .child(
                        div()
                            .mt_5()
                            .text_xs()
                            .text_color(theme::text_secondary())
                            .child(self.tr(if setup {
                                "vault-create-password-label"
                            } else {
                                "vault-password-label"
                            })),
                    )
                    .child(
                        div()
                            .mt_2()
                            .child(Input::new(&self.vault_password).mask_toggle().h(px(36.0))),
                    )
                    .when(setup, |popup| {
                        popup
                            .child(
                                div()
                                    .mt_3()
                                    .text_xs()
                                    .text_color(theme::text_secondary())
                                    .child(self.tr("vault-confirm-password-label")),
                            )
                            .child(
                                div().mt_2().child(
                                    Input::new(&self.vault_new_password)
                                        .mask_toggle()
                                        .h(px(36.0)),
                                ),
                            )
                    })
                    .child(
                        components::button(
                            "unlock-submit",
                            self.tr(if setup {
                                "vault-create-action"
                            } else {
                                "vault-unlock-password-action"
                            }),
                            None,
                            true,
                        )
                        .mt_4()
                        .w_full()
                        .disabled(working)
                        .on_click(cx.listener(|this, _, window, cx| {
                            if this.vault_status.configured {
                                this.unlock_vault_with_password(window, cx);
                            } else {
                                this.initialize_vault(window, cx);
                            }
                        })),
                    )
                    .child(
                        components::button(
                            "unlock-recovery-disclosure",
                            self.tr("unlock-use-recovery"),
                            Some(IconName::ChevronDown),
                            false,
                        )
                        .mt_3()
                        .ghost()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.vault_advanced_expanded = !this.vault_advanced_expanded;
                            cx.notify();
                        })),
                    )
                    .when(self.vault_advanced_expanded, |popup| {
                        popup
                            .child(
                                div().mt_2().child(
                                    Input::new(&self.vault_recovery_key)
                                        .mask_toggle()
                                        .h(px(36.0)),
                                ),
                            )
                            .child(
                                components::button(
                                    "unlock-recovery",
                                    self.tr(if setup {
                                        "vault-restore-action"
                                    } else {
                                        "vault-unlock-recovery-action"
                                    }),
                                    None,
                                    false,
                                )
                                .mt_3()
                                .disabled(working)
                                .on_click(cx.listener(
                                    |this, _, window, cx| {
                                        if this.vault_status.configured {
                                            this.unlock_vault_with_recovery(window, cx);
                                        } else {
                                            this.restore_vault_with_recovery(window, cx);
                                        }
                                    },
                                )),
                            )
                    })
            })
            .when_some(self.vault_recovery_secret.as_ref(), |popup, secret| {
                popup
                    .child(
                        div()
                            .mt_4()
                            .text_sm()
                            .text_color(theme::text_secondary())
                            .child(self.tr("vault-recovery-save-now-description")),
                    )
                    .child(
                        div()
                            .mt_4()
                            .p_3()
                            .rounded(theme::RADIUS_SMALL)
                            .bg(theme::blue_pale())
                            .text_xs()
                            .child(secret.clone()),
                    )
                    .child(
                        components::button(
                            "unlock-export-recovery",
                            self.tr("vault-export-recovery"),
                            Some(IconName::ExternalLink),
                            true,
                        )
                        .mt_4()
                        .w_full()
                        .on_click(cx.listener(|this, _, _, cx| this.export_vault_recovery_key(cx))),
                    )
                    .child(
                        components::button(
                            "unlock-continue",
                            self.tr("unlock-recovery-saved"),
                            None,
                            false,
                        )
                        .mt_3()
                        .w_full()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.hide_vault_recovery_key(cx);
                            this.resume_unlock_intent(cx);
                        })),
                    )
            })
            .when_some(
                super::settings::vault_activity_message(self),
                |popup, (message, tone)| {
                    popup.child(
                        div()
                            .mt_3()
                            .text_sm()
                            .text_color(tone.foreground())
                            .child(message),
                    )
                },
            )
            .child(
                div().mt_4().flex().justify_end().child(
                    components::button("unlock-cancel", self.tr("action-cancel"), None, false)
                        .ghost()
                        .disabled(working)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.dismiss_unlock(window, cx)),
                        ),
                ),
            );
        let cancel = cx.listener(|this, _, window, cx| {
            if this.vault_activity != VaultActivity::Working {
                this.dismiss_unlock(window, cx);
            }
        });
        let submit = cx.listener(|this, _, window, cx| {
            if this.vault_activity != VaultActivity::Working && this.vault_recovery_secret.is_none()
            {
                if this.vault_status.configured {
                    this.unlock_vault_with_password(window, cx);
                } else {
                    this.initialize_vault(window, cx);
                }
            }
        });
        gpui_kit::base::Dialog::new(cx)
            .focus_handle(self.modal_focus.clone())
            .flex()
            .items_center()
            .justify_center()
            .backdrop(div().absolute().inset_0().bg(gpui_kit::rgba(0x10182060)))
            .popup(popup)
            .close_on_backdrop_press(false)
            .on_cancel(move |event, window, cx| {
                cancel(event, window, cx);
                false
            })
            .on_ok(move |event, window, cx| {
                submit(event, window, cx);
                false
            })
            .into_any_element()
    }
}
