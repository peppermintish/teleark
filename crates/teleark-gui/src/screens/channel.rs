use std::{collections::HashSet, path::Path};

use gpui::{
    AnyElement, App, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, WeakEntity, Window, div,
    prelude::FluentBuilder as _, px,
};
use gpui_component::{
    Disableable as _, Icon, IconName, Sizable as _,
    checkbox::Checkbox,
    input::Input,
    scroll::ScrollableElement as _,
    table::{Column, Table, TableDelegate, TableState},
    tooltip::Tooltip,
};
use qrcode::{QrCode, types::Color};
use teleark_core::FileKind;
use teleark_i18n::{
    MessageArgs,
    format::{format_bytes, format_integer, format_unix_millis},
};
use teleark_runtime::{ChannelDownloadState, TelegramAuthState, TelegramFileSummary};

use crate::{
    app::{
        ChannelBatchActivity, ChannelBatchPeriod, TeleArkApp, TelegramActivity,
        telegram_login_controls_enabled,
    },
    components::{self, Tone},
    layout::LayoutPolicy,
    theme,
};

pub(crate) const CHANNEL_FILE_PAGE_SIZE: usize = 5_000;

#[derive(Clone, Debug, Eq, PartialEq)]
struct ChannelFileTableRow {
    message_id: i64,
    name: SharedString,
    sent_at: SharedString,
    kind: SharedString,
    size: SharedString,
}

pub(crate) struct ChannelFileTableDelegate {
    columns: Vec<Column>,
    rows: Vec<ChannelFileTableRow>,
    owner: Option<WeakEntity<TeleArkApp>>,
    loading: bool,
    exhausted: bool,
    select_all_label: SharedString,
    select_file_label: SharedString,
    download_label: SharedString,
    empty_label: SharedString,
}

impl ChannelFileTableDelegate {
    pub(crate) fn new() -> Self {
        Self {
            columns: channel_table_columns(["", "", "", "", "", ""].map(Into::into)),
            rows: Vec::new(),
            owner: None,
            loading: false,
            exhausted: true,
            select_all_label: "".into(),
            select_file_label: "".into(),
            download_label: "".into(),
            empty_label: "".into(),
        }
    }

    pub(crate) fn message_id_at(&self, row: usize) -> Option<i64> {
        self.rows.get(row).map(|row| row.message_id)
    }
}

impl TableDelegate for ChannelFileTableDelegate {
    fn columns_count(&self, _cx: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _cx: &App) -> usize {
        self.rows.len()
    }

    fn column(&self, col_ix: usize, _cx: &App) -> &Column {
        &self.columns[col_ix]
    }

