use gpui::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    Disableable as _, Icon, IconName, input::Input, scroll::ScrollableElement as _,
    tooltip::Tooltip,
};
use qrcode::{QrCode, types::Color};
use teleark_core::FileKind;
use teleark_i18n::{
    MessageArgs,
    format::{format_bytes, format_integer, format_unix_millis},
};
use teleark_runtime::{ChannelDownloadState, TelegramAuthState};

use crate::{
    app::{
        ChannelBatchActivity, ChannelBatchPeriod, TeleArkApp, TelegramActivity,
        telegram_login_controls_enabled,
    },
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
        let authorized = matches!(self.telegram_auth, TelegramAuthState::Authorized(_));
        let body = match &self.telegram_auth {
            TelegramAuthState::Disconnected | TelegramAuthState::Unauthorized => {
                self.render_telegram_login_methods(layout, None, cx)
            }
            TelegramAuthState::QrCode {
                deep_link,
                expires_at_unix_seconds: _,
            } => self.render_telegram_login_methods(layout, Some(deep_link), cx),
            TelegramAuthState::CodeSent => self.render_telegram_code(cx),
            TelegramAuthState::PasswordRequired { hint } => {
                self.render_telegram_password(hint.as_deref(), cx)
            }
            TelegramAuthState::Authorized(_) => self.render_telegram_channels(layout, cx),
        };
        let content = div()
            .flex_1()
            .min_h_0()
            .px(px(padding))
            .pb(px(padding))
            .child(body);
        let content = if channel_uses_route_scroll(authorized, layout) {
            content.overflow_y_scrollbar().into_any_element()
        } else {
            content.into_any_element()
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
            .child(content)
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

    fn render_telegram_login_methods(
        &self,
        layout: LayoutPolicy,
        qr_deep_link: Option<&str>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let credentials_enabled = telegram_login_controls_enabled(self.configured_telegram_api_id);
        let phone_panel = div()
            .min_w_0()
            .pr(px(if layout.is_compact() { 16.0 } else { 24.0 }))
            .child(components::section_title(
                self.tr("telegram-phone-login-title"),
            ))
            .child(
                div()
                    .mt_2()
                    .text_sm()
                    .text_color(theme::text_secondary())
                    .child(self.tr("telegram-phone-login-description")),
            )
            .child(div().mt_4().child(labeled_input(
                self.tr("telegram-phone-label"),
                &self.telegram_phone,
                !credentials_enabled,
            )))
            .child(primary_action(
                "telegram-connect-phone",
                self.tr("telegram-connect-action"),
                IconName::ArrowRight,
                !credentials_enabled,
                cx.listener(|this, _, _, cx| this.begin_telegram_login(cx)),
            ));

        let qr = if !credentials_enabled {
            credential_qr_placeholder(
                if layout.is_compact() { 140.0 } else { 196.0 },
                self.tr("telegram-qr-credentials-placeholder"),
            )
        } else {
            qr_deep_link.map_or_else(
                || {
                    div()
                        .size(px(if layout.is_compact() { 140.0 } else { 196.0 }))
                        .p_5()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap_3()
                        .rounded(theme::RADIUS_MEDIUM)
                        .border_1()
                        .border_color(theme::border())
                        .bg(theme::border_subtle())
                        .text_center()
                        .text_sm()
                        .text_color(theme::text_secondary())
                        .child(Icon::new(IconName::Frame).size(px(40.0)))
                        .child(self.tr("telegram-qr-placeholder"))
                        .into_any_element()
                },
                |deep_link| {
                    qr_code_element(deep_link, if layout.is_compact() { 132.0 } else { 180.0 })
                },
            )
        };
        let qr_action = if qr_deep_link.is_some() {
            primary_action(
                "telegram-connect-qr",
                self.tr("telegram-qr-refresh-action"),
                IconName::Redo2,
                !credentials_enabled,
                cx.listener(|this, _, _, cx| this.refresh_telegram_qr_login(cx)),
            )
        } else {
            primary_action(
                "telegram-connect-qr",
                self.tr("telegram-connect-qr-action"),
                IconName::Frame,
                !credentials_enabled,
                cx.listener(|this, _, _, cx| this.begin_telegram_qr_login(cx)),
            )
        };
        let qr_panel = div()
            .min_w_0()
            .pl(px(if layout.is_compact() { 16.0 } else { 24.0 }))
            .border_l_1()
            .border_color(theme::border())
            .child(components::section_title(self.tr("telegram-qr-title")))
            .child(
                div()
                    .mt_2()
                    .text_sm()
                    .text_color(theme::text_secondary())
                    .child(self.tr("telegram-qr-description")),
            )
            .child(div().mt_4().flex().justify_center().child(qr))
            .child(
                div()
                    .mt_4()
                    .text_xs()
                    .text_color(theme::text_secondary())
                    .child(self.tr("telegram-qr-refresh-note")),
            )
            .child(qr_action);

        components::card()
            .max_w(px(1_080.0))
            .mx_auto()
            .p(px(layout.content_padding().max(20.0)))
            .child(
                div()
                    .text_xl()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(self.tr("telegram-login-title")),
            )
            .child(
                div()
                    .mt_2()
                    .text_sm()
                    .text_color(theme::text_secondary())
                    .child(self.tr("telegram-login-description")),
            )
            .when_some(self.telegram_error_message(), |card, message| {
                card.child(error_banner(message))
            })
            .when(!credentials_enabled, |card| {
                card.child(
                    div()
                        .mt_4()
                        .p_4()
                        .flex()
                        .items_center()
                        .gap_3()
                        .rounded(theme::RADIUS_MEDIUM)
                        .border_1()
                        .border_color(theme::amber())
                        .bg(theme::amber_soft())
                        .child(Icon::new(IconName::TriangleAlert).text_color(theme::amber()))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .child(self.tr("telegram-api-id-required-title")),
                                )
                                .child(
                                    div()
                                        .mt_1()
                                        .text_xs()
                                        .text_color(theme::text_secondary())
                                        .child(self.tr("telegram-api-id-required-description")),
                                ),
                        )
                        .child(
                            components::button(
                                "telegram-open-api-settings",
                                self.tr("telegram-api-id-open-settings-action"),
                                Some(IconName::Settings),
                                true,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.open_telegram_api_id_settings(cx);
                            })),
                        ),
                )
            })
            .child(
                div()
                    .mt_4()
                    .grid()
                    .grid_cols(login_method_columns(layout))
                    .when(!credentials_enabled, |methods| methods.opacity(0.46))
                    .child(phone_panel)
                    .child(qr_panel),
            )
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
                false,
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
                false,
                cx.listener(|this, _, _, cx| this.submit_telegram_password(cx)),
            ))
            .into_any_element()
    }

    fn render_telegram_channels(&self, layout: LayoutPolicy, cx: &mut Context<Self>) -> AnyElement {
        let list = components::card()
            .when(layout.is_compact(), |list| list.w_full())
            .when(!layout.is_compact(), |list| list.w(px(330.0)).flex_none())
            .p_2()
            .when(layout.is_compact(), |list| list.max_h(px(280.0)))
            .when(!layout.is_compact(), |list| list.h_full().min_h_0())
            .overflow_y_scrollbar()
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
            .when(!layout.is_compact(), |detail| {
                detail
                    .h_full()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
            })
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
            .child(
                div()
                    .mt_4()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        components::button(
                            "telegram-files-refresh",
                            self.tr("telegram-files-refresh-action"),
                            Some(IconName::Redo2),
                            true,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.load_selected_telegram_files(false, cx);
                        })),
                    )
                    .child(
                        components::button(
                            "telegram-index-next",
                            self.tr("telegram-index-next-action"),
                            Some(IconName::Search),
                            false,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.index_selected_telegram_chat(cx);
                        })),
                    ),
            )
            .child(self.render_channel_batch_controls(cx))
            .child(self.render_telegram_file_list(layout, cx));

        div()
            .flex()
            .when(!layout.is_compact(), |body| body.h_full().min_h_0())
            .when(layout.is_compact(), |body| body.flex_col())
            .gap_4()
            .child(list)
            .child(detail)
            .into_any_element()
    }

    fn render_channel_batch_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let preparing = self.channel_batch_activity == ChannelBatchActivity::Preparing;
        let periods = [
            (ChannelBatchPeriod::AnyTime, "telegram-batch-period-any"),
            (ChannelBatchPeriod::Past24Hours, "telegram-batch-period-24h"),
            (ChannelBatchPeriod::Past7Days, "telegram-batch-period-7d"),
            (ChannelBatchPeriod::Past30Days, "telegram-batch-period-30d"),
        ];
        let kinds = [
            (None, "telegram-batch-kind-all"),
            (Some(FileKind::Video), "telegram-batch-kind-video"),
            (Some(FileKind::Document), "telegram-batch-kind-document"),
            (Some(FileKind::Archive), "telegram-batch-kind-archive"),
            (Some(FileKind::Audio), "telegram-batch-kind-audio"),
            (Some(FileKind::Image), "telegram-batch-kind-image"),
            (Some(FileKind::Other), "telegram-batch-kind-other"),
        ];
        let status = match self.channel_batch_activity {
            ChannelBatchActivity::Idle => None,
            ChannelBatchActivity::Preparing => {
                Some((self.tr("telegram-batch-preparing"), Tone::Blue))
            }
            ChannelBatchActivity::Queued { count, .. } => Some((
                self.tr_with(
                    "telegram-batch-queued",
                    MessageArgs::new().with("count", format_integer(self.locale(), count as u64)),
                ),
                Tone::Green,
            )),
            ChannelBatchActivity::NoMatches => {
                Some((self.tr("telegram-batch-no-matches"), Tone::Amber))
            }
            ChannelBatchActivity::Failed(_) => Some((self.tr("telegram-batch-failed"), Tone::Red)),
        };
        components::card()
            .mt_3()
            .p_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(self.tr("telegram-batch-title")),
                            )
                            .child(
                                div()
                                    .mt_1()
                                    .truncate()
                                    .text_xs()
                                    .text_color(theme::text_secondary())
                                    .child(self.tr("telegram-batch-description")),
                            ),
                    )
                    .when_some(status, |row, (label, tone)| {
                        row.child(components::badge(label, tone))
                    })
                    .child(
                        components::icon_button(
                            "telegram-batch-toggle",
                            if self.channel_batch_expanded {
                                IconName::ChevronUp
                            } else {
                                IconName::ChevronDown
                            },
                            self.tr("telegram-batch-title"),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.channel_batch_expanded = !this.channel_batch_expanded;
                            cx.notify();
                        })),
                    ),
            )
            .when(self.channel_batch_expanded, |card| {
                card.child(
                    div()
                        .mt_3()
                        .text_xs()
                        .text_color(theme::text_muted())
                        .child(self.tr("telegram-batch-period-label")),
                )
                .child(
                    div()
                        .mt_2()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .children(periods.into_iter().map(|(period, label)| {
                            components::button(
                                ("telegram-batch-period", period as usize),
                                self.tr(label),
                                None,
                                self.channel_batch_period == period,
                            )
                            .disabled(preparing)
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.channel_batch_period = period;
                                    this.channel_batch_activity = ChannelBatchActivity::Idle;
                                    cx.notify();
                                },
                            ))
                        })),
                )
                .child(
                    div()
                        .mt_3()
                        .text_xs()
                        .text_color(theme::text_muted())
                        .child(self.tr("telegram-batch-kind-label")),
                )
                .child(div().mt_2().flex().flex_wrap().gap_2().children(
                    kinds.into_iter().enumerate().map(|(index, (kind, label))| {
                        components::button(
                            ("telegram-batch-kind", index),
                            self.tr(label),
                            None,
                            self.channel_batch_kind == kind,
                        )
                        .disabled(preparing)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.channel_batch_kind = kind;
                            this.channel_batch_activity = ChannelBatchActivity::Idle;
                            cx.notify();
                        }))
                    }),
                ))
                .child(
                    div().mt_3().child(
                        components::button(
                            "telegram-batch-download",
                            self.tr("telegram-batch-download-action"),
                            Some(IconName::ArrowDown),
                            true,
                        )
                        .disabled(preparing || self.selected_chat_id.is_none())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.download_filtered_telegram_files(cx);
                        })),
                    ),
                )
            })
            .into_any_element()
    }

    fn render_telegram_file_list(
        &self,
        layout: LayoutPolicy,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let heading = self.tr_with(
            "telegram-files-title",
            MessageArgs::new().with(
                "count",
                format_integer(self.locale(), self.telegram_files.len() as u64),
            ),
        );
        let mut list = div()
            .mt_5()
            .when(!layout.is_compact(), |list| {
                list.flex_1().min_h_0().flex().flex_col().overflow_hidden()
            })
            .border_t_1()
            .border_color(theme::border())
            .child(
                div()
                    .h(px(42.0))
                    .flex()
                    .items_center()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_sm()
                    .child(heading),
            );

        if self.telegram_files.is_empty() {
            list = list.child(
                div()
                    .py_6()
                    .text_sm()
                    .text_color(theme::text_secondary())
                    .child(if self.telegram_files_loading {
                        self.tr("telegram-files-loading")
                    } else {
                        self.tr("telegram-files-empty")
                    }),
            );
        } else {
            list = list.child(
                div()
                    .when(layout.is_compact(), |body| {
                        body.h(px(layout.channel_compact_file_list_height()))
                    })
                    .when(!layout.is_compact(), |body| body.flex_1().min_h_0())
                    .overflow_y_scrollbar()
                    .children(self.telegram_files.iter().map(|file| {
                        let message_id = file.message_id;
                        let selected = self.selected_telegram_message_id == Some(message_id);
                        let name = if file.file_name.trim().is_empty() {
                            self.tr_with(
                                "telegram-file-unnamed",
                                MessageArgs::new().with("message_id", message_id.to_string()),
                            )
                        } else {
                            file.file_name.clone().into()
                        };
                        let caption = if file.caption.trim().is_empty() {
                            self.tr("telegram-message-no-caption").to_string()
                        } else {
                            file.caption.clone()
                        };
                        let caption_preview = caption_preview(&caption, 96);
                        let caption_tooltip = caption.clone();
                        let sent_at = format_unix_millis(self.locale(), file.sent_at_unix_ms);
                        div()
                            .id(("telegram-file", message_id.unsigned_abs()))
                            .min_h(px(72.0))
                            .py_2()
                            .flex()
                            .items_start()
                            .gap_3()
                            .border_b_1()
                            .border_color(theme::border_subtle())
                            .cursor_pointer()
                            .when(selected, |row| row.bg(theme::blue_pale()))
                            .hover(|row| row.bg(theme::blue_pale()))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.selected_telegram_message_id = Some(message_id);
                                cx.notify();
                            }))
                            .child(Icon::new(IconName::File).text_color(theme::blue()))
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .child(div().truncate().text_sm().child(name))
                                    .child(
                                        div()
                                            .mt_1()
                                            .text_xs()
                                            .text_color(theme::text_secondary())
                                            .child(
                                                self.tr_with(
                                                    "telegram-file-metadata",
                                                    MessageArgs::new()
                                                        .with(
                                                            "size",
                                                            format_bytes(
                                                                self.locale(),
                                                                file.size_bytes,
                                                            ),
                                                        )
                                                        .with("message_id", message_id.to_string()),
                                                ),
                                            )
                                            .child(" · ")
                                            .child(sent_at),
                                    )
                                    .child(
                                        div()
                                            .id(("telegram-caption", message_id.unsigned_abs()))
                                            .mt_1()
                                            .truncate()
                                            .text_xs()
                                            .text_color(theme::text_muted())
                                            .child(caption_preview)
                                            .tooltip(move |window, cx| {
                                                Tooltip::new(caption_tooltip.clone())
                                                    .build(window, cx)
                                            }),
                                    ),
                            )
                            .child(
                                components::button(
                                    ("telegram-download", message_id.unsigned_abs()),
                                    self.tr("telegram-file-download-action"),
                                    Some(IconName::ArrowDown),
                                    false,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.download_telegram_file(message_id, cx);
                                    },
                                )),
                            )
                    })),
            );
        }

        list = list.when_some(self.telegram_download, |list, (_, state)| {
            let (message, tone) = match state {
                ChannelDownloadState::Queued => ("telegram-download-queued", Tone::Amber),
                ChannelDownloadState::Running => ("telegram-download-running", Tone::Blue),
                ChannelDownloadState::Paused => ("telegram-download-paused", Tone::Amber),
                ChannelDownloadState::Completed => ("telegram-download-completed", Tone::Green),
                ChannelDownloadState::Failed(_) => ("telegram-download-failed", Tone::Red),
                ChannelDownloadState::Cancelled => ("telegram-download-cancelled", Tone::Red),
            };
            list.child(
                div()
                    .mt_3()
                    .child(components::badge(self.tr(message), tone)),
            )
        });

        if !self.telegram_files_exhausted && !self.telegram_files.is_empty() {
            list = list.child(
                div().mt_3().child(
                    components::button(
                        "telegram-files-more",
                        self.tr("telegram-files-more-action"),
                        Some(IconName::ChevronDown),
                        false,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.load_selected_telegram_files(true, cx);
                    })),
                ),
            );
        }
        let selected_message = self.selected_telegram_message_id.and_then(|message_id| {
            self.telegram_files
                .iter()
                .find(|file| file.message_id == message_id)
        });
        div()
            .when(!layout.is_compact(), |browser| {
                browser.flex_1().min_h_0().flex().gap_4()
            })
            .when(layout.is_compact(), |browser| browser.flex().flex_col())
            .child(
                div()
                    .min_w_0()
                    .when(!layout.is_compact(), |panel| {
                        panel.flex_1().min_h_0().flex().flex_col().overflow_hidden()
                    })
                    .child(list),
            )
            .when_some(selected_message, |browser, file| {
                browser.child(self.render_telegram_message_detail(file, layout))
            })
            .into_any_element()
    }

    fn render_telegram_message_detail(
        &self,
        file: &teleark_runtime::TelegramFileSummary,
        layout: LayoutPolicy,
    ) -> AnyElement {
        let caption = if file.caption.trim().is_empty() {
            self.tr("telegram-message-no-caption")
        } else {
            file.caption.clone().into()
        };
        components::card()
            .when(!layout.is_compact(), |detail| {
                detail.w(px(300.0)).flex_none().mt_5().mb_3()
            })
            .when(layout.is_compact(), |detail| detail.mt_3())
            .p_4()
            .overflow_y_scrollbar()
            .child(components::section_title(
                self.tr("telegram-message-detail-title"),
            ))
            .child(message_detail_row(
                self.tr("telegram-message-file-name"),
                if file.file_name.is_empty() {
                    self.tr_with(
                        "telegram-file-unnamed",
                        MessageArgs::new().with("message_id", file.message_id.to_string()),
                    )
                } else {
                    file.file_name.clone().into()
                },
            ))
            .child(message_detail_row(
                self.tr("detail-message-id"),
                file.message_id.to_string().into(),
            ))
            .child(message_detail_row(
                self.tr("telegram-message-sent-at"),
                format_unix_millis(self.locale(), file.sent_at_unix_ms).into(),
            ))
            .child(message_detail_row(
                self.tr("telegram-message-mime-type"),
                file.mime_type
                    .clone()
                    .unwrap_or_else(|| self.tr("transfer-value-unavailable").to_string())
                    .into(),
            ))
            .child(message_detail_row(
                self.tr("table-size"),
                format_bytes(self.locale(), file.size_bytes).into(),
            ))
            .child(
                div()
                    .mt_4()
                    .text_xs()
                    .text_color(theme::text_muted())
                    .child(self.tr("telegram-message-caption")),
            )
            .child(div().mt_2().text_sm().whitespace_normal().child(caption))
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

fn channel_uses_route_scroll(authorized: bool, layout: LayoutPolicy) -> bool {
    !authorized || layout.is_compact()
}

fn caption_preview(caption: &str, maximum_chars: usize) -> String {
    let mut characters = caption.chars();
    let preview: String = characters.by_ref().take(maximum_chars).collect();
    if characters.next().is_some() {
        format!("{preview}…")
    } else {
        preview
    }
}

fn message_detail_row(label: gpui::SharedString, value: gpui::SharedString) -> gpui::Div {
    div()
        .mt_3()
        .child(div().text_xs().text_color(theme::text_muted()).child(label))
        .child(div().mt_1().text_sm().whitespace_normal().child(value))
}

fn credential_qr_placeholder(size: f32, label: gpui::SharedString) -> AnyElement {
    let finder = || {
        div()
            .size(px(25.0))
            .p(px(4.0))
            .rounded(px(5.0))
            .border_1()
            .border_color(theme::blue())
            .child(div().size_full().rounded(px(2.0)).bg(theme::blue()))
    };
    div()
        .size(px(size))
        .p_4()
        .flex()
        .flex_col()
        .rounded(theme::RADIUS_MEDIUM)
        .border_1()
        .border_color(theme::blue_soft())
        .bg(theme::blue_pale())
        .child(
            div()
                .w_full()
                .flex()
                .justify_between()
                .child(finder())
                .child(finder()),
        )
        .child(
            div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_1()
                .child(
                    div()
                        .size(px(38.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .bg(theme::surface())
                        .text_color(theme::blue())
                        .child(Icon::new(IconName::Frame).size(px(22.0))),
                )
                .child(
                    div()
                        .text_xs()
                        .text_center()
                        .text_color(theme::text_secondary())
                        .child(label),
                ),
        )
        .child(div().w_full().flex().child(finder()).child(div().flex_1()))
        .into_any_element()
}

fn qr_code_element(deep_link: &str, target_size: f32) -> AnyElement {
    let Ok(code) = QrCode::new(deep_link.as_bytes()) else {
        return div()
            .size(px(180.0))
            .bg(theme::red_soft())
            .into_any_element();
    };
    let width = code.width();
    let colors = code.to_colors();
    let cell = (target_size / width as f32).floor().max(2.0);
    div()
        .p(px(cell * 4.0))
        .bg(theme::surface())
        .border_1()
        .border_color(theme::border())
        .children((0..width).map(|row| {
            div().flex().children((0..width).map(|column| {
                div()
                    .size(px(cell))
                    .bg(if colors[row * width + column] == Color::Dark {
                        theme::text_primary()
                    } else {
                        theme::surface()
                    })
            }))
        }))
        .into_any_element()
}

fn login_method_columns(_layout: LayoutPolicy) -> u16 {
    // The supported minimum is 900 px. Both methods fit at that width and
    // staying side by side makes the choice immediately discoverable.
    2
}

fn labeled_input(
    label: gpui::SharedString,
    state: &gpui::Entity<gpui_component::input::InputState>,
    disabled: bool,
) -> AnyElement {
    div()
        .child(
            div()
                .mb_2()
                .text_xs()
                .text_color(theme::text_secondary())
                .child(label),
        )
        .child(Input::new(state).disabled(disabled).h(px(38.0)))
        .into_any_element()
}

fn primary_action(
    id: &'static str,
    label: gpui::SharedString,
    icon: IconName,
    disabled: bool,
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
        .bg(if disabled {
            theme::border()
        } else {
            theme::blue()
        })
        .text_color(if disabled {
            theme::text_muted()
        } else {
            theme::surface()
        })
        .when(!disabled, |action| {
            action.cursor_pointer().on_click(listener)
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_methods_remain_visible_at_every_supported_window_class() {
        assert_eq!(
            login_method_columns(LayoutPolicy::from_size(900.0, 600.0)),
            2
        );
        assert_eq!(
            login_method_columns(LayoutPolicy::from_size(1_360.0, 760.0)),
            2
        );
        assert_eq!(
            login_method_columns(LayoutPolicy::from_size(1_920.0, 1_080.0)),
            2
        );
    }

    #[test]
    fn authorized_desktop_sources_use_independent_channel_and_file_scroll_panes() {
        assert!(channel_uses_route_scroll(
            true,
            LayoutPolicy::from_size(960.0, 640.0)
        ));
        assert!(!channel_uses_route_scroll(
            true,
            LayoutPolicy::from_size(1_360.0, 760.0)
        ));
        assert!(channel_uses_route_scroll(
            false,
            LayoutPolicy::from_size(1_920.0, 1_080.0)
        ));
    }

    #[test]
    fn caption_preview_is_unicode_safe_and_only_truncates_long_content() {
        assert_eq!(caption_preview("完整 caption", 20), "完整 caption");
        assert_eq!(caption_preview("一二三四五六", 4), "一二三四…");
        assert_eq!(caption_preview("hidden", 0), "…");
    }
}
