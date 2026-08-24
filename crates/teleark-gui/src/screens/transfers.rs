use gpui::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px,
};
use gpui_component::{Icon, IconName, scroll::ScrollableElement as _};

use crate::{
    app::TeleArkApp,
    components::{self, Tone},
    mock::{
        ActivityLog, ConnectionRow, TransferRow, TransferState, activity_logs, connections,
        transfers,
    },
    theme,
};

impl TeleArkApp {
    pub(crate) fn render_transfers(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let query = self.search_input.read(cx).value().to_lowercase();
        let transfer_rows: Vec<_> = transfers(self.upload_queued)
            .into_iter()
            .filter(|transfer| transfer_matches_nav(self.nav_selection, transfer.state))
            .filter(|transfer| {
                query.is_empty()
                    || transfer.name.to_lowercase().contains(&query)
                    || transfer.source.to_lowercase().contains(&query)
                    || transfer.destination.to_lowercase().contains(&query)
            })
            .collect();
        let selected_index = self
            .selected_file
            .min(transfer_rows.len().saturating_sub(1));
        let selected = transfer_rows.get(selected_index).cloned();

        let summary = div()
            .h(px(120.0))
            .px_4()
            .py_3()
            .flex()
            .gap_3()
            .child(summary_card(
                IconName::ArrowDown,
                self.tr("transfer-summary-downloading"),
                "8",
                self.tr("transfer-summary-tasks"),
                Tone::Blue,
            ))
            .child(summary_card(
                IconName::Calendar,
                self.tr("transfer-summary-waiting"),
                "156",
                self.tr("transfer-summary-ready"),
                Tone::Amber,
            ))
            .child(summary_card(
                IconName::CircleCheck,
                self.tr("transfer-summary-completed"),
                "3,842",
                self.tr("transfer-summary-today"),
                Tone::Green,
            ))
            .child(summary_card(
                IconName::CircleX,
                self.tr("transfer-summary-failed"),
                "12",
                self.tr("transfer-summary-retry"),
                Tone::Red,
            ))
            .child(summary_card(
                IconName::ArrowDown,
                self.tr("transfer-summary-total-speed"),
                "38.2 MB/s",
                "↓ 23.6   ↑ 14.6".into(),
                Tone::Blue,
            ))
            .child(summary_card(
                IconName::Inbox,
                self.tr("transfer-summary-today-data"),
                "186 GB",
                self.tr("transfer-summary-month-change"),
                Tone::Purple,
            ));

        let toolbar = div()
            .h(px(46.0))
            .px_4()
            .flex()
            .items_center()
            .gap_2()
            .child(components::button(
                "transfers-start-all",
                self.tr("action-start-all"),
                Some(IconName::ArrowRight),
                true,
            ))
            .child(
                components::button(
                    "transfers-pause-all",
                    if self.transfer_paused {
                        self.tr("action-resume-all")
                    } else {
                        self.tr("action-pause-all")
                    },
                    Some(if self.transfer_paused {
                        IconName::ArrowRight
                    } else {
                        IconName::Dash
                    }),
                    false,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.transfer_paused = !this.transfer_paused;
                    cx.notify();
                })),
            )
            .child(components::button(
                "transfers-retry",
                self.tr("action-retry-failed"),
                Some(IconName::Redo2),
                false,
            ))
            .child(components::button(
                "transfers-clear",
                self.tr("action-clear-completed"),
                Some(IconName::Delete),
                false,
            ))
            .child(components::button(
                "transfers-new-queue",
                self.tr("action-new-queue"),
                Some(IconName::Plus),
                false,
            ))
            .child(div().flex_1())
            .child(components::button(
                "transfers-status-filter",
                self.tr("filter-all-statuses"),
                Some(IconName::Settings2),
                false,
            ))
            .child(components::icon_button(
                "transfers-view",
                IconName::LayoutDashboard,
                self.tr("action-view-options"),
            ));

        let header = div()
            .h(px(34.0))
            .px_3()
            .flex()
            .items_center()
            .bg(theme::sidebar())
            .border_y_1()
            .border_color(theme::border())
            .child(transfer_header("", Some(28.0)))
            .child(transfer_header(self.tr("table-name"), None))
            .child(transfer_header(self.tr("table-source"), Some(108.0)))
            .child(transfer_header(self.tr("table-size"), Some(76.0)))
            .child(transfer_header(self.tr("table-progress"), Some(112.0)))
            .child(transfer_header(self.tr("table-speed"), Some(82.0)))
            .child(transfer_header(self.tr("table-eta"), Some(64.0)))
            .child(transfer_header(self.tr("table-status"), Some(94.0)))
            .child(transfer_header(self.tr("table-destination"), Some(118.0)));

        let rows = transfer_rows
            .into_iter()
            .enumerate()
            .map(|(index, transfer)| self.render_transfer_row(index, transfer, cx));

        let table_footer = div()
            .h(px(38.0))
            .px_3()
            .flex()
            .items_center()
            .gap_4()
            .border_t_1()
            .border_color(theme::border())
            .text_xs()
            .text_color(theme::text_secondary())
            .child(self.tr("transfer-footer-total"))
            .child(self.tr("transfer-footer-downloading"))
            .child(self.tr("transfer-footer-waiting"))
            .child(div().flex_1())
            .child(div().text_color(theme::blue()).child("↓ 23.6 MB/s"))
            .child(div().text_color(theme::blue()).child("↑ 14.6 MB/s"))
            .child(self.tr("transfer-footer-unlimited"));

        let table = div()
            .flex_1()
            .min_h_0()
            .mx_4()
            .rounded(theme::RADIUS_MEDIUM)
            .border_1()
            .border_color(theme::border())
            .bg(theme::surface())
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(header)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .children(rows),
            )
            .child(table_footer);

        let bottom = div()
            .h(px(184.0))
            .px_4()
            .py_2()
            .flex()
            .gap_3()
            .child(self.render_activity_log())
            .child(self.render_connections());

        let main = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(summary)
            .child(toolbar)
            .child(table)
            .child(bottom);

        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .bg(theme::canvas())
            .child(main)
            .when_some(selected, |layout, selected| {
                layout.child(self.render_transfer_detail(selected, cx))
            })
            .into_any_element()
    }

    fn render_transfer_row(
        &self,
        index: usize,
        transfer: TransferRow,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.selected_file == index;
        let tone = transfer_tone(transfer.state);

        div()
            .id(("transfer-row", index))
            .h(px(38.0))
            .px_3()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(theme::border_subtle())
            .text_xs()
            .cursor_pointer()
            .focusable()
            .tab_index(0)
            .when(selected, |row| {
                row.bg(theme::blue_pale()).border_color(theme::blue_soft())
            })
            .hover(|row| row.bg(theme::blue_pale()))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.selected_file = index;
                cx.notify();
            }))
            .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    this.selected_file = index;
                    cx.notify();
                }
            }))
            .child(
                div().w(px(28.0)).child(
                    div()
                        .size(px(13.0))
                        .rounded(px(3.0))
                        .border_1()
                        .border_color(if selected {
                            theme::blue()
                        } else {
                            theme::border()
                        })
                        .when(selected, |box_| {
                            box_.bg(theme::blue())
                                .text_color(theme::surface())
                                .child("✓")
                        }),
                ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_weight(FontWeight::MEDIUM)
                    .child(transfer.name),
            )
            .child(transfer_cell(transfer.source, 108.0))
            .child(transfer_cell(transfer.size, 76.0))
            .child(
                div()
                    .w(px(112.0))
                    .pr_3()
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .text_color(tone.foreground())
                            .child(format!("{:.1}%", transfer.progress)),
                    )
                    .child(components::progress(transfer.progress, tone)),
            )
            .child(transfer_cell(transfer.speed, 82.0))
            .child(transfer_cell(transfer.eta, 64.0))
            .child(div().w(px(94.0)).child(components::badge(
                self.tr(transfer.state.message_id()),
                tone,
            )))
            .child(transfer_cell(transfer.destination, 118.0))
            .into_any_element()
    }

    fn render_activity_log(&self) -> AnyElement {
        let rows = activity_logs().into_iter().map(|log| {
            let message = self.tr(log.message_id);
            render_log_row(log, message)
        });
        components::card()
            .flex_1()
            .h_full()
            .overflow_hidden()
            .child(bottom_header(self.tr("transfer-tab-log")))
            .child(div().px_3().py_1().children(rows))
            .into_any_element()
    }

    fn render_connections(&self) -> AnyElement {
        let header = div()
            .h(px(30.0))
            .px_3()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(theme::border())
            .text_xs()
            .text_color(theme::text_muted())
            .child(connection_cell(self.tr("connection-address"), None))
            .child(connection_cell(self.tr("table-progress"), Some(70.0)))
            .child(connection_cell(self.tr("table-speed"), Some(70.0)))
            .child(connection_cell(self.tr("connection-client"), Some(56.0)))
            .child(connection_cell(self.tr("connection-latency"), Some(52.0)));
        let rows = connections().into_iter().map(render_connection_row);

        components::card()
            .flex_1()
            .h_full()
            .overflow_hidden()
            .child(bottom_header(self.tr("connection-title")))
            .child(header)
            .child(div().children(rows))
            .into_any_element()
    }

    fn render_transfer_detail(&self, transfer: TransferRow, cx: &mut Context<Self>) -> AnyElement {
        let tone = transfer_tone(transfer.state);
        let active = matches!(
            transfer.state,
            TransferState::Downloading | TransferState::Uploading
        );
        let waiting = transfer.state == TransferState::Waiting;
        let completed = transfer.state == TransferState::Completed;
        let failed = transfer.state == TransferState::Failed;
        let remote_message_id = if waiting { "—" } else { "1876543210897" };
        let started_at = if waiting {
            "—"
        } else {
            "2026-08-23 20:49:02"
        };
        let transfer_icon = match transfer.state {
            TransferState::Downloading => IconName::ArrowDown,
            TransferState::Uploading => IconName::ArrowUp,
            TransferState::Waiting => IconName::Calendar,
            TransferState::Completed => IconName::CircleCheck,
            TransferState::Failed => IconName::CircleX,
        };
        let details = [
            (
                self.tr("detail-source-channel"),
                SharedString::from(transfer.source),
            ),
            (
                self.tr("detail-message-id"),
                SharedString::from(remote_message_id),
            ),
            (
                self.tr("detail-local-path"),
                SharedString::from(transfer.destination),
            ),
            (self.tr("detail-speed"), SharedString::from(transfer.speed)),
            (
                self.tr("detail-transferred"),
                SharedString::from(format!("{} / {}", transfer.transferred, transfer.size)),
            ),
            (
                self.tr("detail-active-connections"),
                SharedString::from(transfer.connections),
            ),
            (
                self.tr("detail-workers"),
                SharedString::from(if active { "16" } else { "—" }),
            ),
            (
                self.tr("detail-retries"),
                SharedString::from(if failed { "3" } else { "0" }),
            ),
            (
                self.tr("detail-created"),
                SharedString::from("2026-08-23 20:48:31"),
            ),
            (self.tr("detail-started"), SharedString::from(started_at)),
        ];

        let (verification, verification_tone, verification_icon) = if completed {
            (
                self.tr("detail-verification-passed"),
                Tone::Green,
                IconName::CircleCheck,
            )
        } else if failed {
            (
                self.tr("detail-verification-failed"),
                Tone::Red,
                IconName::CircleX,
            )
        } else {
            (
                self.tr("detail-verification-pending"),
                Tone::Amber,
                IconName::LoaderCircle,
            )
        };

        let primary_action = match transfer.state {
            TransferState::Downloading | TransferState::Uploading => components::button(
                "detail-pause",
                if self.transfer_paused {
                    self.tr("action-resume")
                } else {
                    self.tr("action-pause")
                },
                Some(if self.transfer_paused {
                    IconName::ArrowRight
                } else {
                    IconName::Dash
                }),
                true,
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.transfer_paused = !this.transfer_paused;
                cx.notify();
            })),
            TransferState::Waiting => components::button(
                "detail-start",
                self.tr("action-start"),
                Some(IconName::ArrowRight),
                true,
            ),
            TransferState::Completed => components::button(
                "detail-open",
                self.tr("action-open-file"),
                Some(IconName::FolderOpen),
                true,
            ),
            TransferState::Failed => components::button(
                "detail-retry",
                self.tr("action-retry"),
                Some(IconName::Redo2),
                true,
            ),
        };

        div()
            .w(theme::DETAIL_WIDTH)
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .bg(theme::surface())
            .border_l_1()
            .border_color(theme::border())
            .child(
                div()
                    .p_5()
                    .border_b_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .flex()
                            .gap_3()
                            .items_start()
                            .child(
                                div()
                                    .size(px(42.0))
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(theme::RADIUS_SMALL)
                                    .bg(tone.background())
                                    .child(Icon::new(transfer_icon).text_color(tone.foreground())),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .child(
                                        div()
                                            .truncate()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(transfer.name),
                                    )
                                    .child(
                                        div()
                                            .mt_1()
                                            .text_xs()
                                            .text_color(theme::text_muted())
                                            .child(transfer.size),
                                    ),
                            )
                            .child(
                                Icon::new(IconName::EllipsisVertical)
                                    .text_color(theme::text_muted()),
                            ),
                    )
                    .child(
                        div()
                            .mt_5()
                            .flex()
                            .justify_between()
                            .text_xs()
                            .child(self.tr("table-status"))
                            .child(components::badge(
                                self.tr(transfer.state.message_id()),
                                tone,
                            )),
                    )
                    .child(
                        div()
                            .mt_4()
                            .flex()
                            .justify_between()
                            .text_sm()
                            .child(self.tr("table-progress"))
                            .child(format!("{:.1}%", transfer.progress)),
                    )
                    .child(
                        div()
                            .mt_2()
                            .child(components::progress(transfer.progress, tone)),
                    )
                    .child(
                        div()
                            .mt_4()
                            .flex()
                            .justify_between()
                            .text_xs()
                            .text_color(theme::text_secondary())
                            .child(self.tr("detail-time-remaining"))
                            .child(transfer.eta),
                    ),
            )
            .child(
                div()
                    .h(px(42.0))
                    .px_5()
                    .flex()
                    .items_end()
                    .gap_5()
                    .border_b_1()
                    .border_color(theme::border())
                    .child(detail_tab(self.tr("detail-tab-details"), true))
                    .child(detail_tab(self.tr("detail-tab-file-list"), false)),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .p_5()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .children(
                        details
                            .into_iter()
                            .map(|(label, value)| detail_row(label, value)),
                    )
                    .child(
                        div()
                            .mt_1()
                            .flex()
                            .justify_between()
                            .text_xs()
                            .child(self.tr("detail-verification"))
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .text_color(verification_tone.foreground())
                                    .child(
                                        Icon::new(verification_icon)
                                            .text_color(verification_tone.foreground()),
                                    )
                                    .child(verification),
                            ),
                    ),
            )
            .child(
                div()
                    .p_4()
                    .flex_none()
                    .border_t_1()
                    .border_color(theme::border())
                    .child(primary_action.w_full()),
            )
            .into_any_element()
    }
}