    fn render_th(
        &mut self,
        col_ix: usize,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        if col_ix != 0 {
            return div()
                .size_full()
                .flex()
                .items_center()
                .child(self.columns[col_ix].name.clone())
                .into_any_element();
        }
        let all_selected = self.owner.as_ref().is_some_and(|owner| {
            owner
                .read_with(cx, |app, _| {
                    !self.rows.is_empty()
                        && self
                            .rows
                            .iter()
                            .all(|row| app.selected_channel_message_ids.contains(&row.message_id))
                })
                .unwrap_or(false)
        });
        let owner = self.owner.clone();
        let tooltip = self.select_all_label.clone();
        div()
            .id("channel-files-select-all-cell")
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(
                Checkbox::new("channel-files-select-all")
                    .checked(all_selected)
                    .on_click(move |checked, _, cx| {
                        if let Some(owner) = owner.as_ref() {
                            let _ = owner.update(cx, |app, cx| {
                                app.set_all_channel_files_selected(*checked, cx);
                            });
                        }
                    }),
            )
            .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
            .into_any_element()
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let Some(row) = self.rows.get(row_ix).cloned() else {
            return div().into_any_element();
        };
        match col_ix {
            0 => {
                let checked = self.owner.as_ref().is_some_and(|owner| {
                    owner
                        .read_with(cx, |app, _| {
                            app.selected_channel_message_ids.contains(&row.message_id)
                        })
                        .unwrap_or(false)
                });
                let owner = self.owner.clone();
                let tooltip = self.select_file_label.clone();
                div()
                    .id(("channel-file-select-cell", row.message_id.unsigned_abs()))
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        Checkbox::new(("channel-file-select", row.message_id.unsigned_abs()))
                            .checked(checked)
                            .on_click(move |checked, _, cx| {
                                if let Some(owner) = owner.as_ref() {
                                    let _ = owner.update(cx, |app, cx| {
                                        app.set_channel_file_selected(row.message_id, *checked, cx);
                                    });
                                }
                            }),
                    )
                    .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
                    .into_any_element()
            }
            1 => div()
                .size_full()
                .min_w_0()
                .flex()
                .items_center()
                .gap_2()
                .child(Icon::new(IconName::File).text_color(theme::blue()))
                .child(div().min_w_0().flex_1().truncate().child(row.name))
                .into_any_element(),
            2 => table_cell(row.sent_at),
            3 => table_cell(row.kind),
            4 => table_cell(row.size),
            _ => {
                let owner = self.owner.clone();
                let tooltip = self.download_label.clone();
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_end()
                    .child(
                        components::icon_button(
                            ("channel-file-download", row.message_id.unsigned_abs()),
                            IconName::ArrowDown,
                            tooltip,
                        )
                        .on_click(move |_, _, cx| {
                            if let Some(owner) = owner.as_ref() {
                                let _ = owner.update(cx, |app, cx| {
                                    app.download_telegram_file(row.message_id, cx);
                                });
                            }
                        }),
                    )
                    .into_any_element()
            }
        }
    }

    fn render_empty(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_sm()
            .text_color(theme::text_secondary())
            .child(self.empty_label.clone())
    }

    fn loading(&self, _cx: &App) -> bool {
        self.loading && self.rows.is_empty()
    }

    fn is_eof(&self, _cx: &App) -> bool {
        self.exhausted
    }

    fn load_more_threshold(&self) -> usize {
        100
    }

    fn load_more(&mut self, _window: &mut Window, cx: &mut Context<TableState<Self>>) {
        if self.loading || self.exhausted {
            return;
        }
        self.loading = true;
        if let Some(owner) = self.owner.as_ref() {
            let _ = owner.update(cx, |app, cx| {
                app.load_selected_telegram_files(true, cx);
            });
        }
    }
}

impl TeleArkApp {
    pub(crate) fn refresh_channel_file_table(&mut self, cx: &mut Context<Self>) {
        let rows = self
            .channel_filtered_files()
            .into_iter()
            .map(|file| {
                let name = if file.file_name.trim().is_empty() {
                    self.tr_with(
                        "telegram-file-unnamed",
                        MessageArgs::new().with("message_id", file.message_id.to_string()),
                    )
                } else {
                    file.file_name.clone().into()
                };
                let kind = teleark_runtime::classify_file(Path::new(&file.file_name));
                ChannelFileTableRow {
                    message_id: file.message_id,
                    name,
                    sent_at: format_unix_millis(self.locale(), file.sent_at_unix_ms).into(),
                    kind: self.tr(crate::screens::library::file_kind_message_id(kind)),
                    size: format_bytes(self.locale(), file.size_bytes).into(),
                }
            })
            .collect();
        let columns = channel_table_columns([
            "".into(),
            self.tr("table-name"),
            self.tr("telegram-message-sent-at"),
            self.tr("table-type"),
            self.tr("table-size"),
            "".into(),
        ]);
        let owner = cx.weak_entity();
        let loading = self.telegram_files_loading;
        let exhausted = self.telegram_files_exhausted;
        let select_all_label = self.tr("telegram-files-select-all");
        let select_file_label = self.tr("telegram-file-select-action");
        let download_label = self.tr("telegram-file-download-action");
        let empty_label = self.tr(if loading {
            "telegram-files-loading"
        } else {
            "telegram-files-empty"
        });
        self.channel_file_table.update(cx, |table, table_cx| {
            let delegate = table.delegate_mut();
            delegate.rows = rows;
            delegate.columns = columns;
            delegate.owner = Some(owner);
            delegate.loading = loading;
            delegate.exhausted = exhausted;
            delegate.select_all_label = select_all_label;
            delegate.select_file_label = select_file_label;
            delegate.download_label = download_label;
            delegate.empty_label = empty_label;
            table.refresh(table_cx);
        });
    }

