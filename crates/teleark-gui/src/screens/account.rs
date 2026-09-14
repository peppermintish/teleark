//! Account-first launch and authentication. Session content stays in the runtime.
use crate::assets::Symbol;
use crate::{
    app::{TeleArkApp, TelegramActivity},
    components,
    layout::LayoutPolicy,
    theme,
};
use gpui_kit::component::{
    Disableable as _, Icon, IconName, button::ButtonVariants as _, scroll::ScrollableElement as _,
};
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _, Styled as _,
    Window, div, prelude::FluentBuilder as _, px,
};
use teleark_runtime::TelegramAuthState;

impl TeleArkApp {
    pub(crate) fn render_account_switch_dialog(&self, cx: &mut Context<Self>) -> AnyElement {
        let popup = components::card()
            .id("account-switch-dialog")
            .debug_selector(|| "account-switch-dialog".into())
            .w(px(420.0))
            .p_6()
            .shadow_lg()
            .child(
                div()
                    .text_size(px(20.0))
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .child(self.tr("account-switch-confirm-title")),
            )
            .child(
                div()
                    .mt_3()
                    .text_sm()
                    .text_color(theme::text_secondary())
                    .child(self.tr("account-switch-confirm-description")),
            )
            .when(self.show_account_switch, |popup| {
                popup.child(
                    div()
                        .mt_3()
                        .text_sm()
                        .text_color(theme::amber())
                        .child(self.tr("account-switch-busy")),
                )
            })
            .child(
                div()
                    .mt_5()
                    .flex()
                    .justify_end()
                    .gap_3()
                    .child(
                        components::button(
                            "account-switch-cancel",
                            self.tr("common-cancel"),
                            None,
                            false,
                        )
                        .debug_selector(|| "account-switch-cancel".into())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.confirm_account_switch = false;
                            this.show_account_switch = false;
                            cx.notify();
                        })),
                    )
                    .child(
                        components::button(
                            "account-switch-confirm",
                            self.tr("account-switch-confirm-action"),
                            None,
                            true,
                        )
                        .debug_selector(|| "account-switch-confirm".into())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.switch_telegram_account(window, cx)
                        })),
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
            .on_cancel(|_, _, _| false)
            .on_ok(|_, _, _| false)
            .into_any_element()
    }

    pub(crate) fn render_account(
        &self,
        _window: &mut Window,
        layout: LayoutPolicy,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let returning = self.telegram_account.is_some()
            && matches!(self.telegram_auth, TelegramAuthState::Authorized(_));
        let body = if returning {
            div()
                .w(px(400.0))
                .flex()
                .flex_col()
                .items_center()
                .gap_4()
                .child(self.account_avatar_element(104.0))
                .child(
                    div()
                        .mt_2()
                        .text_size(px(28.0))
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                        .child(
                            self.telegram_account
                                .as_ref()
                                .map(|a| a.display_name.clone())
                                .unwrap_or_default(),
                        ),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(theme::text_secondary())
                        .child(self.tr("account-welcome-back")),
                )
                .child(
                    components::button(
                        "account-enter",
                        self.tr("account-login"),
                        Some(IconName::ArrowRight),
                        true,
                    )
                    .mt_3()
                    .w(px(200.0))
                    .h(px(38.0))
                    .disabled(self.telegram_activity == TelegramActivity::Working)
                    .on_click(cx.listener(|this, _, _, cx| this.enter_workspace(cx))),
                )
                .child(
                    components::button("account-switch", self.tr("account-switch"), None, false)
                        .ghost()
                        .disabled(self.telegram_activity == TelegramActivity::Working)
                        .debug_selector(|| "account-switch".into())
                        .on_click(cx.listener(|this, _, _, cx| this.request_account_switch(cx))),
                )
                .child(
                    div()
                        .text_xs()
                        .text_center()
                        .text_color(theme::text_muted())
                        .child(self.tr(if self.show_account_switch {
                            "account-switch-busy"
                        } else if self.telegram_activity == TelegramActivity::Working {
                            "account-switching"
                        } else {
                            "account-switch-description"
                        })),
                )
                .when_some(self.telegram_error_message(), |body, error| {
                    body.child(div().text_sm().text_color(theme::red()).child(error))
                })
                .into_any_element()
        } else if self.account_restoring {
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap_4()
                .child(components::app_mark(64.0))
                .child(
                    div()
                        .text_sm()
                        .text_color(theme::text_secondary())
                        .child(self.tr("account-restoring")),
                )
                .into_any_element()
        } else {
            let auth = match &self.telegram_auth {
                TelegramAuthState::QrCode { deep_link, .. } => {
                    self.render_telegram_login_methods(layout, Some(deep_link), cx)
                }
                TelegramAuthState::CodeSent => self.render_telegram_code(cx),
                TelegramAuthState::PasswordRequired { hint } => {
                    self.render_telegram_password(hint.as_deref(), cx)
                }
                _ => self.render_telegram_login_methods(layout, None, cx),
            };
            div()
                .w_full()
                .max_w(px(760.0))
                .flex()
                .flex_col()
                .items_center()
                .gap_3()
                .child(
                    div()
                        .text_size(px(26.0))
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                        .child(self.tr("account-welcome")),
                )
                .child(div().w_full().child(auth))
                .when(
                    matches!(
                        self.telegram_auth,
                        TelegramAuthState::CodeSent | TelegramAuthState::PasswordRequired { .. }
                    ),
                    |body| {
                        body.child(
                            components::button(
                                "account-change-method",
                                self.tr("account-change-method"),
                                Some(IconName::ArrowLeft),
                                false,
                            )
                            .ghost()
                            .disabled(self.telegram_activity == TelegramActivity::Working)
                            .on_click(cx.listener(
                                |this, _, window, cx| this.reset_telegram_login(window, cx),
                            )),
                        )
                    },
                )
                .into_any_element()
        };
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .bg(theme::canvas())
            .child(
                div()
                    .h(px(52.0))
                    .px_5()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(components::app_mark(22.0))
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                                    .child("TeleArk"),
                            ),
                    )
                    .when(!self.app_is_locked(), |header| {
                        header.child(
                            components::button(
                                "account-preferences",
                                self.tr("settings-title"),
                                Some(IconName::Settings),
                                false,
                            )
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_page(crate::app::Page::Settings, cx)
                            })),
                        )
                    }),
            )
            .child(
                div().flex_1().min_h_0().overflow_y_scrollbar().child(
                    div()
                        .min_h_full()
                        .py_6()
                        .px_6()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(body),
                ),
            )
            .child(
                div()
                    .h(px(40.0))
                    .flex_none()
                    .flex()
                    .justify_center()
                    .items_center()
                    .gap_2()
                    .text_xs()
                    .text_color(theme::text_muted())
                    .child(Icon::new(Symbol::Lock).size(px(12.0)))
                    .child(self.tr(if self.visual_preview {
                        "shell-preview"
                    } else {
                        "account-private-note"
                    })),
            )
            .into_any_element()
    }
}