fn summary_card(
    icon: IconName,
    label: SharedString,
    value: &'static str,
    hint: SharedString,
    tone: Tone,
) -> AnyElement {
    components::card()
        .flex_1()
        .min_w(px(0.0))
        .h_full()
        .p_3()
        .flex()
        .items_start()
        .gap_3()
        .child(
            div()
                .size(px(34.0))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(theme::RADIUS_MEDIUM)
                .bg(tone.background())
                .child(Icon::new(icon).text_color(tone.foreground())),
        )
        .child(
            div()
                .min_w_0()
                .child(
                    div()
                        .truncate()
                        .text_xs()
                        .text_color(theme::text_secondary())
                        .child(label),
                )
                .child(
                    div()
                        .mt_1()
                        .text_lg()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(value),
                )
                .child(
                    div()
                        .mt_1()
                        .truncate()
                        .text_xs()
                        .text_color(theme::text_muted())
                        .child(hint),
                ),
        )
        .into_any_element()
}

fn transfer_header(label: impl Into<SharedString>, width: Option<f32>) -> AnyElement {
    div()
        .when_some(width, |cell, width| cell.w(px(width)))
        .when(width.is_none(), |cell| cell.flex_1())
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme::text_secondary())
        .child(label.into())
        .into_any_element()
}

