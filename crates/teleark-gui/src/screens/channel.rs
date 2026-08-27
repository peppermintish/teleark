use gpui::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{Icon, IconName, input::Input, scroll::ScrollableElement as _};
use teleark_i18n::{MessageArgs, format::format_integer};
use teleark_runtime::TelegramAuthState;

use crate::{
    app::{TeleArkApp, TelegramActivity},
    components::{self, Tone},
    layout::LayoutPolicy,
    theme,
};

impl TeleArkApp {
    pub(crate) fn render_channel(
        &self,
        _window: &mut Window,
        layout: LayoutPolicy,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let padding = layout.content_padding();
        let body = match &self.telegram_auth {
            TelegramAuthState::Disconnected | TelegramAuthState::Unauthorized => {
                self.render_telegram_credentials(layout, cx)
            }
            TelegramAuthState::CodeSent => self.render_telegram_code(cx),
            TelegramAuthState::PasswordRequired { hint } => {
                self.render_telegram_password(hint.as_deref(), cx)
            }
            TelegramAuthState::Authorized(_) => self.render_telegram_channels(layout, cx),
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
                    .h(px(58.0))
                    .px(px(padding))
                    .flex()
                    .items_center()
                    .child(components::section_title(self.tr("telegram-library-title")))
                    .child(div().flex_1())
                    .child(self.telegram_status_badge()),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .px(px(padding))
                    .pb(px(padding))
                    .overflow_y_scrollbar()
                    .child(body),
            )
            .into_any_element()
    }

    fn telegram_status_badge(&self) -> AnyElement {
        let (label, tone) = match self.telegram_activity {
            TelegramActivity::Working => (self.tr("telegram-status-working"), Tone::Blue),
            TelegramActivity::Failed(_) => (self.tr("telegram-status-failed"), Tone::Red),
            TelegramActivity::Idle => match self.telegram_auth {
                TelegramAuthState::Authorized(_) => {
                    (self.tr("telegram-status-connected"), Tone::Green)
                }
                _ => (self.tr("telegram-status-not-connected"), Tone::Neutral),
            },
        };
        components::badge(label, tone).into_any_element()
    }

    fn render_telegram_credentials(
        &self,
        layout: LayoutPolicy,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        components::card()
            .max_w(px(760.0))
            .mx_auto()
            .p(px(layout.content_padding().max(20.0)))
            .child(components::section_title(self.tr("telegram-login-title")))
            .child(
                div()
                    .mt_2()
                    .text_sm()
                    .text_color(theme::text_secondary())
                    .child(self.tr("telegram-login-description")),
            )
            .child(
                div()
                    .mt_5()
                    .grid()
                    .grid_cols(if layout.is_compact() { 1 } else { 2 })
                    .gap_3()
                    .child(labeled_input(
                        self.tr("telegram-api-id-label"),
                        &self.telegram_api_id,
                    ))
                    .child(labeled_input(
                        self.tr("telegram-api-hash-label"),
                        &self.telegram_api_hash,
                    ))
                    .child(labeled_input(
                        self.tr("telegram-phone-label"),
                        &self.telegram_phone,
                    )),
            )
            .when_some(self.telegram_error_message(), |card, message| {
                card.child(error_banner(message))
            })
            .child(primary_action(
                "telegram-connect",
                self.tr("telegram-connect-action"),
                IconName::ArrowRight,
                cx.listener(|this, _, _, cx| this.begin_telegram_login(cx)),
            ))
            .into_any_element()
    }

    fn render_telegram_code(&self, cx: &mut Context<Self>) -> AnyElement {
        components::card()
            .max_w(px(560.0))
            .mx_auto()
            .p_6()
            .child(components::section_title(self.tr("telegram-code-title")))
            .child(
                div()
                    .mt_2()
                    .text_sm()
                    .text_color(theme::text_secondary())
                    .child(self.tr("telegram-code-description")),
            )
            .child(
                div()
                    .mt_5()
                    .child(Input::new(&self.telegram_code).h(px(38.0))),
            )
            .when_some(self.telegram_error_message(), |card, message| {
                card.child(error_banner(message))
            })
            .child(primary_action(
                "telegram-submit-code",
                self.tr("telegram-code-action"),
                IconName::Check,
                cx.listener(|this, _, _, cx| this.submit_telegram_code(cx)),
            ))
            .into_any_element()
    }

    fn render_telegram_password(&self, hint: Option<&str>, cx: &mut Context<Self>) -> AnyElement {
        let description = hint.map_or_else(
            || self.tr("telegram-password-description"),
            |hint| {
                self.tr_with(
                    "telegram-password-hint",
                    MessageArgs::new().with("hint", hint),
                )
            },
        );
        components::card()
            .max_w(px(560.0))
            .mx_auto()
            .p_6()
            .child(components::section_title(
                self.tr("telegram-password-title"),
            ))
            .child(
                div()
                    .mt_2()
                    .text_sm()
                    .text_color(theme::text_secondary())
                    .child(description),
            )
            .child(
                div().mt_5().child(
                    Input::new(&self.telegram_password)
                        .mask_toggle()
                        .h(px(38.0)),
                ),
            )
            .when_some(self.telegram_error_message(), |card, message| {
                card.child(error_banner(message))
            })
            .child(primary_action(
                "telegram-submit-password",
                self.tr("telegram-password-action"),
                IconName::Asterisk,
                cx.listener(|this, _, _, cx| this.submit_telegram_password(cx)),
            ))
            .into_any_element()
    }