    pub(crate) fn notify_channel_file_table(&self, cx: &mut Context<Self>) {
        self.channel_file_table
            .update(cx, |_, table_cx| table_cx.notify());
    }

    pub(crate) fn set_channel_file_selected(
        &mut self,
        message_id: i64,
        selected: bool,
        cx: &mut Context<Self>,
    ) {
        if selected {
            self.selected_channel_message_ids.insert(message_id);
        } else {
            self.selected_channel_message_ids.remove(&message_id);
        }
        self.notify_channel_file_table(cx);
        cx.notify();
    }

    pub(crate) fn set_all_channel_files_selected(
        &mut self,
        selected: bool,
        cx: &mut Context<Self>,
    ) {
        if selected {
            let ids = self
                .channel_filtered_files()
                .into_iter()
                .map(|file| file.message_id)
                .collect::<Vec<_>>();
            self.selected_channel_message_ids.extend(ids);
        } else {
            self.selected_channel_message_ids.clear();
        }
        self.notify_channel_file_table(cx);
        cx.notify();
    }

    fn channel_filtered_files(&self) -> Vec<&TelegramFileSummary> {
        let now_unix_ms = current_unix_millis();
        self.telegram_files
            .iter()
            .filter(|file| {
                channel_file_matches_filters(
                    file,
                    self.channel_batch_period,
                    &self.channel_batch_kinds,
                    now_unix_ms,
                )
            })
            .collect()
    }

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
            .when(!layout.is_compact(), |list| list.w(px(220.0)).flex_none())
            .p_1()
            .when(layout.is_compact(), |list| list.max_h(px(220.0)))
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
                    .h(px(34.0))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
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
            .p_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(components::section_title(selected_name)),
                    )
                    .child(
                        components::icon_button(
                            "telegram-files-refresh",
                            IconName::Redo2,
                            self.tr("telegram-files-refresh-action"),
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
            .when_some(self.telegram_index.as_ref(), |detail, progress| {
                detail.child(
                    div()
                        .mt_2()
                        .px_3()
                        .py_2()
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
            (FileKind::Video, "telegram-batch-kind-video"),
            (FileKind::Document, "telegram-batch-kind-document"),
            (FileKind::Archive, "telegram-batch-kind-archive"),
            (FileKind::Audio, "telegram-batch-kind-audio"),
            (FileKind::Image, "telegram-batch-kind-image"),
            (FileKind::Other, "telegram-batch-kind-other"),
        ];
        components::card()
            .mt_2()
            .px_3()
            .py_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
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
                                    this.selected_channel_message_ids.clear();
                                    this.refresh_channel_file_table(cx);
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
                .child(
                    div()
                        .mt_2()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .child(
                            components::button(
                                "telegram-batch-kind-all",
                                self.tr("telegram-batch-kind-all"),
                                None,
                                self.channel_batch_kinds.is_empty(),
                            )
                            .disabled(preparing)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.channel_batch_kinds.clear();
                                this.channel_batch_activity = ChannelBatchActivity::Idle;
                                this.selected_channel_message_ids.clear();
                                this.refresh_channel_file_table(cx);
                                cx.notify();
                            })),
                        )
                        .children(kinds.into_iter().enumerate().map(|(index, (kind, label))| {
                            components::button(
                                ("telegram-batch-kind", index),
                                self.tr(label),
                                None,
                                self.channel_batch_kinds.contains(&kind),
                            )
                            .disabled(preparing)
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    if !this.channel_batch_kinds.remove(&kind) {
                                        this.channel_batch_kinds.insert(kind);
                                    }
                                    this.channel_batch_activity = ChannelBatchActivity::Idle;
                                    this.selected_channel_message_ids.clear();
                                    this.refresh_channel_file_table(cx);
                                    cx.notify();
                                },
                            ))
                        })),
                )
                .child(
                    div()
                        .mt_3()
                        .text_xs()
                        .text_color(theme::text_secondary())
                        .child(self.tr_with(
                            "telegram-files-title",
                            MessageArgs::new().with(
                                "count",
                                format_integer(
                                    self.locale(),
                                    self.channel_filtered_files().len() as u64,
                                ),
                            ),
                        )),
                )
            })
            .child(
                div()
                    .mt_2()
                    .h(px(40.0))
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(components::badge(
                        self.tr_with(
                            "telegram-files-selected",
                            MessageArgs::new().with(
                                "count",
                                format_integer(
                                    self.locale(),
                                    self.selected_channel_message_ids.len() as u64,
                                ),
                            ),
                        ),
                        Tone::Blue,
                    ))
                    .child(
                        components::button(
                            "telegram-files-select-all-results",
                            self.tr("telegram-files-select-all"),
                            None,
                            false,
                        )
                        .disabled(self.channel_filtered_files().is_empty())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.set_all_channel_files_selected(true, cx);
                        })),
                    )
                    .child(
                        components::button(
                            "telegram-files-clear-selection",
                            self.tr("telegram-files-clear-selection"),
                            None,
                            false,
                        )
                        .disabled(self.selected_channel_message_ids.is_empty())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.set_all_channel_files_selected(false, cx);
                        })),
                    )
                    .child(div().flex_1())
                    .when_some(channel_batch_status(self), |row, (label, tone)| {
                        row.child(components::badge(label, tone))
                    })
                    .child(
                        components::button(
                            "telegram-batch-download",
                            self.tr("telegram-batch-download-action"),
                            Some(IconName::ArrowDown),
                            true,
                        )
                        .disabled(preparing || self.selected_channel_message_ids.is_empty())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.download_filtered_telegram_files(cx);
                        })),
                    ),
            )
            .into_any_element()
    }

    fn render_telegram_file_list(
        &self,
        layout: LayoutPolicy,
        _cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected_message = self.selected_telegram_message_id.and_then(|message_id| {
            self.telegram_files
                .iter()
                .find(|file| file.message_id == message_id)
        });
        let table = div()
            .min_w_0()
            .when(!layout.is_compact(), |panel| panel.flex_1().min_h_0())
            .when(layout.is_compact(), |panel| {
                panel.h(px(layout.channel_compact_file_list_height()))
            })
            .border_1()
            .border_color(theme::border())
            .rounded(theme::RADIUS_SMALL)
            .overflow_hidden()
            .child(
                Table::new(&self.channel_file_table)
                    .small()
                    .bordered(false)
                    .scrollbar_visible(true, false),
            );
        div()
            .mt_2()
            .when(!layout.is_compact(), |browser| {
                browser.flex_1().min_h_0().flex().gap_3()
            })
            .when(layout.is_compact(), |browser| browser.flex().flex_col())
            .child(table)
            .child(self.render_telegram_message_detail(selected_message, layout))
            .into_any_element()
    }

    fn render_telegram_message_detail(
        &self,
        file: Option<&TelegramFileSummary>,
        layout: LayoutPolicy,
    ) -> AnyElement {
        let detail = components::card()
            .when(!layout.is_compact(), |detail| {
                detail.w(px(300.0)).h_full().flex_none()
            })
            .when(layout.is_compact(), |detail| detail.mt_3())
            .p_3()
            .overflow_y_scrollbar()
            .child(components::section_title(
                self.tr("telegram-message-detail-title"),
            ));
        let Some(file) = file else {
            return detail
                .child(
                    div()
                        .mt_4()
                        .text_sm()
                        .text_color(theme::text_secondary())
                        .child(self.tr("telegram-message-detail-empty")),
                )
                .into_any_element();
        };
        let caption = if file.caption.trim().is_empty() {
            self.tr("telegram-message-no-caption")
        } else {
            file.caption.clone().into()
        };
        detail
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

fn channel_table_columns(names: [SharedString; 6]) -> Vec<Column> {
    [
        ("selected", 36.0),
        ("name", 280.0),
        ("sent_at", 150.0),
        ("kind", 96.0),
        ("size", 104.0),
        ("action", 44.0),
    ]
    .into_iter()
    .zip(names)
    .map(|((key, width), name)| {
        Column::new(key, name)
            .width(px(width))
            .resizable(false)
            .movable(false)
            .selectable(false)
    })
    .collect()
}

fn table_cell(value: SharedString) -> AnyElement {
    div()
        .size_full()
        .min_w_0()
        .flex()
        .items_center()
        .truncate()
        .child(value)
        .into_any_element()
}

fn current_unix_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or_default()
}