fn transfer_cell(value: impl Into<SharedString>, width: f32) -> AnyElement {
    div()
        .w(px(width))
        .min_w_0()
        .truncate()
        .text_color(theme::text_secondary())
        .child(value.into())
        .into_any_element()
}

fn transfer_tone(state: TransferState) -> Tone {
    match state {
        TransferState::Downloading | TransferState::Uploading => Tone::Blue,
        TransferState::Waiting => Tone::Amber,
        TransferState::Completed => Tone::Green,
        TransferState::Failed => Tone::Red,
    }
}

fn bottom_header(label: impl Into<SharedString>) -> AnyElement {
    div()
        .h(px(32.0))
        .px_3()
        .flex()
        .items_center()
        .border_b_1()
        .border_color(theme::border())
        .font_weight(FontWeight::MEDIUM)
        .text_sm()
        .child(label.into())
        .into_any_element()
}

fn render_log_row(log: ActivityLog, message: SharedString) -> AnyElement {
    div()
        .h(px(20.0))
        .flex()
        .items_center()
        .gap_3()
        .text_xs()
        .text_color(if log.is_error {
            theme::red()
        } else {
            theme::text_secondary()
        })
        .child(
            div()
                .w(px(58.0))
                .text_color(theme::text_muted())
                .child(log.time),
        )
        .child(div().w(px(150.0)).truncate().child(log.subject))
        .child(div().flex_1().truncate().child(message))
        .into_any_element()
}

