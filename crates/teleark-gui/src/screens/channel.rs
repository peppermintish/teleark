use std::{collections::HashSet, path::Path};

use gpui_kit::component::{
    Disableable as _, Icon, IconName, Sizable as _,
    button::ButtonVariants as _,
    checkbox::Checkbox,
    input::Input,
    spinner::Spinner,
    table::{Column, DataTable, TableDelegate, TableState},
    tooltip::Tooltip,
};
use gpui_kit::{
    AnyElement, App, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, WeakEntity, Window, div,
    prelude::FluentBuilder as _, px,
};
use qrcode::{QrCode, types::Color};
use teleark_core::FileKind;
use teleark_i18n::{
    MessageArgs,
    format::{format_bytes, format_integer, format_unix_millis},
};
use teleark_runtime::{
    ChannelDownloadState, ManagedVaultFile, TelegramAuthState, TelegramFileSummary,
};

use crate::{
    app::{
        ChannelBatchActivity, ChannelBatchPeriod, StorageView, TeleArkApp, TelegramActivity,
        telegram_login_controls_enabled,
    },
    components::{self, Tone},
    layout::LayoutPolicy,
    theme,
};

pub(crate) const CHANNEL_FILE_LIST_CAPACITY: usize = 5_000;
pub(crate) const CHANNEL_FILE_INITIAL_SCAN: usize = 200;
pub(crate) const CHANNEL_FILE_LOAD_MORE_SCAN: usize = 1_000;
pub(crate) const CHANNEL_FILE_SCAN_CHUNK: usize = 200;

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
    load_scope: Option<(i64, i64, u64)>,
    loading: bool,
    exhausted: bool,
    select_all_label: SharedString,
    select_file_label: SharedString,
    download_label: SharedString,
    empty_label: SharedString,
    loading_label: SharedString,
    slow_label: SharedString,
    cancel_label: SharedString,
    retry_label: SharedString,
    slow: bool,
    failed: bool,
}