    fn render_telegram_channels(&self, layout: LayoutPolicy, cx: &mut Context<Self>) -> AnyElement {
        let list = components::card()
            .when(layout.is_compact(), |list| list.w_full())
            .when(!layout.is_compact(), |list| list.w(px(330.0)).flex_none())
            .p_2()
            .child(
                div()
                    .px_3()
                    .py_2()
                    .text_sm()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(self.tr("telegram-channel-select-title")),
            )
            .children(self.telegram_chats.iter().map(|chat| {
                let id = chat.id;
                let selected = self.selected_chat_id == Some(id);
                let name = if chat.name.is_empty() {
                    chat.username.clone().unwrap_or_else(|| chat.id.to_string())
                } else {
                    chat.name.clone()
                };
                div()
                    .id(("telegram-chat", id.unsigned_abs()))
                    .px_3()
                    .py_2()
                    .flex()
                    .items_center()
                    .gap_3()
                    .rounded(theme::RADIUS_SMALL)
                    .cursor_pointer()
                    .when(selected, |row| row.bg(theme::blue_soft()))
                    .hover(|row| row.bg(theme::blue_pale()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_telegram_chat(id, cx);
                    }))
                    .child(Icon::new(IconName::Inbox).text_color(theme::purple()))
                    .child(div().min_w_0().flex_1().truncate().text_sm().child(name))
            }));

        let selected_name = self
            .selected_chat_id
            .and_then(|id| self.telegram_chats.iter().find(|chat| chat.id == id))
            .map(|chat| chat.name.clone())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| self.tr("telegram-no-channel-selected").to_string());
        let detail = components::card()
            .flex_1()
            .min_w_0()
            .p_5()
            .child(components::section_title(selected_name))
            .child(
                div()
                    .mt_2()
                    .text_sm()
                    .text_color(theme::text_secondary())
                    .child(self.tr("telegram-index-description")),
            )
            .when_some(self.telegram_index.as_ref(), |detail, progress| {
                detail.child(
                    div()
                        .mt_4()
                        .p_3()
                        .rounded(theme::RADIUS_SMALL)
                        .bg(theme::blue_pale())
                        .text_sm()
                        .child(self.tr_with(
                            if progress.exhausted {
                                "telegram-index-complete"
                            } else {
                                "telegram-index-page-complete"
                            },
                            MessageArgs::new().with(
                                "count",
                                format_integer(self.locale(), progress.files_indexed),
                            ),
                        )),
                )
            })
            .when_some(self.telegram_error_message(), |detail, message| {
                detail.child(error_banner(message))
            })
            .child(primary_action(
                "telegram-index-next",
                self.tr("telegram-index-next-action"),
                IconName::Search,
                cx.listener(|this, _, _, cx| this.index_selected_telegram_chat(cx)),
            ));

        div()
            .flex()
            .when(layout.is_compact(), |body| body.flex_col())
            .gap_4()
            .child(list)
            .child(detail)
            .into_any_element()
    }

    fn telegram_error_message(&self) -> Option<gpui::SharedString> {
        let TelegramActivity::Failed(kind) = self.telegram_activity else {
            return None;
        };
        Some(self.tr(match kind {
            teleark_core::ApplicationErrorKind::InvalidRequest => "telegram-error-invalid-request",
            teleark_core::ApplicationErrorKind::Authorization => "telegram-error-authorization",
            teleark_core::ApplicationErrorKind::Network => "telegram-error-network",
            teleark_core::ApplicationErrorKind::Persistence => "telegram-error-persistence",
            _ => "telegram-error-generic",
        }))
    }
}

fn labeled_input(
    label: gpui::SharedString,
    state: &gpui::Entity<gpui_component::input::InputState>,
) -> AnyElement {
    div()
        .child(
            div()
                .mb_2()
                .text_xs()
                .text_color(theme::text_secondary())
                .child(label),
        )
        .child(Input::new(state).h(px(38.0)))
        .into_any_element()
}

fn primary_action(
    id: &'static str,
    label: gpui::SharedString,
    icon: IconName,
    listener: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .mt_5()
        .h(px(36.0))
        .px_5()
        .flex()
        .items_center()
        .justify_center()
        .gap_2()
        .rounded(theme::RADIUS_SMALL)
        .bg(theme::blue())
        .text_color(theme::surface())
        .cursor_pointer()
        .on_click(listener)
        .child(Icon::new(icon))
        .child(label)
        .into_any_element()
}

fn error_banner(message: gpui::SharedString) -> AnyElement {
    div()
        .mt_4()
        .p_3()
        .rounded(theme::RADIUS_SMALL)
        .bg(theme::red_soft())
        .text_sm()
        .text_color(theme::red())
        .child(message)
        .into_any_element()
}