fn render_connection_row(connection: ConnectionRow) -> AnyElement {
    div()
        .h(px(20.0))
        .px_3()
        .flex()
        .items_center()
        .text_xs()
        .text_color(theme::text_secondary())
        .child(connection_cell(connection.address, None))
        .child(
            div()
                .w(px(70.0))
                .truncate()
                .text_color(theme::blue())
                .child(connection.progress),
        )
        .child(connection_cell(connection.speed, Some(70.0)))
        .child(connection_cell(connection.client, Some(56.0)))
        .child(connection_cell(connection.latency, Some(52.0)))
        .into_any_element()
}

fn connection_cell(value: impl Into<SharedString>, width: Option<f32>) -> AnyElement {
    div()
        .when_some(width, |cell, width| cell.w(px(width)))
        .when(width.is_none(), |cell| cell.flex_1().min_w_0())
        .truncate()
        .child(value.into())
        .into_any_element()
}

fn transfer_matches_nav(selection: &str, state: TransferState) -> bool {
    match selection {
        "nav-completed" => state == TransferState::Completed,
        "nav-failed" => state == TransferState::Failed,
        _ => true,
    }
}

fn detail_tab(label: impl Into<SharedString>, selected: bool) -> AnyElement {
    div()
        .h_full()
        .pb_2()
        .flex()
        .items_end()
        .text_sm()
        .text_color(if selected {
            theme::blue()
        } else {
            theme::text_secondary()
        })
        .when(selected, |tab| tab.border_b_2().border_color(theme::blue()))
        .child(label.into())
        .into_any_element()
}

fn detail_row(label: SharedString, value: SharedString) -> AnyElement {
    div()
        .flex()
        .gap_3()
        .text_xs()
        .child(
            div()
                .w(px(98.0))
                .flex_none()
                .text_color(theme::text_muted())
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(theme::text_secondary())
                .child(value),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transfer_sidebar_facets_match_only_the_requested_state() {
        assert!(transfer_matches_nav(
            "nav-completed",
            TransferState::Completed
        ));
        assert!(!transfer_matches_nav(
            "nav-completed",
            TransferState::Failed
        ));
        assert!(transfer_matches_nav("nav-failed", TransferState::Failed));
        assert!(transfer_matches_nav(
            "nav-transfers-all",
            TransferState::Waiting
        ));
    }
}