impl ChannelFileTableDelegate {
    pub(crate) fn new() -> Self {
        Self {
            columns: channel_table_columns(["", "", "", "", "", ""].map(Into::into)),
            rows: Vec::new(),
            owner: None,
            load_scope: None,
            loading: false,
            exhausted: true,
            select_all_label: "".into(),
            select_file_label: "".into(),
            download_label: "".into(),
            empty_label: "".into(),
            loading_label: "".into(),
            slow_label: "".into(),
            cancel_label: "".into(),
            retry_label: "".into(),
            slow: false,
            failed: false,
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

    fn column(&self, col_ix: usize, _cx: &App) -> Column {
        self.columns[col_ix].clone()
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
                                        cx.stop_propagation();
                                        app.set_channel_file_selected(row.message_id, *checked, cx);
                                    });
                                }
                            }),
                    )
                    .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
                    .into_any_element()
            }
            1 => {
                let local = self.owner.as_ref().and_then(|owner| {
                    owner
                        .read_with(cx, |app, _| {
                            app.selected_chat_id
                                .and_then(|chat_id| {
                                    app.local_download_for_source(
                                        chat_id,
                                        Some(row.message_id),
                                        None,
                                    )
                                })
                                .map(|item| {
                                    (app.local_presence_label(Some(item.presence)), item.presence)
                                })
                        })
                        .ok()
                        .flatten()
                });
                div()
                    .size_full()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(Icon::new(IconName::File).text_color(theme::blue()))
                    .child(div().min_w_0().flex_1().truncate().child(row.name))
                    .when_some(local, |cell, (label, presence)| {
                        cell.child(components::badge(
                            label,
                            if presence == teleark_runtime::LocalFilePresence::Present {
                                Tone::Green
                            } else {
                                Tone::Amber
                            },
                        ))
                    })
                    .into_any_element()
            }
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
                                    cx.stop_propagation();
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
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .text_sm()
            .text_color(theme::text_secondary())
            .when(self.loading, |content| {
                let owner = self.owner.clone();
                content
                    .child(Spinner::new().small().color(theme::blue().into()))
                    .child(self.loading_label.clone())
                    .when(self.slow, |content| {
                        content.child(
                            div()
                                .text_xs()
                                .text_color(theme::text_muted())
                                .child(self.slow_label.clone()),
                        )
                    })
                    .child(
                        components::button(
                            "channel-files-cancel-empty",
                            self.cancel_label.clone(),
                            None,
                            false,
                        )
                        .on_click(move |_, _, cx| {
                            if let Some(owner) = owner.as_ref() {
                                let _ = owner.update(cx, |app, cx| {
                                    app.cancel_telegram_file_load(cx);
                                });
                            }
                        }),
                    )
            })
            .when(!self.loading, |content| {
                content.child(self.empty_label.clone())
            })
            .when(self.failed, |content| {
                let owner = self.owner.clone();
                content.child(
                    components::button(
                        "channel-files-retry-empty",
                        self.retry_label.clone(),
                        Some(IconName::Redo2),
                        false,
                    )
                    .on_click(move |_, _, cx| {
                        if let Some(owner) = owner.as_ref() {
                            let _ = owner.update(cx, |app, cx| {
                                app.retry_telegram_file_load(cx);
                            });
                        }
                    }),
                )
            })
    }

    fn loading(&self, _cx: &App) -> bool {
        false
    }

    fn has_more(&self, _cx: &App) -> bool {
        !self.loading && !self.exhausted && !self.failed && self.load_scope.is_some()
    }

    fn load_more_threshold(&self) -> usize {
        100
    }

    fn load_more(&mut self, _window: &mut Window, cx: &mut Context<TableState<Self>>) {
        if !self.has_more(cx) {
            return;
        }
        self.loading = true;
        if let Some(owner) = self.owner.clone() {
            let scope = self.load_scope;
            // TableState calls its delegate while leased. Refreshing it through
            // the owner in this stack would re-enter the same entity update.
            // Use App::defer, not defer_in (which leases TableState again).
            cx.defer(move |cx| {
                let _ = owner.update(cx, |app, cx| {
                    if app.channel_file_load_scope() == scope {
                        app.load_selected_telegram_files(true, cx);
                    }
                    // Also release a queued load rejected after source/route change.
                    if !app.telegram_files_loading {
                        app.refresh_channel_file_table(cx);
                    }
                });
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
        let load_scope = self.channel_file_load_scope();
        let loading = self.telegram_files_loading;
        let exhausted = self.telegram_files_exhausted;
        let select_all_label = self.tr("telegram-files-select-all");
        let select_file_label = self.tr("telegram-file-select-action");
        let download_label = self.tr("telegram-file-download-action");
        let empty_label = self.tr(if loading {
            "telegram-files-loading"
        } else if matches!(self.telegram_activity, TelegramActivity::Failed(_)) {
            "telegram-files-load-failed"
        } else {
            "telegram-files-empty"
        });
        let loading_label = self.tr_with(
            "telegram-files-fetching-progress",
            MessageArgs::new()
                .with(
                    "scanned",
                    format_integer(self.locale(), self.telegram_files_scanned),
                )
                .with(
                    "target",
                    format_integer(self.locale(), self.telegram_files_scan_target),
                ),
        );
        let slow_label = self.tr("telegram-files-fetching-slow");
        let cancel_label = self.tr("telegram-files-cancel-action");
        let retry_label = self.tr("telegram-files-retry-action");
        let slow = self.telegram_files_slow;
        let failed = matches!(self.telegram_activity, TelegramActivity::Failed(_));
        self.channel_file_table.update(cx, |table, table_cx| {
            let delegate = table.delegate_mut();
            delegate.rows = rows;
            delegate.columns = columns;
            delegate.owner = Some(owner);
            delegate.load_scope = load_scope;
            delegate.loading = loading;
            delegate.exhausted = exhausted;
            delegate.select_all_label = select_all_label;
            delegate.select_file_label = select_file_label;
            delegate.download_label = download_label;
            delegate.empty_label = empty_label;
            delegate.loading_label = loading_label;
            delegate.slow_label = slow_label;
            delegate.cancel_label = cancel_label;
            delegate.retry_label = retry_label;
            delegate.slow = slow;
            delegate.failed = failed;
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
        window: &mut Window,
        layout: LayoutPolicy,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if !matches!(self.telegram_auth, TelegramAuthState::Authorized(_)) {
            return self.render_account(window, layout, cx);
        }
        let name = self
            .selected_chat_id
            .and_then(|id| self.telegram_chats.iter().find(|chat| chat.id == id))
            .map(|chat| chat.name.clone())
            .unwrap_or_else(|| self.tr("nav-channels").to_string());
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .px(px(layout.content_padding()))
            .pb(px(layout.content_padding()))
            .child(
                div()
                    .h(px(76.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        Icon::new(crate::assets::Symbol::Hash)
                            .size(px(26.0))
                            .text_color(theme::text_secondary()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(23.0))
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(name),
                    )
                    .child(self.telegram_status_badge()),
            )
            .child(self.render_telegram_channels(layout, cx))
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

    pub(crate) fn render_telegram_login_methods(
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
            .w_full()
            .max_w(px(760.0))
            .mx_auto()
            .p(px(layout.content_padding().max(20.0)))
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
                                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
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
                                this.show_telegram_api_id_prompt = true;
                                cx.notify();
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

    pub(crate) fn render_telegram_code(&self, cx: &mut Context<Self>) -> AnyElement {
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
                cx.listener(|this, _, window, cx| this.submit_telegram_code(window, cx)),
            ))
            .into_any_element()
    }

    pub(crate) fn render_telegram_password(
        &self,
        hint: Option<&str>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
                cx.listener(|this, _, window, cx| this.submit_telegram_password(window, cx)),
            ))
            .into_any_element()
    }

    pub(crate) fn render_telegram_channels(
        &self,
        layout: LayoutPolicy,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.viewing_storage() && self.storage_view == StorageView::Files {
            return self.render_storage_managed(layout, cx);
        }

        let selected_name = if self.viewing_storage() {
            self.tr("storage-channel-title").to_string()
        } else {
            self.selected_chat_id
                .and_then(|id| self.telegram_chats.iter().find(|chat| chat.id == id))
                .map(|chat| chat.name.clone())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| self.tr("telegram-no-channel-selected").to_string())
        };
        let detail = components::card()
            .flex_1()
            .min_w_0()
            .h_full()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_hidden()
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
                    .when(self.channel_batch_expanded, |header| {
                        header.child(
                            components::button(
                                "telegram-index-next",
                                self.tr("telegram-index-next-action"),
                                Some(IconName::Search),
                                false,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.index_selected_telegram_chat(cx);
                            })),
                        )
                    }),
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
            .h_full()
            .min_h_0()
            .child(detail)
            .into_any_element()
    }

    pub(crate) fn render_managed_package_detail(
        &self,
        package: Option<&ManagedVaultFile>,
        _layout: LayoutPolicy,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let detail = div().w_full().p_4().child(components::section_title(
            self.tr("storage-channel-managed-detail-title"),
        ));
        let Some(package) = package else {
            return components::inspector_body(
                "managed-detail-body",
                &self.raw_detail_scroll,
                detail.child(
                    div()
                        .mt_4()
                        .text_sm()
                        .text_color(theme::text_secondary())
                        .child(self.tr("storage-channel-managed-detail-empty")),
                ),
            );
        };
        components::inspector_body(
            "managed-detail-body",
            &self.raw_detail_scroll,
            detail
                .when_some(
                    self.selected_chat_id.and_then(|chat_id| {
                        self.local_download_for_source(chat_id, None, Some(&package.package_id))
                    }),
                    |detail, observation| {
                        detail
                            .child(message_detail_row(
                                self.tr("local-file-status"),
                                self.local_presence_label(Some(observation.presence)),
                            ))
                            .child(self.local_download_actions(observation))
                    },
                )
                .child(message_detail_row(
                    self.tr("storage-channel-package-id"),
                    package.package_id.clone().into(),
                ))
                .child(message_detail_row(
                    self.tr("storage-channel-logical-name"),
                    package.logical_name.clone().into(),
                ))
                .child(message_detail_row(
                    self.tr("storage-channel-manifest-state"),
                    self.tr("storage-channel-manifest-authenticated"),
                ))
                .child(message_detail_row(
                    self.tr("table-parts"),
                    format_integer(self.locale(), package.part_count as u64).into(),
                ))
                .child(message_detail_row(
                    self.tr("storage-channel-encoded-size"),
                    format_bytes(self.locale(), package.encoded_size_bytes).into(),
                ))
                .child(message_detail_row(
                    self.tr("storage-channel-restore-state"),
                    self.tr("storage-channel-restore-ready"),
                ))
                .child(
                    div()
                        .mt_4()
                        .text_xs()
                        .text_color(theme::text_muted())
                        .child(self.tr("storage-channel-related-files")),
                )
                .children(package.related_remote_names.iter().map(|file| {
                    div()
                        .mt_2()
                        .p_2()
                        .rounded(theme::RADIUS_SMALL)
                        .bg(theme::sidebar())
                        .text_xs()
                        .child(file.clone())
                }))
                .child(
                    components::button(
                        "storage-channel-download-managed",
                        self.tr("storage-channel-download-restored-action"),
                        Some(IconName::ArrowDown),
                        true,
                    )
                    .mt_4()
                    .disabled(self.vault_activity == crate::app::VaultActivity::Working)
                    .on_click({
                        let package_id = package.package_numeric_id;
                        cx.listener(move |this, _, _, cx| {
                            this.download_managed_vault_file(package_id, cx);
                        })
                    }),
                ),
        )
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
        div()
            .mt_2()
            .py_2()
            .border_y_1()
            .border_color(theme::border_subtle())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        components::button(
                            "telegram-batch-toggle",
                            self.tr("transfer-filter-title"),
                            Some(IconName::Settings2),
                            self.channel_batch_expanded,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.channel_batch_expanded = !this.channel_batch_expanded;
                            cx.notify();
                        })),
                    )
                    .child(
                        div()
                            .flex_1()
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
                    .min_h(px(40.0))
                    .flex()
                    .flex_wrap()
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
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name_width =
            px((layout.raw_table_width(self.page == crate::app::Page::Channel) - 430.0).max(220.0));
        if self.channel_file_table.read(cx).delegate().columns[1].width != name_width {
            self.channel_file_table.update(cx, |table, cx| {
                table.delegate_mut().columns[1].width = name_width;
                table.refresh(cx);
            });
        }
        let selected_message = self.selected_telegram_message_id.and_then(|message_id| {
            self.telegram_files
                .iter()
                .find(|file| file.message_id == message_id)
        });
        let table = div()
            .min_w_0()
            .flex_1()
            .h_full()
            .min_h_0()
            .border_1()
            .border_color(theme::border())
            .rounded(theme::RADIUS_SMALL)
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(
                div().flex_1().min_h_0().child(
                    DataTable::new(&self.channel_file_table)
                        .small()
                        .bordered(false)
                        .scrollbar_visible(true, true),
                ),
            )
            .when(
                self.telegram_files_loading && !self.telegram_files.is_empty(),
                |table| table.child(self.render_telegram_fetch_footer(cx)),
            )
            .when(
                !self.telegram_files_loading
                    && !self.telegram_files.is_empty()
                    && matches!(self.telegram_activity, TelegramActivity::Failed(_)),
                |table| table.child(self.render_telegram_retry_footer(cx)),
            );
        div()
            .mt_2()
            .flex_1()
            .min_h_0()
            .relative()
            .flex()
            .child(table)
            .when_some(
                selected_message.filter(|_| self.show_channel_detail),
                |browser, message| {
                    browser.child(
                        components::inspector_panel("raw-file-inspector", 340.0)
                            .absolute()
                            .right_0()
                            .top_0()
                            .bottom_0()
                            .shadow_lg()
                            .child(
                                div()
                                    .px_3()
                                    .py_2()
                                    .flex()
                                    .items_center()
                                    .justify_end()
                                    .child(
                                        components::icon_button(
                                            "channel-close-detail",
                                            IconName::Close,
                                            self.tr("action-close-details"),
                                        )
                                        .ghost()
                                        .on_click(
                                            cx.listener(|this, _, _, cx| {
                                                this.show_channel_detail = false;
                                                cx.notify();
                                            }),
                                        ),
                                    ),
                            )
                            .child(self.render_telegram_message_detail(Some(message), layout)),
                    )
                },
            )
            .into_any_element()
    }

    fn render_telegram_fetch_footer(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .min_h(px(42.0))
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .border_t_1()
            .border_color(theme::border())
            .bg(theme::surface())
            .child(Spinner::new().small().color(theme::blue().into()))
            .child(
                div().text_xs().text_color(theme::text_secondary()).child(
                    self.tr_with(
                        "telegram-files-fetching-progress",
                        MessageArgs::new()
                            .with(
                                "scanned",
                                format_integer(self.locale(), self.telegram_files_scanned),
                            )
                            .with(
                                "target",
                                format_integer(self.locale(), self.telegram_files_scan_target),
                            ),
                    ),
                ),
            )
            .when(self.telegram_files_slow, |footer| {
                footer.child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .truncate()
                        .text_xs()
                        .text_color(theme::text_muted())
                        .child(self.tr("telegram-files-fetching-slow")),
                )
            })
            .when(!self.telegram_files_slow, |footer| {
                footer.child(div().flex_1())
            })
            .child(
                components::button(
                    "channel-files-cancel-footer",
                    self.tr("telegram-files-cancel-action"),
                    None,
                    false,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.cancel_telegram_file_load(cx);
                })),
            )
            .into_any_element()
    }

    fn render_telegram_retry_footer(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .min_h(px(42.0))
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .border_t_1()
            .border_color(theme::border())
            .bg(theme::surface())
            .child(
                div()
                    .flex_1()
                    .text_xs()
                    .text_color(theme::red())
                    .child(self.tr("telegram-files-load-failed")),
            )
            .child(
                components::button(
                    "channel-files-retry-footer",
                    self.tr("telegram-files-retry-action"),
                    Some(IconName::Redo2),
                    false,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.retry_telegram_file_load(cx);
                })),
            )
            .into_any_element()
    }

    fn render_telegram_message_detail(
        &self,
        file: Option<&TelegramFileSummary>,
        _layout: LayoutPolicy,
    ) -> AnyElement {
        let detail = div().w_full().p_4().child(components::section_title(
            self.tr("telegram-message-detail-title"),
        ));
        let Some(file) = file else {
            return components::inspector_body(
                "raw-detail-body",
                &self.raw_detail_scroll,
                detail.child(
                    div()
                        .mt_4()
                        .text_sm()
                        .text_color(theme::text_secondary())
                        .child(self.tr("telegram-message-detail-empty")),
                ),
            );
        };
        let caption = if file.caption.trim().is_empty() {
            self.tr("telegram-message-no-caption")
        } else {
            file.caption.clone().into()
        };
        let teleark_descriptor = teleark_remote_file(&file.file_name, &file.caption);
        let related_files = teleark_descriptor.as_ref().map(|descriptor| {
            self.telegram_files
                .iter()
                .filter(|candidate| {
                    teleark_remote_file(&candidate.file_name, &candidate.caption)
                        .is_some_and(|candidate| candidate.package_id == descriptor.package_id)
                })
                .collect::<Vec<_>>()
        });
        components::inspector_body(
            "raw-detail-body",
            &self.raw_detail_scroll,
            detail
                .when_some(
                    self.selected_chat_id.and_then(|chat_id| {
                        self.local_download_for_source(chat_id, Some(file.message_id), None)
                    }),
                    |detail, observation| {
                        detail
                            .child(message_detail_row(
                                self.tr("local-file-status"),
                                self.local_presence_label(Some(observation.presence)),
                            ))
                            .child(self.local_download_actions(observation))
                    },
                )
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
                .when_some(teleark_descriptor, |detail, descriptor| {
                    let (role, explanation) = match descriptor.role {
                        TeleArkRemoteRole::Manifest => (
                            self.tr("storage-channel-role-manifest"),
                            self.tr("storage-channel-manifest-explanation"),
                        ),
                        TeleArkRemoteRole::Part(index) => (
                            self.tr_with(
                                "storage-channel-role-part",
                                MessageArgs::new().with(
                                    "index",
                                    format_integer(self.locale(), u64::from(index) + 1),
                                ),
                            ),
                            self.tr("storage-channel-part-explanation"),
                        ),
                    };
                    detail
                        .child(message_detail_row(
                            self.tr("storage-channel-file-role"),
                            role,
                        ))
                        .child(message_detail_row(
                            self.tr("storage-channel-why-file-exists"),
                            explanation,
                        ))
                        .child(message_detail_row(
                            self.tr("storage-channel-package-id"),
                            descriptor.package_id.into(),
                        ))
                        .child(message_detail_row(
                            self.tr("storage-channel-logical-name"),
                            self.tr("storage-channel-managed-name-locked"),
                        ))
                })
                .when_some(related_files, |detail, related| {
                    detail
                        .child(
                            div()
                                .mt_4()
                                .text_xs()
                                .text_color(theme::text_muted())
                                .child(self.tr("storage-channel-related-files")),
                        )
                        .children(related.into_iter().map(|related| {
                            div()
                                .mt_2()
                                .p_2()
                                .rounded(theme::RADIUS_SMALL)
                                .bg(theme::sidebar())
                                .text_xs()
                                .child(related.file_name.clone())
                        }))
                }),
        )
    }

    pub(crate) fn telegram_error_message(&self) -> Option<gpui_kit::SharedString> {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TeleArkRemoteRole {
    Manifest,
    Part(u32),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TeleArkRemoteFile {
    package_id: String,
    role: TeleArkRemoteRole,
}

#[cfg(test)]
#[derive(Clone, Debug, Eq, PartialEq)]
struct SavedPackageFile {
    file_name: String,
}

#[cfg(test)]
#[derive(Clone, Debug, Eq, PartialEq)]
struct SavedMessagePackage {
    package_id: String,
    reference_message_id: i64,
    has_manifest: bool,
    part_count: usize,
    encoded_size: u64,
    files: Vec<SavedPackageFile>,
}

fn teleark_remote_file(file_name: &str, caption: &str) -> Option<TeleArkRemoteFile> {
    let (package_id, suffix) = file_name.split_once(".v1.")?;
    if package_id.len() != 32 || !package_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let role = if suffix == "manifest.tam" && caption == "teleark-manifest-v1" {
        TeleArkRemoteRole::Manifest
    } else {
        let index = suffix.strip_suffix(".part.tav")?.parse::<u32>().ok()?;
        let expected_caption = format!("teleark-object-v1-{package_id}-{index:08x}");
        if caption != expected_caption {
            return None;
        }
        TeleArkRemoteRole::Part(index)
    };
    Some(TeleArkRemoteFile {
        package_id: package_id.to_owned(),
        role,
    })
}

#[cfg(test)]
fn saved_message_packages(files: &[TelegramFileSummary]) -> Vec<SavedMessagePackage> {
    let mut packages: std::collections::BTreeMap<String, SavedMessagePackage> =
        std::collections::BTreeMap::new();
    for file in files {
        let Some(descriptor) = teleark_remote_file(&file.file_name, &file.caption) else {
            continue;
        };
        let package = packages
            .entry(descriptor.package_id.clone())
            .or_insert_with(|| SavedMessagePackage {
                package_id: descriptor.package_id,
                reference_message_id: file.message_id,
                has_manifest: false,
                part_count: 0,
                encoded_size: 0,
                files: Vec::new(),
            });
        package.encoded_size = package.encoded_size.saturating_add(file.size_bytes);
        match descriptor.role {
            TeleArkRemoteRole::Manifest => {
                package.has_manifest = true;
                package.reference_message_id = file.message_id;
            }
            TeleArkRemoteRole::Part(_) => package.part_count = package.part_count.saturating_add(1),
        }
        package.files.push(SavedPackageFile {
            file_name: file.file_name.clone(),
        });
    }
    let mut packages: Vec<_> = packages.into_values().collect();
    packages.sort_by_key(|package| std::cmp::Reverse(package.reference_message_id));
    packages
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

fn message_detail_row(
    label: gpui_kit::SharedString,
    value: gpui_kit::SharedString,
) -> gpui_kit::Div {
    div()
        .mt_3()
        .child(div().text_xs().text_color(theme::text_muted()).child(label))
        .child(div().mt_1().text_sm().whitespace_normal().child(value))
}

fn credential_qr_placeholder(size: f32, label: gpui_kit::SharedString) -> AnyElement {
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
        .bg(gpui_kit::rgb(0xffffff))
        .border_1()
        .border_color(theme::border())
        .children((0..width).map(|row| {
            div().flex().children((0..width).map(|column| {
                div()
                    .size(px(cell))
                    .bg(if colors[row * width + column] == Color::Dark {
                        gpui_kit::rgb(0x000000)
                    } else {
                        gpui_kit::rgb(0xffffff)
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
    label: gpui_kit::SharedString,
    state: &gpui_kit::Entity<gpui_kit::component::input::InputState>,
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
    label: gpui_kit::SharedString,
    icon: IconName,
    disabled: bool,
    listener: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut gpui_kit::App) + 'static,
) -> AnyElement {
    components::button(id, label, Some(icon), true)
        .mt_5()
        .w_full()
        .h(px(36.0))
        .disabled(disabled)
        .on_click(listener)
        .into_any_element()
}

fn error_banner(message: gpui_kit::SharedString) -> AnyElement {
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
    fn one_channel_page_supports_five_thousand_filtered_rows() {
        let now = 2_000_000_000_000;
        let files = (0..CHANNEL_FILE_LIST_CAPACITY)
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
            CHANNEL_FILE_LIST_CAPACITY
        );
    }

    #[test]
    fn source_browser_separates_render_capacity_from_network_batch_sizes() {
        assert_eq!(CHANNEL_FILE_INITIAL_SCAN, CHANNEL_FILE_SCAN_CHUNK);
        assert_eq!(CHANNEL_FILE_INITIAL_SCAN, 200);
        assert_eq!(CHANNEL_FILE_LOAD_MORE_SCAN, 1_000);
        assert_eq!(CHANNEL_FILE_LIST_CAPACITY, 5_000);
        assert_eq!(CHANNEL_FILE_LOAD_MORE_SCAN % CHANNEL_FILE_SCAN_CHUNK, 0);
    }

    #[test]
    fn raw_storage_recognizes_only_versioned_teleark_remote_names() {
        let package = "0123456789abcdef0123456789abcdef";
        assert_eq!(
            teleark_remote_file(&format!("{package}.v1.manifest.tam"), "teleark-manifest-v1"),
            Some(TeleArkRemoteFile {
                package_id: package.to_owned(),
                role: TeleArkRemoteRole::Manifest,
            })
        );
        assert_eq!(
            teleark_remote_file(
                &format!("{package}.v1.000004.part.tav"),
                &format!("teleark-object-v1-{package}-00000004"),
            ),
            Some(TeleArkRemoteFile {
                package_id: package.to_owned(),
                role: TeleArkRemoteRole::Part(4),
            })
        );
        assert!(teleark_remote_file("report.v1.manifest.tam", "teleark-manifest-v1").is_none());
        assert!(
            teleark_remote_file(&format!("{package}.v2.manifest.tam"), "teleark-manifest-v1")
                .is_none()
        );
    }

    #[test]
    fn raw_storage_groups_manifest_and_parts_without_claiming_recovery() {
        let package = "0123456789abcdef0123456789abcdef";
        let files = [
            (
                1,
                format!("{package}.v1.000000.part.tav"),
                format!("teleark-object-v1-{package}-00000000"),
                40,
            ),
            (
                2,
                format!("{package}.v1.manifest.tam"),
                "teleark-manifest-v1".to_owned(),
                10,
            ),
            (3, "ordinary.pdf".to_owned(), String::new(), 20),
        ]
        .into_iter()
        .map(
            |(message_id, file_name, caption, size_bytes)| TelegramFileSummary {
                message_id,
                sent_at_unix_ms: 0,
                modified_at_unix_ms: 0,
                file_name,
                caption,
                mime_type: None,
                size_bytes,
            },
        )
        .collect::<Vec<_>>();
        let packages = saved_message_packages(&files);
        assert_eq!(packages.len(), 1);
        assert!(packages[0].has_manifest);
        assert_eq!(packages[0].part_count, 1);
        assert_eq!(packages[0].encoded_size, 50);
        assert_eq!(packages[0].files.len(), 2);
    }
}