fn channel_file_matches_filters(
    file: &TelegramFileSummary,
    period: ChannelBatchPeriod,
    kinds: &HashSet<FileKind>,
    now_unix_ms: i64,
) -> bool {
    let maximum_age_ms = match period {
        ChannelBatchPeriod::AnyTime => None,
        ChannelBatchPeriod::Past24Hours => Some(24 * 60 * 60 * 1_000),
        ChannelBatchPeriod::Past7Days => Some(7 * 24 * 60 * 60 * 1_000),
        ChannelBatchPeriod::Past30Days => Some(30 * 24 * 60 * 60 * 1_000),
    };
    let matches_period = maximum_age_ms.is_none_or(|age| {
        file.sent_at_unix_ms >= now_unix_ms.saturating_sub(age)
            && file.sent_at_unix_ms <= now_unix_ms
    });
    let matches_kind = kinds.is_empty()
        || kinds.contains(&teleark_runtime::classify_file(Path::new(&file.file_name)));
    matches_period && matches_kind
}

fn channel_batch_status(app: &TeleArkApp) -> Option<(SharedString, Tone)> {
    match app.channel_batch_activity {
        ChannelBatchActivity::Idle => app.telegram_download.map(|(_, state)| {
            let (message, tone) = match state {
                ChannelDownloadState::Queued => ("telegram-download-queued", Tone::Amber),
                ChannelDownloadState::Running => ("telegram-download-running", Tone::Blue),
                ChannelDownloadState::Paused => ("telegram-download-paused", Tone::Amber),
                ChannelDownloadState::Completed => ("telegram-download-completed", Tone::Green),
                ChannelDownloadState::Failed(_) => ("telegram-download-failed", Tone::Red),
                ChannelDownloadState::Cancelled => ("telegram-download-cancelled", Tone::Red),
            };
            (app.tr(message), tone)
        }),
        ChannelBatchActivity::Preparing => Some((app.tr("telegram-batch-preparing"), Tone::Blue)),
        ChannelBatchActivity::Queued { count, .. } => Some((
            app.tr_with(
                "telegram-batch-queued",
                MessageArgs::new().with("count", format_integer(app.locale(), count as u64)),
            ),
            Tone::Green,
        )),
        ChannelBatchActivity::NoMatches => Some((app.tr("telegram-batch-no-matches"), Tone::Amber)),
        ChannelBatchActivity::Failed(_) => Some((app.tr("telegram-batch-failed"), Tone::Red)),
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
    fn one_channel_page_supports_five_thousand_filtered_rows() {
        let now = 2_000_000_000_000;
        let files = (0..CHANNEL_FILE_PAGE_SIZE)
            .map(|message_id| TelegramFileSummary {
                message_id: message_id as i64,
                modified_at_unix_ms: now - 1_000,
                file_name: format!("archive-{message_id}.zip"),
                caption: String::new(),
                mime_type: None,
                size_bytes: 1,
                sent_at_unix_ms: now - 1_000,
            })
            .collect::<Vec<_>>();
        let kinds = HashSet::from([FileKind::Archive]);
        assert_eq!(
            files
                .iter()
                .filter(|file| channel_file_matches_filters(
                    file,
                    ChannelBatchPeriod::Past24Hours,
                    &kinds,
                    now,
                ))
                .count(),
            CHANNEL_FILE_PAGE_SIZE
        );
    }
}
