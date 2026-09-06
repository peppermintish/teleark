use gpui::{
    AnyElement, AppContext as _, Context, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px,
};
use gpui_component::{
    Disableable as _, Icon, IconName, checkbox::Checkbox, scroll::ScrollableElement as _,
};
use teleark_i18n::{
    MessageArgs,
    format::{
        format_bytes, format_decimal, format_duration_millis, format_integer, format_percent,
        format_speed, format_unix_millis,
    },
};
use teleark_runtime::{
    ChannelDownloadEventKind, ChannelDownloadSnapshot, ChannelDownloadState,
    ChannelDownloadVerification, ControllerDecision, ControllerDecisionOutcome,
    ControllerDecisionReason, ControllerPhase, DownloadPartState, TransferBottleneck,
    TransferControlParameters, TransferTelemetrySnapshot, TunableParameter, VaultTransferDirection,
    VaultTransferSnapshot, VaultTransferState,
};

use crate::{
    app::TeleArkApp,
    components::{self, Tone},
    layout::LayoutPolicy,
    mock::{TransferDirection, TransferRow, TransferState, transfers},
    theme,
};

// The view owns one command batch. Dropping the owner stops at the next task
// boundary, without interrupting an atomic runtime operation already in progress.
pub(crate) struct TransferActionJob {
    _task: gpui::Task<()>,
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for TransferActionJob {
    fn drop(&mut self) {
        self.cancelled
            .store(true, std::sync::atomic::Ordering::Release);
    }
}

impl TeleArkApp {
    pub(crate) fn transfer_rows(&self) -> Vec<TransferRow> {
        let Some(transfers_runtime) = self.transfers.as_ref() else {
            return transfers(self.upload_queued);
        };
        let Ok(snapshots) = transfers_runtime.snapshots() else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        let mut rendered_batches = std::collections::BTreeSet::new();
        for snapshot in snapshots.iter().rev() {
            let Some(batch_id) = snapshot.batch_id else {
                rows.push(self.transfer_row_from_snapshot(snapshot, false));
                continue;
            };
            if !rendered_batches.insert(batch_id) {
                continue;
            }
            let mut items: Vec<_> = snapshots
                .iter()
                .filter(|item| item.batch_id == Some(batch_id))
                .collect();
            items.sort_by_key(|item| item.id);
            rows.push(self.transfer_row_from_batch(batch_id, &items));
            if self.expanded_transfer_batches.contains(&batch_id) {
                rows.extend(
                    items
                        .into_iter()
                        .rev()
                        .map(|item| self.transfer_row_from_snapshot(item, true)),
                );
            }
        }
        if let Some(vault) = self.vault.as_ref() {
            rows.splice(
                0..0,
                vault
                    .transfers()
                    .into_iter()
                    .rev()
                    .map(|snapshot| self.transfer_row_from_vault_snapshot(&snapshot)),
            );
        }
        if self.upload_queued
            && let Some(preview) = transfers(true).into_iter().find(|row| {
                row.direction == TransferDirection::Upload && row.state == TransferState::Waiting
            })
        {
            rows.insert(0, preview);
        }
        rows
    }

    fn transfer_row_from_vault_snapshot(&self, snapshot: &VaultTransferSnapshot) -> TransferRow {
        let state = match snapshot.state {
            VaultTransferState::Running => match snapshot.direction {
                VaultTransferDirection::Upload => TransferState::Uploading,
                VaultTransferDirection::Download => TransferState::Downloading,
            },
            VaultTransferState::Completed => TransferState::Completed,
            VaultTransferState::Failed(_) => TransferState::Failed,
        };
        let direction = match snapshot.direction {
            VaultTransferDirection::Upload => TransferDirection::Upload,
            VaultTransferDirection::Download => TransferDirection::Download,
        };
        let destination = match snapshot.direction {
            VaultTransferDirection::Upload => self.tr("transfer-vault-saved-messages"),
            VaultTransferDirection::Download => snapshot
                .destination
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned().into())
                .unwrap_or_else(|| self.tr("transfer-value-unavailable")),
        };
        TransferRow {
            runtime_task_id: None,
            vault_transfer_id: Some(snapshot.id),
            runtime_batch_id: None,
            batch_child: false,
            message_id: None,
            message_sent_at_unix_ms: None,
            caption: snapshot.package_id.clone().map(Into::into),
            mime_type: Some(self.tr("transfer-vault-encrypted-type")),
            name: snapshot.file_name.clone().into(),
            source: self.tr("saved-messages-title"),
            direction,
            size: format_bytes(self.locale(), snapshot.size_bytes).into(),
            transferred: format_bytes(self.locale(), snapshot.transferred_bytes).into(),
            progress: transfer_progress(
                snapshot.transferred_bytes,
                snapshot.size_bytes,
                state == TransferState::Completed,
            ),
            speed: snapshot
                .average_bytes_per_second
                .map(|speed| format_speed(self.locale(), speed).into())
                .unwrap_or_else(|| self.tr("transfer-value-unavailable")),
            eta: self.tr("transfer-value-unavailable"),
            connections: self.tr_with(
                "transfer-controller-connections-value",
                MessageArgs::new()
                    .with(
                        "connections",
                        format_integer(
                            self.locale(),
                            u64::from(snapshot.telemetry.parameters.transfer_connection_count),
                        ),
                    )
                    .with(
                        "rpcs",
                        format_integer(
                            self.locale(),
                            u64::from(snapshot.telemetry.parameters.inflight_rpcs_per_connection),
                        ),
                    ),
            ),
            state,
            destination,
        }
    }

    fn transfer_row_from_snapshot(
        &self,
        snapshot: &ChannelDownloadSnapshot,
        batch_child: bool,
    ) -> TransferRow {
        let state = transfer_state(snapshot.state);
        let speed = snapshot
            .current_bytes_per_second
            .or(snapshot.average_bytes_per_second)
            .map(|speed| format_speed(self.locale(), speed))
            .unwrap_or_else(|| self.tr("transfer-value-unavailable").to_string());
        let progress = transfer_progress(
            snapshot.transferred_bytes,
            snapshot.size_bytes,
            state == TransferState::Completed,
        );
        let source = self.telegram_source_name(snapshot.chat_id);
        TransferRow {
            runtime_task_id: Some(snapshot.id),
            vault_transfer_id: None,
            runtime_batch_id: snapshot.batch_id,
            batch_child,
            message_id: Some(snapshot.message_id),
            message_sent_at_unix_ms: snapshot.message_sent_at_unix_ms,
            caption: snapshot.caption.clone().map(Into::into),
            mime_type: snapshot.mime_type.clone().map(Into::into),
            name: snapshot.file_name.clone().into(),
            source: source.into(),
            direction: TransferDirection::Download,
            size: format_bytes(self.locale(), snapshot.size_bytes).into(),
            transferred: format_bytes(self.locale(), snapshot.transferred_bytes).into(),
            progress,
            speed: speed.into(),
            eta: snapshot
                .eta_ms
                .map(|eta| format_duration_millis(self.locale(), eta))
                .unwrap_or_else(|| self.tr("transfer-value-unavailable").to_string())
                .into(),
            connections: self.tr_with(
                "transfer-controller-connections-value",
                MessageArgs::new()
                    .with(
                        "connections",
                        format_integer(
                            self.locale(),
                            u64::from(snapshot.telemetry.parameters.transfer_connection_count),
                        ),
                    )
                    .with(
                        "rpcs",
                        format_integer(
                            self.locale(),
                            u64::from(snapshot.telemetry.parameters.inflight_rpcs_per_connection),
                        ),
                    ),
            ),
            state,
            destination: snapshot.destination.to_string_lossy().into_owned().into(),
        }
    }

    fn transfer_row_from_batch(
        &self,
        batch_id: u64,
        items: &[&ChannelDownloadSnapshot],
    ) -> TransferRow {
        let total_bytes = items
            .iter()
            .fold(0_u64, |total, item| total.saturating_add(item.size_bytes));
        let transferred_bytes = items.iter().fold(0_u64, |total, item| {
            total.saturating_add(item.transferred_bytes)
        });
        let current_speed = items.iter().fold(0_u64, |total, item| {
            total.saturating_add(item.current_bytes_per_second.unwrap_or(0))
        });
        let eta_ms = total_bytes
            .saturating_sub(transferred_bytes)
            .saturating_mul(1_000)
            .checked_div(current_speed);
        let state = aggregate_batch_state(items);
        let source = items
            .first()
            .map(|item| self.telegram_source_name(item.chat_id))
            .unwrap_or_default();
        let destination = items
            .first()
            .and_then(|item| item.destination.parent())
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default();
        TransferRow {
            runtime_task_id: None,
            vault_transfer_id: None,
            runtime_batch_id: Some(batch_id),
            batch_child: false,
            message_id: None,
            message_sent_at_unix_ms: None,
            caption: None,
            mime_type: None,
            name: self.tr_with(
                "transfer-batch-name",
                MessageArgs::new().with("count", format_integer(self.locale(), items.len() as u64)),
            ),
            source: source.into(),
            direction: TransferDirection::Download,
            size: format_bytes(self.locale(), total_bytes).into(),
            transferred: format_bytes(self.locale(), transferred_bytes).into(),
            progress: transfer_progress(
                transferred_bytes,
                total_bytes,
                state == TransferState::Completed,
            ),
            speed: if current_speed == 0 {
                self.tr("transfer-value-unavailable")
            } else {
                format_speed(self.locale(), current_speed).into()
            },
            eta: eta_ms
                .map(|eta| format_duration_millis(self.locale(), eta).into())
                .unwrap_or_else(|| self.tr("transfer-value-unavailable")),
            connections: self.tr("transfer-value-unavailable"),
            state,
            destination: destination.into(),
        }
    }

    fn telegram_source_name(&self, chat_id: i64) -> String {
        self.telegram_chats
            .iter()
            .find(|chat| chat.id == chat_id)
            .map(|chat| chat.name.clone())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| {
                self.tr_with(
                    "library-source-telegram-chat",
                    MessageArgs::new().with("chat_id", chat_id.to_string()),
                )
                .to_string()
            })
    }

    fn runtime_transfer_snapshot(&self, id: u64) -> Option<ChannelDownloadSnapshot> {
        self.transfers
            .as_ref()?
            .snapshots()
            .ok()?
            .into_iter()
            .find(|snapshot| snapshot.id == id)
    }

    fn vault_transfer_snapshot(&self, id: u64) -> Option<VaultTransferSnapshot> {
        self.vault
            .as_ref()?
            .transfers()
            .into_iter()
            .find(|snapshot| snapshot.id == id)
    }

    pub(crate) fn render_transfers(
        &self,
        _window: &mut Window,
        layout: LayoutPolicy,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let padding = layout.content_padding();
        let query = self.search_input.read(cx).value().to_lowercase();
        let all_transfer_rows = self.transfer_rows();
        let upload_count = all_transfer_rows
            .iter()
            .filter(|row| !row.batch_child && row.direction == TransferDirection::Upload)
            .count();
        let download_count = all_transfer_rows
            .iter()
            .filter(|row| !row.batch_child && row.direction == TransferDirection::Download)
            .count();
        let uploading = all_transfer_rows
            .iter()
            .filter(|transfer| !transfer.batch_child)
            .filter(|transfer| transfer.state == TransferState::Uploading)
            .count();
        let downloading = all_transfer_rows
            .iter()
            .filter(|transfer| !transfer.batch_child)
            .filter(|transfer| transfer.state == TransferState::Downloading)
            .count();
        let waiting = all_transfer_rows
            .iter()
            .filter(|transfer| !transfer.batch_child)
            .filter(|transfer| {
                matches!(
                    transfer.state,
                    TransferState::Waiting | TransferState::Paused
                )
            })
            .count();
        let completed = all_transfer_rows
            .iter()
            .filter(|transfer| !transfer.batch_child)
            .filter(|transfer| transfer.state == TransferState::Completed)
            .count();
        let failed = all_transfer_rows
            .iter()
            .filter(|transfer| !transfer.batch_child)
            .filter(|transfer| {
                matches!(
                    transfer.state,
                    TransferState::Failed | TransferState::Cancelled
                )
            })
            .count();
        let total_speed = self
            .transfers
            .as_ref()
            .and_then(|transfers| transfers.snapshots().ok())
            .into_iter()
            .flatten()
            .filter(|snapshot| snapshot.state == ChannelDownloadState::Running)
            .filter_map(|snapshot| snapshot.current_bytes_per_second)
            .fold(0_u64, u64::saturating_add);
        let total_speed = self.vault.as_ref().map_or(total_speed, |vault| {
            vault
                .transfers()
                .iter()
                .filter(|snapshot| snapshot.state == VaultTransferState::Running)
                .fold(total_speed, |total, snapshot| {
                    total.saturating_add(snapshot.telemetry.goodput_bytes_per_second)
                })
        });
        let transfer_rows: Vec<_> = all_transfer_rows
            .into_iter()
            .filter(|transfer| {
                transfer_matches_nav(self.nav_selection, transfer.state, transfer.direction)
            })
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
        let selected = transfer_rows
            .get(selected_index)
            .filter(|transfer| {
                transfer.runtime_task_id.is_some() || transfer.runtime_batch_id.is_none()
            })
            .cloned();
        let visible_transfer_keys: Vec<_> = transfer_rows
            .iter()
            .enumerate()
            .map(|(index, transfer)| transfer_selection_key(transfer, index))
            .collect();
        let all_visible_selected = !visible_transfer_keys.is_empty()
            && visible_transfer_keys
                .iter()
                .all(|key| self.selected_transfer_keys.contains(key));
        let runtime_snapshots = self
            .transfers
            .as_ref()
            .and_then(|transfers| transfers.snapshots().ok())
            .unwrap_or_default();
        let task_batches: Vec<_> = runtime_snapshots
            .iter()
            .map(|snapshot| (snapshot.id, snapshot.batch_id))
            .collect();
        let scoped_ids =
            transfer_scope_ids(&transfer_rows, &self.selected_transfer_keys, &task_batches);
        let selection_count = visible_transfer_keys
            .iter()
            .filter(|key| self.selected_transfer_keys.contains(key))
            .count();
        let summary = div()
            .flex_none()
            .px(px(padding))
            .py_3()
            .flex()
            .items_center()
            .gap_3()
            .child(components::section_title(self.tr("transfer-title")))
            .child(div().flex_1())
            .child(
                div()
                    .text_sm()
                    .text_color(theme::text_secondary())
                    .child(self.tr("transfer-summary-total-speed")),
            )
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::blue())
                    .child(format_speed(self.locale(), total_speed)),
            );
        let filters = div()
            .flex_none()
            .px(px(padding))
            .pb_2()
            .flex()
            .flex_wrap()
            .gap_2()
            .children(
                [
                    (
                        "nav-transfers-all",
                        "nav-all-transfers",
                        uploading + downloading + waiting + completed + failed,
                    ),
                    ("nav-uploads", "transfer-uploads", upload_count),
                    ("nav-downloads", "transfer-downloads", download_count),
                    ("nav-waiting", "transfer-summary-waiting", waiting),
                    ("nav-completed", "transfer-summary-completed", completed),
                    ("nav-failed", "transfer-summary-failed", failed),
                ]
                .into_iter()
                .map(|(id, label, count)| {
                    components::button(
                        id,
                        self.tr_with(
                            "transfer-filter-count",
                            MessageArgs::new()
                                .with("label", self.tr(label).to_string())
                                .with("count", format_integer(self.locale(), count as u64)),
                        ),
                        None,
                        self.nav_selection == id,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.nav_selection = id;
                        this.selected_file = 0;
                        this.selected_transfer_keys.clear();
                        this.pending_transfer_bulk_delete.clear();
                        this.show_transfer_detail = false;
                        cx.notify();
                    }))
                }),
            );
        let toolbar =
            div()
                .flex_none()
                .min_h(px(50.0))
                .px(px(padding))
                .py_2()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .child(div().text_xs().text_color(theme::text_secondary()).child(
                    if selection_count == 0 {
                        self.tr("transfer-scope-visible")
                    } else {
                        self.tr_with(
                            "transfer-footer-selected",
                            MessageArgs::new().with(
                                "count",
                                format_integer(self.locale(), selection_count as u64),
                            ),
                        )
                    },
                ))
                .children(
                    [
                        TransferAction::Resume,
                        TransferAction::Pause,
                        TransferAction::Retry,
                        TransferAction::Cancel,
                        TransferAction::Delete,
                    ]
                    .into_iter()
                    .map(|action| {
                        let ids: Vec<_> = runtime_snapshots
                            .iter()
                            .filter(|snapshot| {
                                scoped_ids.contains(&snapshot.id) && action.supports(snapshot.state)
                            })
                            .map(|snapshot| snapshot.id)
                            .collect();
                        components::button(
                            ("transfer-bulk", action as usize),
                            self.tr(action.label()),
                            Some(action.icon()),
                            false,
                        )
                        .disabled(ids.is_empty() || self.transfer_action_job.is_some())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if action == TransferAction::Delete {
                                this.pending_transfer_bulk_delete = ids.clone();
                            } else {
                                this.apply_transfer_action(action, &ids, cx);
                            }
                            cx.notify();
                        }))
                    }),
                )
                .when(selection_count > 0, |bar| {
                    bar.child(
                        components::button(
                            "transfer-clear-selection",
                            self.tr("telegram-files-clear-selection"),
                            None,
                            false,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.selected_transfer_keys.clear();
                            this.pending_transfer_bulk_delete.clear();
                            cx.notify();
                        })),
                    )
                })
                .when(self.transfer_action_job.is_some(), |bar| {
                    bar.child(
                        div()
                            .text_xs()
                            .text_color(theme::blue())
                            .child(self.tr("transfer-actions-applying")),
                    )
                })
                .when_some(self.transfer_action_error, |bar, error| {
                    bar.child(
                        div()
                            .w_full()
                            .text_xs()
                            .text_color(theme::red())
                            .child(self.tr(native_download_error_message_id(error))),
                    )
                })
                .when(
                    !self.pending_transfer_bulk_delete.is_empty()
                        || self.pending_transfer_delete.is_some(),
                    |bar| {
                        let ids = if let Some(id) = self.pending_transfer_delete {
                            vec![id]
                        } else {
                            self.pending_transfer_bulk_delete.clone()
                        };
                        bar.child(
                            div()
                                .w_full()
                                .p_3()
                                .rounded(theme::RADIUS_SMALL)
                                .bg(theme::amber_soft())
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap_2()
                                .child(div().flex_1().text_sm().child(self.tr_with(
                                    "transfer-delete-confirmation",
                                    MessageArgs::new().with(
                                        "count",
                                        format_integer(self.locale(), ids.len() as u64),
                                    ),
                                )))
                                .child(
                                    components::button(
                                        "transfer-confirm-delete",
                                        self.tr("action-confirm-delete-task"),
                                        None,
                                        false,
                                    )
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            this.apply_transfer_action(
                                                TransferAction::Delete,
                                                &ids,
                                                cx,
                                            );
                                            this.pending_transfer_bulk_delete.clear();
                                            this.pending_transfer_delete = None;
                                            cx.notify();
                                        },
                                    )),
                                )
                                .child(
                                    components::button(
                                        "transfer-dismiss-delete",
                                        self.tr("action-cancel"),
                                        None,
                                        false,
                                    )
                                    .on_click(cx.listener(
                                        |this, _, _, cx| {
                                            this.pending_transfer_bulk_delete.clear();
                                            this.pending_transfer_delete = None;
                                            cx.notify();
                                        },
                                    )),
                                ),
                        )
                    },
                );

        let header = div()
            .h(px(34.0))
            .px_3()
            .flex()
            .items_center()
            .bg(theme::sidebar())
            .border_y_1()
            .border_color(theme::border())
            .child(
                div().w(px(28.0)).flex_none().child(
                    Checkbox::new("transfers-select-all")
                        .checked(all_visible_selected)
                        .on_click(cx.listener({
                            let visible_transfer_keys = visible_transfer_keys.clone();
                            move |this, checked: &bool, _, cx| {
                                toggle_visible_selection(
                                    &mut this.selected_transfer_keys,
                                    &visible_transfer_keys,
                                    !*checked,
                                );
                                cx.notify();
                            }
                        })),
                ),
            )
            .child(transfer_header(self.tr("table-name"), None))
            .when(layout.shows_transfer_source(), |header| {
                header
                    .child(transfer_header(self.tr("table-source"), Some(108.0)))
                    .child(transfer_header(self.tr("detail-direction"), Some(76.0)))
            })
            .child(transfer_header(self.tr("table-size"), Some(76.0)))
            .child(transfer_header(self.tr("table-progress"), Some(112.0)))
            .when(layout.shows_transfer_speed(), |header| {
                header
                    .child(transfer_header(self.tr("table-speed"), Some(82.0)))
                    .when(!layout.is_compact(), |header| {
                        header.child(transfer_header(self.tr("table-eta"), Some(64.0)))
                    })
            })
            .child(transfer_header(
                self.tr("table-status"),
                Some(layout.transfer_status_width()),
            ))
            .child(transfer_header(self.tr("transfer-actions"), Some(152.0)));

        let has_rows = !transfer_rows.is_empty();
        let rows = transfer_rows
            .into_iter()
            .enumerate()
            .map(|(index, transfer)| self.render_transfer_row(index, transfer, layout, cx));

        let table_footer = div()
            .min_h(px(if layout.is_compact() { 58.0 } else { 38.0 }))
            .px_3()
            .py_2()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(if layout.is_compact() {
                px(8.0)
            } else {
                px(16.0)
            })
            .border_t_1()
            .border_color(theme::border())
            .text_xs()
            .text_color(theme::text_secondary())
            .child(self.tr_with(
                "transfer-footer-selected",
                MessageArgs::new().with(
                    "count",
                    format_integer(self.locale(), selection_count as u64),
                ),
            ))
            .child(self.tr_with(
                "transfer-footer-total-live",
                MessageArgs::new().with(
                    "count",
                    format_integer(
                        self.locale(),
                        (uploading + downloading + waiting + completed + failed) as u64,
                    ),
                ),
            ))
            .child(self.tr_with(
                "transfer-footer-downloading-live",
                MessageArgs::new().with("count", format_integer(self.locale(), downloading as u64)),
            ))
            .child(self.tr_with(
                "transfer-footer-waiting-live",
                MessageArgs::new().with("count", format_integer(self.locale(), waiting as u64)),
            ))
            .when(!layout.is_compact(), |footer| footer.child(div().flex_1()));

        let table = div()
            .flex_1()
            .min_h_0()
            .mx(px(padding))
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
                    .children(rows)
                    .when(!has_rows, |body| {
                        body.child(
                            div()
                                .h_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_sm()
                                .text_color(theme::text_secondary())
                                .child(self.tr("transfer-empty")),
                        )
                    }),
            )
            .child(table_footer);

        let main = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(summary)
            .child(filters)
            .child(toolbar)
            .child(table)
            .pb(px(padding));

        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .relative()
            .bg(theme::canvas())
            .child(main)
            .when_some(
                selected.filter(|_| self.show_transfer_detail),
                |page, selected| {
                    page.child(
                        div()
                            .absolute()
                            .right_0()
                            .top_0()
                            .bottom_0()
                            .shadow_lg()
                            .child(self.render_transfer_detail(selected, layout, cx)),
                    )
                },
            )
            .into_any_element()
    }

    fn apply_transfer_action(
        &mut self,
        action: TransferAction,
        ids: &[u64],
        cx: &mut Context<Self>,
    ) {
        if self.transfer_action_job.is_some() || ids.is_empty() {
            return;
        }
        let Some(transfers) = self.transfers.clone() else {
            return;
        };
        self.transfer_action_error = None;
        let ids = ids.to_vec();
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cancellation = cancelled.clone();
        let work = cx.background_spawn(async move {
            execute_transfer_actions(action, &ids, &cancellation, |id| match action {
                TransferAction::Pause => transfers.pause(id),
                TransferAction::Resume => transfers.resume(id),
                TransferAction::Retry => transfers.retry(id),
                TransferAction::Cancel => transfers.cancel(id),
                TransferAction::Delete => transfers.delete(id),
            })
        });
        let task = cx.spawn(async move |this, cx| {
            let (deleted, failure) = work.await;
            let Some(this) = this.upgrade() else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                if !deleted.is_empty() {
                    this.show_transfer_detail = false;
                }
                for id in deleted {
                    this.selected_transfer_keys.remove(&id);
                }
                this.transfer_action_error = failure;
                this.transfer_action_job = None;
                cx.notify();
            });
        });
        self.transfer_action_job = Some(TransferActionJob {
            _task: task,
            cancelled,
        });
        cx.notify();
    }

    fn render_transfer_actions(
        &self,
        transfer: &TransferRow,
        index: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut actions = div()
            .w(px(152.0))
            .flex_none()
            .flex()
            .justify_end()
            .items_center()
            .gap_1();
        if let Some(id) = transfer.runtime_task_id {
            for action in [
                TransferAction::Pause,
                TransferAction::Resume,
                TransferAction::Retry,
                TransferAction::Cancel,
                TransferAction::Delete,
            ] {
                if action.supports_view(transfer.state) {
                    actions = actions.child(
                        components::icon_button(
                            (action.element_id(), id),
                            action.icon(),
                            self.tr(action.label()),
                        )
                        .disabled(self.transfer_action_job.is_some())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            if action == TransferAction::Delete {
                                this.pending_transfer_delete = Some(id);
                                this.pending_transfer_bulk_delete.clear();
                            } else {
                                this.apply_transfer_action(action, &[id], cx);
                            }
                            cx.notify();
                        })),
                    );
                }
            }
        }
        if self.transfers.is_none() && transfer.runtime_task_id.is_none() {
            for action in [
                TransferAction::Pause,
                TransferAction::Resume,
                TransferAction::Retry,
                TransferAction::Cancel,
                TransferAction::Delete,
            ] {
                if action.supports_view(transfer.state) {
                    actions = actions.child(
                        components::icon_button(
                            (action.element_id(), index),
                            action.icon(),
                            self.tr(action.label()),
                        )
                        .disabled(true),
                    );
                }
            }
        }
        if transfer.state == TransferState::Completed
            && transfer.direction == TransferDirection::Download
            && (transfer.runtime_task_id.is_some() || transfer.vault_transfer_id.is_some())
        {
            let destination = std::path::PathBuf::from(transfer.destination.as_ref());
            let reveal = destination.clone();
            actions = actions
                .child(
                    components::icon_button(
                        ("transfer-open", index),
                        IconName::ArrowRight,
                        self.tr("action-open-file"),
                    )
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        cx.open_with_system(&destination);
                    }),
                )
                .child(
                    components::icon_button(
                        ("transfer-reveal", index),
                        IconName::FolderOpen,
                        self.tr("action-show-in-folder"),
                    )
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        cx.reveal_path(&reveal);
                    }),
                );
        }
        if transfer.runtime_task_id.is_some() || transfer.runtime_batch_id.is_none() {
            actions = actions.child(
                components::icon_button(
                    ("transfer-details", index),
                    IconName::Info,
                    self.tr("transfer-show-details"),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.selected_file = index;
                    this.show_transfer_detail = true;
                    cx.notify();
                })),
            );
        }
        actions.into_any_element()
    }

    fn render_transfer_row(
        &self,
        index: usize,
        transfer: TransferRow,
        layout: LayoutPolicy,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let focused = self.selected_file == index;
        let selection_key = transfer_selection_key(&transfer, index);
        let selected = self.selected_transfer_keys.contains(&selection_key);
        let tone = transfer_tone(transfer.state);
        let actions = self.render_transfer_actions(&transfer, index, cx);
        let batch_group_id = transfer
            .runtime_batch_id
            .filter(|_| transfer.runtime_task_id.is_none() && !transfer.batch_child);
        let batch_expanded =
            batch_group_id.is_some_and(|id| self.expanded_transfer_batches.contains(&id));

        div()
            .id(("transfer-row", index))
            .h(px(52.0))
            .px_3()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(theme::border_subtle())
            .text_xs()
            .cursor_pointer()
            .focusable()
            .tab_index(0)
            .when(focused || selected, |row| {
                row.bg(theme::blue_pale()).border_color(theme::blue_soft())
            })
            .hover(|row| row.bg(theme::blue_pale()))
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(batch_id) = batch_group_id {
                    if !this.expanded_transfer_batches.insert(batch_id) {
                        this.expanded_transfer_batches.remove(&batch_id);
                    }
                } else {
                    this.selected_file = index;
                    this.pending_transfer_delete = None;
                }
                cx.notify();
            }))
            .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    if let Some(batch_id) = batch_group_id {
                        if !this.expanded_transfer_batches.insert(batch_id) {
                            this.expanded_transfer_batches.remove(&batch_id);
                        }
                    } else {
                        this.selected_file = index;
                        this.pending_transfer_delete = None;
                    }
                    cx.notify();
                }
            }))
            .child(
                div().w(px(28.0)).flex_none().child(
                    Checkbox::new(("transfer-select", selection_key))
                        .checked(selected)
                        .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                            cx.stop_propagation();
                            if *checked {
                                this.selected_transfer_keys.insert(selection_key);
                            } else {
                                this.selected_transfer_keys.remove(&selection_key);
                            }
                            cx.notify();
                        })),
                ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_2()
                    .when(transfer.batch_child, |name| name.pl_4())
                    .font_weight(FontWeight::MEDIUM)
                    .when(batch_group_id.is_some(), |name| {
                        name.child(Icon::new(if batch_expanded {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        }))
                    })
                    .child(
                        Icon::new(if transfer.direction == TransferDirection::Upload {
                            IconName::ArrowUp
                        } else {
                            IconName::ArrowDown
                        })
                        .size(px(14.0))
                        .text_color(
                            if transfer.direction == TransferDirection::Upload {
                                Tone::Purple.foreground()
                            } else {
                                theme::blue()
                            },
                        ),
                    )
                    .child(div().min_w_0().truncate().child(transfer.name)),
            )
            .when(layout.shows_transfer_source(), |row| {
                row.child(transfer_cell(transfer.source, 108.0))
                    .child(transfer_cell(
                        self.tr(match transfer.direction {
                            TransferDirection::Upload => "detail-direction-upload",
                            TransferDirection::Download => "detail-direction-download",
                        }),
                        76.0,
                    ))
            })
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
                            .child(format_percent(
                                self.locale(),
                                f64::from(transfer.progress) / 100.0,
                                1,
                            )),
                    )
                    .child(components::progress(transfer.progress, tone)),
            )
            .when(layout.shows_transfer_speed(), |row| {
                row.child(transfer_cell(transfer.speed, 82.0))
                    .when(!layout.is_compact(), |row| {
                        row.child(transfer_cell(transfer.eta, 64.0))
                    })
            })
            .child(
                div()
                    .w(px(layout.transfer_status_width()))
                    .child(components::badge(
                        self.tr(transfer.state.message_id()),
                        tone,
                    )),
            )
            .child(actions)
            .into_any_element()
    }

    fn render_transfer_telemetry(
        &self,
        telemetry: TransferTelemetrySnapshot,
        transfer_is_active: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let replay_mode = self.transfer_inspector_replay || !transfer_is_active;
        let decision_count = telemetry.decisions.len();
        let replay_cursor = self
            .transfer_replay_cursor
            .min(decision_count.saturating_sub(1));
        let decisions: Vec<_> = if replay_mode {
            telemetry
                .decisions
                .get(replay_cursor)
                .copied()
                .into_iter()
                .collect()
        } else {
            telemetry
                .decisions
                .iter()
                .rev()
                .take(8)
                .rev()
                .copied()
                .collect()
        };
        let decision_rows = decisions
            .into_iter()
            .map(|decision| self.render_controller_decision(decision));
        let live_button = components::button(
            "transfer-telemetry-live",
            self.tr("transfer-mode-live"),
            Some(IconName::ChartPie),
            !replay_mode,
        )
        .disabled(!transfer_is_active)
        .on_click(cx.listener(|this, _, _, cx| {
            this.transfer_inspector_replay = false;
            cx.notify();
        }));
        let replay_button = components::button(
            "transfer-telemetry-replay",
            self.tr("transfer-mode-replay"),
            Some(IconName::Redo),
            replay_mode,
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            this.transfer_inspector_replay = true;
            this.transfer_replay_cursor = decision_count.saturating_sub(1);
            cx.notify();
        }));
        let previous_button = components::button(
            "transfer-replay-previous",
            self.tr("transfer-replay-previous"),
            Some(IconName::ChevronLeft),
            false,
        )
        .disabled(!replay_mode || replay_cursor == 0 || decision_count == 0)
        .on_click(cx.listener(|this, _, _, cx| {
            this.transfer_replay_cursor = this.transfer_replay_cursor.saturating_sub(1);
            cx.notify();
        }));
        let next_button = components::button(
            "transfer-replay-next",
            self.tr("transfer-replay-next"),
            Some(IconName::ChevronRight),
            false,
        )
        .disabled(!replay_mode || replay_cursor.saturating_add(1) >= decision_count)
        .on_click(cx.listener(move |this, _, _, cx| {
            this.transfer_replay_cursor = this
                .transfer_replay_cursor
                .saturating_add(1)
                .min(decision_count.saturating_sub(1));
            cx.notify();
        }));
        let lane_rows: Vec<AnyElement> = telemetry
            .lanes
            .iter()
            .map(|lane| {
                let status = if lane.paused_until_millis.is_some() {
                    self.tr("transfer-lane-paused")
                } else {
                    self.tr("transfer-lane-active")
                };
                div()
                    .py_1()
                    .text_xs()
                    .text_color(theme::text_secondary())
                    .child(
                        self.tr_with(
                            "transfer-lane-value",
                            MessageArgs::new()
                                .with(
                                    "dc",
                                    format_integer(
                                        self.locale(),
                                        lane.data_center_id.max(0) as u64,
                                    ),
                                )
                                .with(
                                    "lane",
                                    format_integer(self.locale(), u64::from(lane.lane_id)),
                                )
                                .with(
                                    "inflight",
                                    format_integer(
                                        self.locale(),
                                        u64::from(lane.inflight_rpc_count),
                                    ),
                                )
                                .with(
                                    "speed",
                                    format_speed(self.locale(), lane.throughput_bytes_per_second),
                                )
                                .with(
                                    "rtt",
                                    format_duration_millis(
                                        self.locale(),
                                        lane.round_trip_time_p95_millis,
                                    ),
                                )
                                .with("status", status.to_string()),
                        ),
                    )
                    .into_any_element()
            })
            .collect();

        components::card()
            .mt_2()
            .p_3()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(self.tr("transfer-controller-title")),
                    )
                    .child(div().flex().gap_2().child(live_button).child(replay_button)),
            )
            .child(
                div().text_xs().text_color(theme::text_secondary()).child(
                    self.tr_with(
                        "transfer-controller-queue-waits-value",
                        MessageArgs::new()
                            .with(
                                "network",
                                format_duration_millis(
                                    self.locale(),
                                    telemetry.network_waiting_for_encryption_millis,
                                ),
                            )
                            .with(
                                "encryption",
                                format_duration_millis(
                                    self.locale(),
                                    telemetry.encryption_waiting_for_network_millis,
                                ),
                            )
                            .with(
                                "parts",
                                format_decimal(
                                    self.locale(),
                                    telemetry.parts.completed_parts_per_second_milli as f64
                                        / 1_000.0,
                                    2,
                                ),
                            ),
                    ),
                ),
            )
            .child(
                div().text_xs().text_color(theme::text_secondary()).child(
                    self.tr_with(
                        "transfer-controller-buffers-value",
                        MessageArgs::new()
                            .with(
                                "plaintext",
                                format_bytes(
                                    self.locale(),
                                    telemetry.memory.plaintext_buffer_bytes,
                                ),
                            )
                            .with(
                                "encrypted",
                                format_bytes(
                                    self.locale(),
                                    telemetry.memory.encrypted_buffer_bytes,
                                ),
                            )
                            .with(
                                "network",
                                format_bytes(
                                    self.locale(),
                                    telemetry.memory.network_inflight_bytes,
                                ),
                            )
                            .with(
                                "writer",
                                format_bytes(self.locale(), telemetry.memory.writer_queue_bytes),
                            ),
                    ),
                ),
            )
            .child(
                div()
                    .pt_2()
                    .border_t_1()
                    .border_color(theme::border())
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.tr("transfer-controller-decisions")),
            )
            .when(decision_count == 0, |card| {
                card.child(
                    div()
                        .text_xs()
                        .text_color(theme::text_muted())
                        .child(self.tr("transfer-controller-no-decisions")),
                )
            })
            .children(decision_rows)
            .when(replay_mode && decision_count != 0, |card| {
                card.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(previous_button)
                        .child(
                            div().text_xs().text_color(theme::text_muted()).child(
                                self.tr_with(
                                    "transfer-replay-position",
                                    MessageArgs::new()
                                        .with(
                                            "current",
                                            format_integer(
                                                self.locale(),
                                                replay_cursor.saturating_add(1) as u64,
                                            ),
                                        )
                                        .with(
                                            "total",
                                            format_integer(self.locale(), decision_count as u64),
                                        ),
                                ),
                            ),
                        )
                        .child(next_button),
                )
            })
            .child(
                div()
                    .pt_2()
                    .border_t_1()
                    .border_color(theme::border())
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.tr("transfer-controller-lanes")),
            )
            .when(lane_rows.is_empty(), |card| {
                card.child(
                    div()
                        .text_xs()
                        .text_color(theme::text_muted())
                        .child(self.tr("transfer-controller-lanes-unavailable")),
                )
            })
            .children(lane_rows)
            .into_any_element()
    }

    fn render_controller_decision(&self, decision: ControllerDecision) -> AnyElement {
        let parameter = decision.parameter.map_or_else(
            || self.tr("transfer-controller-no-parameter"),
            |parameter| self.tr(parameter_message_id(parameter)),
        );
        let before_value = decision
            .parameter
            .map(|parameter| control_parameter_value(decision.before, parameter));
        let after_value = decision
            .parameter
            .map(|parameter| control_parameter_value(decision.after, parameter));
        let change = match (before_value, after_value) {
            (Some(before), Some(after)) => format!("{parameter}: {before} → {after}"),
            _ => parameter.to_string(),
        };
        let outcome = decision_outcome_message_id(decision.outcome);
        let tone = match decision.outcome {
            ControllerDecisionOutcome::Keep
            | ControllerDecisionOutcome::OverrideSoftLimit
            | ControllerDecisionOutcome::ResumeLane => Tone::Green,
            ControllerDecisionOutcome::Rollback | ControllerDecisionOutcome::PauseLane => Tone::Red,
            ControllerDecisionOutcome::Confirm
            | ControllerDecisionOutcome::Recover
            | ControllerDecisionOutcome::RespectSoftLimit => Tone::Amber,
            _ => Tone::Blue,
        };
        div()
            .p_2()
            .rounded(theme::RADIUS_SMALL)
            .bg(theme::canvas())
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_xs()
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(format!(
                        "{} · {}",
                        self.tr(controller_phase_message_id(decision.phase)),
                        change
                    )))
                    .child(components::badge(self.tr(outcome), tone)),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme::text_secondary())
                    .child(self.tr(decision_reason_message_id(decision.reason))),
            )
            .child(
                div().text_xs().text_color(theme::text_muted()).child(
                    self.tr_with(
                        "transfer-decision-throughput-value",
                        MessageArgs::new()
                            .with(
                                "before",
                                format_speed(
                                    self.locale(),
                                    decision.baseline_goodput_bytes_per_second,
                                ),
                            )
                            .with(
                                "after",
                                format_speed(
                                    self.locale(),
                                    decision.observed_goodput_bytes_per_second,
                                ),
                            )
                            .with(
                                "change",
                                format_percent(
                                    self.locale(),
                                    f64::from(decision.goodput_change_basis_points) / 10_000.0,
                                    1,
                                ),
                            )
                            .with(
                                "elapsed",
                                format_duration_millis(self.locale(), decision.observed_at_millis),
                            ),
                    ),
                ),
            )
            .into_any_element()
    }

    fn render_transfer_detail(
        &self,
        transfer: TransferRow,
        layout: LayoutPolicy,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tone = transfer_tone(transfer.state);
        let active = matches!(
            transfer.state,
            TransferState::Downloading | TransferState::Uploading | TransferState::Paused
        );
        let waiting = transfer.state == TransferState::Waiting;
        let completed = transfer.state == TransferState::Completed;
        let failed = transfer.state == TransferState::Failed;
        let runtime_backed = transfer.runtime_task_id.is_some();
        let vault_backed = transfer.vault_transfer_id.is_some();
        let upload = transfer.direction == TransferDirection::Upload;
        let preview_upload = upload && !runtime_backed && !vault_backed;
        let runtime_snapshot = transfer
            .runtime_task_id
            .and_then(|id| self.runtime_transfer_snapshot(id));
        let vault_snapshot = transfer
            .vault_transfer_id
            .and_then(|id| self.vault_transfer_snapshot(id));
        let telemetry = vault_snapshot
            .as_ref()
            .map(|snapshot| snapshot.telemetry.clone())
            .or_else(|| {
                runtime_snapshot
                    .as_ref()
                    .map(|snapshot| snapshot.telemetry.clone())
            });
        let unavailable = self.tr("transfer-value-unavailable");
        let remote_message_id = if let Some(message_id) = transfer.message_id {
            message_id.to_string().into()
        } else if waiting || vault_backed {
            unavailable.clone()
        } else {
            SharedString::from("1876543210897")
        };
        let created_at = vault_snapshot.as_ref().map_or_else(
            || {
                runtime_snapshot.as_ref().map_or_else(
                    || {
                        if runtime_backed {
                            unavailable.clone()
                        } else {
                            SharedString::from("2026-08-23 20:48:31")
                        }
                    },
                    |snapshot| format_unix_millis(self.locale(), snapshot.queued_at_unix_ms).into(),
                )
            },
            |snapshot| format_unix_millis(self.locale(), snapshot.started_at_unix_ms).into(),
        );
        let started_at = vault_snapshot.as_ref().map_or_else(
            || {
                runtime_snapshot.as_ref().map_or_else(
                    || {
                        if runtime_backed || waiting {
                            unavailable.clone()
                        } else {
                            SharedString::from("2026-08-23 20:49:02")
                        }
                    },
                    |snapshot| {
                        snapshot
                            .started_at_unix_ms
                            .map(|timestamp| format_unix_millis(self.locale(), timestamp).into())
                            .unwrap_or_else(|| unavailable.clone())
                    },
                )
            },
            |snapshot| format_unix_millis(self.locale(), snapshot.started_at_unix_ms).into(),
        );
        let finished_at = runtime_snapshot.as_ref().and_then(|snapshot| {
            snapshot
                .finished_at_unix_ms
                .map(|timestamp| format_unix_millis(self.locale(), timestamp).into())
        });
        let elapsed_ms = runtime_snapshot
            .as_ref()
            .and_then(|snapshot| {
                snapshot.duration_ms.or_else(|| {
                    snapshot.started_at_unix_ms.and_then(|started| {
                        current_unix_millis()
                            .and_then(|now| now.checked_sub(started))
                            .and_then(|elapsed| u64::try_from(elapsed).ok())
                    })
                })
            })
            .or_else(|| {
                vault_snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.duration_ms)
            });
        let transfer_icon = match transfer.state {
            TransferState::Downloading => IconName::ArrowDown,
            TransferState::Uploading => IconName::ArrowUp,
            TransferState::Waiting => IconName::Calendar,
            TransferState::Paused => IconName::Dash,
            TransferState::Completed => IconName::CircleCheck,
            TransferState::Failed => IconName::CircleX,
            TransferState::Cancelled => IconName::CircleX,
        };
        let mut details = vec![
            (
                self.tr("detail-direction"),
                self.tr(if upload {
                    "detail-direction-upload"
                } else {
                    "detail-direction-download"
                }),
            ),
            (self.tr("detail-source-channel"), transfer.source.clone()),
            (self.tr("detail-message-id"), remote_message_id),
            (self.tr("detail-local-path"), transfer.destination.clone()),
            (self.tr("detail-speed"), transfer.speed.clone()),
            (
                self.tr("detail-transferred"),
                SharedString::from(format!("{} / {}", transfer.transferred, transfer.size)),
            ),
            (
                self.tr("detail-active-connections"),
                transfer.connections.clone(),
            ),
            (
                self.tr("detail-workers"),
                vault_snapshot.as_ref().map_or_else(
                    || {
                        runtime_snapshot.as_ref().map_or_else(
                            || {
                                SharedString::from(if runtime_backed {
                                    "—"
                                } else if active {
                                    "16"
                                } else {
                                    "—"
                                })
                            },
                            |snapshot| {
                                if snapshot.started_at_unix_ms.is_some() {
                                    format_integer(self.locale(), 1).into()
                                } else {
                                    unavailable.clone()
                                }
                            },
                        )
                    },
                    |_| format_integer(self.locale(), 1).into(),
                ),
            ),
            (
                self.tr("detail-retries"),
                vault_snapshot.as_ref().map_or_else(
                    || {
                        runtime_snapshot.as_ref().map_or_else(
                            || {
                                SharedString::from(if runtime_backed {
                                    "—"
                                } else if failed {
                                    "3"
                                } else {
                                    "0"
                                })
                            },
                            |snapshot| {
                                format_integer(
                                    self.locale(),
                                    u64::from(snapshot.attempts.saturating_sub(1)),
                                )
                                .into()
                            },
                        )
                    },
                    |_| unavailable.clone(),
                ),
            ),
            (self.tr("detail-created"), created_at),
            (self.tr("detail-started"), started_at),
        ];
        details.push((
            self.tr("detail-storage-format"),
            self.tr(if upload || vault_backed {
                "detail-storage-format-teleark"
            } else {
                "detail-storage-format-native"
            }),
        ));
        details.push((
            self.tr("detail-content-protection"),
            self.tr(
                if vault_backed || (upload && self.preferences.upload_encrypt_content) {
                    "detail-content-protection-aes"
                } else {
                    "detail-content-protection-none"
                },
            ),
        ));
        details.push((
            self.tr("detail-integrity-codec"),
            self.tr(if upload || vault_backed {
                "detail-integrity-teleark"
            } else {
                "detail-integrity-native"
            }),
        ));
        if upload || vault_backed {
            details.push((
                self.tr("upload-part-size"),
                if vault_backed {
                    format_bytes(
                        self.locale(),
                        teleark_runtime::encrypted_part_plaintext_limit(),
                    )
                    .into()
                } else {
                    self.tr_with(
                        "upload-part-size-mib",
                        MessageArgs::new().with(
                            "size",
                            format_integer(
                                self.locale(),
                                u64::from(self.preferences.upload_part_size_mib),
                            ),
                        ),
                    )
                },
            ));
            details.push((
                self.tr("detail-manifest-codec"),
                self.tr("detail-manifest-codec-value"),
            ));
        }
        if let Some(sent_at) = transfer.message_sent_at_unix_ms {
            details.push((
                self.tr("telegram-message-sent-at"),
                format_unix_millis(self.locale(), sent_at).into(),
            ));
        }
        if let Some(mime_type) = transfer.mime_type.clone() {
            details.push((self.tr("telegram-message-mime-type"), mime_type));
        }
        details.push((
            self.tr("telegram-message-caption"),
            transfer
                .caption
                .clone()
                .unwrap_or_else(|| self.tr("telegram-message-no-caption")),
        ));
        if let Some(snapshot) = runtime_snapshot.as_ref() {
            details.push((
                self.tr("detail-trace-id"),
                SharedString::from(format!("DL-{}", snapshot.id)),
            ));
            details.push((
                self.tr("detail-queue-wait"),
                snapshot
                    .queue_wait_ms
                    .map(|duration| format_duration_millis(self.locale(), duration).into())
                    .unwrap_or_else(|| unavailable.clone()),
            ));
            details.push((
                self.tr("detail-elapsed"),
                elapsed_ms
                    .map(|duration| format_duration_millis(self.locale(), duration).into())
                    .unwrap_or_else(|| unavailable.clone()),
            ));
            details.push((
                self.tr("detail-average-speed"),
                snapshot
                    .average_bytes_per_second
                    .map(|speed| format_speed(self.locale(), speed).into())
                    .unwrap_or_else(|| unavailable.clone()),
            ));
            if let Some(finished_at) = finished_at {
                details.push((self.tr("detail-finished"), finished_at));
            }
        }
        if let Some(snapshot) = vault_snapshot.as_ref() {
            details.push((
                self.tr("detail-trace-id"),
                SharedString::from(format!("VAULT-{}", snapshot.id)),
            ));
            details.push((
                self.tr("detail-elapsed"),
                elapsed_ms
                    .map(|duration| format_duration_millis(self.locale(), duration).into())
                    .unwrap_or_else(|| unavailable.clone()),
            ));
            details.push((
                self.tr("detail-average-speed"),
                snapshot
                    .average_bytes_per_second
                    .map(|speed| format_speed(self.locale(), speed).into())
                    .unwrap_or_else(|| unavailable.clone()),
            ));
            details.push((
                self.tr("detail-vault-lifecycle"),
                self.tr("detail-vault-lifecycle-memory-only"),
            ));
        }
        if let Some(telemetry) = telemetry.as_ref() {
            let parameters = telemetry.parameters;
            details.extend([
                (
                    self.tr("transfer-controller-phase"),
                    self.tr(controller_phase_message_id(telemetry.phase)),
                ),
                (
                    self.tr("transfer-controller-parameters"),
                    format_control_parameters(parameters).into(),
                ),
                (
                    self.tr("transfer-controller-goodput"),
                    format_speed(self.locale(), telemetry.goodput_bytes_per_second).into(),
                ),
                (
                    self.tr("transfer-controller-encryption-throughput"),
                    format_speed(self.locale(), telemetry.encryption_bytes_per_second).into(),
                ),
                (
                    self.tr("transfer-controller-disk-throughput"),
                    format_speed(self.locale(), telemetry.disk_bytes_per_second).into(),
                ),
                (
                    self.tr("transfer-controller-bdp"),
                    format_bytes(self.locale(), telemetry.estimated_bdp_bytes).into(),
                ),
                (
                    self.tr("transfer-controller-rtt"),
                    format_duration_millis(self.locale(), telemetry.round_trip_time_p95_millis)
                        .into(),
                ),
                (
                    self.tr("transfer-controller-inflight"),
                    self.tr_with(
                        "transfer-controller-inflight-value",
                        MessageArgs::new()
                            .with(
                                "current",
                                format_bytes(self.locale(), telemetry.inflight_bytes),
                            )
                            .with(
                                "target",
                                format_bytes(self.locale(), telemetry.target_inflight_bytes),
                            ),
                    ),
                ),
                (
                    self.tr("transfer-controller-cpu"),
                    format_percent(
                        self.locale(),
                        f64::from(telemetry.cpu_utilization_basis_points) / 10_000.0,
                        1,
                    )
                    .into(),
                ),
                (
                    self.tr("transfer-controller-bottleneck"),
                    self.tr(bottleneck_message_id(telemetry.bottleneck)),
                ),
                (
                    self.tr("transfer-controller-memory"),
                    self.tr_with(
                        "transfer-controller-memory-value",
                        MessageArgs::new()
                            .with(
                                "used",
                                format_bytes(self.locale(), telemetry.memory.total_bytes()),
                            )
                            .with(
                                "budget",
                                format_bytes(self.locale(), telemetry.memory_budget_bytes),
                            ),
                    ),
                ),
                (
                    self.tr("transfer-controller-part-map"),
                    self.tr_with(
                        "transfer-controller-part-map-value",
                        MessageArgs::new()
                            .with(
                                "completed",
                                format_integer(self.locale(), telemetry.parts.completed_parts),
                            )
                            .with(
                                "inflight",
                                format_integer(self.locale(), telemetry.parts.inflight_parts),
                            )
                            .with(
                                "retry",
                                format_integer(self.locale(), telemetry.parts.retry_parts),
                            )
                            .with(
                                "failed",
                                format_integer(self.locale(), telemetry.parts.failed_parts),
                            )
                            .with(
                                "missing",
                                format_integer(self.locale(), telemetry.parts.missing_parts),
                            ),
                    ),
                ),
            ]);
        }
        if let Some(path) = vault_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.session_log_path.as_ref())
            .or_else(|| {
                runtime_snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.session_log_path.as_ref())
            })
        {
            details.push((
                self.tr("transfer-session-log"),
                path.to_string_lossy().into_owned().into(),
            ));
        }

        let (verification, verification_tone, verification_icon) =
            if let Some(snapshot) = runtime_snapshot.as_ref() {
                match snapshot.verification {
                    ChannelDownloadVerification::Pending => (
                        self.tr("detail-verification-pending"),
                        Tone::Amber,
                        IconName::LoaderCircle,
                    ),
                    ChannelDownloadVerification::SizeChecked => (
                        self.tr("detail-verification-size-checked"),
                        Tone::Green,
                        IconName::CircleCheck,
                    ),
                    ChannelDownloadVerification::NotReached => (
                        self.tr("detail-verification-not-reached"),
                        Tone::Amber,
                        IconName::CircleX,
                    ),
                }
            } else if completed {
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
        let failure_reason = runtime_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.failure)
            .map(|failure| {
                (
                    self.tr(native_download_error_message_id(failure.kind)),
                    self.tr(if failure.retryable {
                        "detail-failure-retryable"
                    } else if failure.requires_user_action {
                        "detail-failure-user-action"
                    } else {
                        "detail-failure-terminal"
                    }),
                )
            })
            .or_else(|| {
                vault_snapshot.as_ref().and_then(|snapshot| {
                    if let VaultTransferState::Failed(kind) = snapshot.state {
                        Some((
                            self.tr(native_download_error_message_id(kind)),
                            self.tr("detail-failure-terminal"),
                        ))
                    } else {
                        None
                    }
                })
            });

        div()
            .w(px(layout.transfer_inspector_width()))
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
                                components::icon_button(
                                    "transfer-close-details",
                                    IconName::Close,
                                    self.tr("transfer-close-details"),
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.show_transfer_detail = false;
                                        cx.notify();
                                    },
                                )),
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
                    .when(preview_upload, |details| {
                        details.child(
                            div()
                                .mt_1()
                                .p_3()
                                .rounded(theme::RADIUS_SMALL)
                                .bg(theme::amber_soft())
                                .text_xs()
                                .text_color(theme::text_secondary())
                                .child(self.tr("transfer-preview-upload-note")),
                        )
                    })
                    .child(
                        div()
                            .mt_1()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .text_xs()
                            .child(self.tr("detail-verification"))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .text_color(verification_tone.foreground())
                                    .child(
                                        Icon::new(verification_icon)
                                            .text_color(verification_tone.foreground()),
                                    )
                                    .child(verification),
                            ),
                    )
                    .when_some(failure_reason, |details, (reason, guidance)| {
                        details.child(
                            div()
                                .mt_2()
                                .p_3()
                                .rounded(theme::RADIUS_SMALL)
                                .bg(theme::red_soft())
                                .flex()
                                .flex_col()
                                .gap_1()
                                .text_xs()
                                .text_color(theme::red())
                                .child(
                                    div()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(self.tr("detail-failure-reason")),
                                )
                                .child(reason)
                                .child(div().text_color(theme::text_secondary()).child(guidance)),
                        )
                    })
                    .when_some(telemetry, |details, telemetry| {
                        details.child(self.render_transfer_telemetry(telemetry, active, cx))
                    })
                    .when_some(runtime_snapshot, |details, snapshot| {
                        let events = snapshot.events.into_iter().map(|event| {
                            let label = self.tr(match event.kind {
                                ChannelDownloadEventKind::Queued => "trace-event-queued",
                                ChannelDownloadEventKind::Started => "trace-event-started",
                                ChannelDownloadEventKind::Paused => "trace-event-paused",
                                ChannelDownloadEventKind::Resumed => "trace-event-resumed",
                                ChannelDownloadEventKind::Completed => "trace-event-completed",
                                ChannelDownloadEventKind::Failed => "trace-event-failed",
                                ChannelDownloadEventKind::Cancelled => "trace-event-cancelled",
                            });
                            let timestamp =
                                format_unix_millis(self.locale(), event.timestamp_unix_ms);
                            let elapsed = event
                                .elapsed_ms
                                .map(|duration| format_duration_millis(self.locale(), duration))
                                .unwrap_or_else(|| "—".to_owned());
                            div()
                                .py_1()
                                .flex()
                                .items_center()
                                .gap_2()
                                .text_xs()
                                .child(
                                    div()
                                        .w(px(68.0))
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(label),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_color(theme::text_secondary())
                                        .child(timestamp),
                                )
                                .child(div().text_color(theme::text_muted()).child(elapsed))
                        });
                        let part_events =
                            snapshot
                                .part_events
                                .into_iter()
                                .rev()
                                .take(20)
                                .map(|event| {
                                    let state = self.tr(match event.state {
                                        DownloadPartState::Inflight => "transfer-part-inflight",
                                        DownloadPartState::Completed => "transfer-part-completed",
                                        DownloadPartState::Retry => "transfer-part-retry",
                                        DownloadPartState::Failed => "transfer-part-failed",
                                    });
                                    div().py_1().flex().flex_col().gap_1().text_xs().child(
                                        self.tr_with(
                                            "transfer-part-event-value",
                                            MessageArgs::new()
                                                .with(
                                                    "part",
                                                    format_integer(self.locale(), event.part_index),
                                                )
                                                .with(
                                                    "offset",
                                                    format_integer(
                                                        self.locale(),
                                                        event.offset_bytes,
                                                    ),
                                                )
                                                .with(
                                                    "length",
                                                    format_bytes(self.locale(), event.length_bytes),
                                                )
                                                .with("state", state.to_string())
                                                .with(
                                                    "attempt",
                                                    format_integer(
                                                        self.locale(),
                                                        u64::from(event.attempt),
                                                    ),
                                                )
                                                .with(
                                                    "elapsed",
                                                    format_duration_millis(
                                                        self.locale(),
                                                        event.elapsed_millis,
                                                    ),
                                                ),
                                        ),
                                    )
                                });
                        details
                            .child(
                                div()
                                    .mt_2()
                                    .pt_3()
                                    .border_t_1()
                                    .border_color(theme::border())
                                    .child(
                                        div()
                                            .mb_1()
                                            .text_xs()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(self.tr("detail-trace-timeline")),
                                    )
                                    .children(events),
                            )
                            .child(
                                div()
                                    .mt_2()
                                    .pt_3()
                                    .border_t_1()
                                    .border_color(theme::border())
                                    .child(
                                        div()
                                            .mb_1()
                                            .text_xs()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(self.tr("transfer-part-timeline")),
                                    )
                                    .children(part_events),
                            )
                    }),
            )
            .into_any_element()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TransferAction {
    Resume,
    Pause,
    Retry,
    Cancel,
    Delete,
}

impl TransferAction {
    fn supports(self, state: ChannelDownloadState) -> bool {
        self.supports_view(transfer_state(state))
    }
    fn supports_view(self, state: TransferState) -> bool {
        match self {
            Self::Resume => state == TransferState::Paused,
            Self::Pause => matches!(state, TransferState::Waiting | TransferState::Downloading),
            Self::Retry => matches!(state, TransferState::Failed | TransferState::Cancelled),
            Self::Cancel => matches!(
                state,
                TransferState::Waiting | TransferState::Downloading | TransferState::Paused
            ),
            Self::Delete => matches!(
                state,
                TransferState::Completed | TransferState::Failed | TransferState::Cancelled
            ),
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Resume => "action-resume",
            Self::Pause => "action-pause",
            Self::Retry => "action-retry",
            Self::Cancel => "action-cancel",
            Self::Delete => "action-delete-task",
        }
    }
    fn element_id(self) -> &'static str {
        match self {
            Self::Resume => "row-resume",
            Self::Pause => "row-pause",
            Self::Retry => "row-retry",
            Self::Cancel => "row-cancel",
            Self::Delete => "row-delete",
        }
    }
    fn icon(self) -> IconName {
        match self {
            Self::Resume => IconName::ArrowRight,
            Self::Pause => IconName::Dash,
            Self::Retry => IconName::Redo2,
            Self::Cancel => IconName::CircleX,
            Self::Delete => IconName::Delete,
        }
    }
}

fn execute_transfer_actions(
    action: TransferAction,
    ids: &[u64],
    cancelled: &std::sync::atomic::AtomicBool,
    mut apply: impl FnMut(u64) -> Result<(), teleark_core::ApplicationError>,
) -> (Vec<u64>, Option<teleark_core::ApplicationErrorKind>) {
    let mut deleted = Vec::new();
    let mut failure = None;
    for id in ids {
        if cancelled.load(std::sync::atomic::Ordering::Acquire) {
            break;
        }
        match apply(*id) {
            Ok(()) if action == TransferAction::Delete => deleted.push(*id),
            Ok(()) => {}
            Err(error) => {
                failure.get_or_insert(error.kind());
            }
        }
    }
    (deleted, failure)
}

// A collapsed batch selects its native child tasks. A set prevents double dispatch
// when both the group and expanded children are selected. Hidden selections never
// turn a visible-list action into an operation on another filter's tasks.
fn transfer_scope_ids(
    rows: &[TransferRow],
    selected: &std::collections::BTreeSet<u64>,
    snapshots: &[(u64, Option<u64>)],
) -> std::collections::BTreeSet<u64> {
    let has_selection = rows
        .iter()
        .enumerate()
        .any(|(index, row)| selected.contains(&transfer_selection_key(row, index)));
    rows.iter()
        .enumerate()
        .filter(|(index, row)| {
            !has_selection || selected.contains(&transfer_selection_key(row, *index))
        })
        .flat_map(|(_, row)| {
            if let Some(id) = row.runtime_task_id {
                vec![id]
            } else if let Some(batch) = row.runtime_batch_id {
                snapshots
                    .iter()
                    .filter(|(_, batch_id)| *batch_id == Some(batch))
                    .map(|(id, _)| *id)
                    .collect()
            } else {
                Vec::new()
            }
        })
        .collect()
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
        TransferState::Waiting | TransferState::Paused => Tone::Amber,
        TransferState::Completed => Tone::Green,
        TransferState::Failed | TransferState::Cancelled => Tone::Red,
    }
}

fn format_control_parameters(parameters: TransferControlParameters) -> String {
    format!(
        "C={} · W={} · F={} · P={} · E={} · Qe={}",
        parameters.transfer_connection_count,
        parameters.inflight_rpcs_per_connection,
        parameters.active_file_count,
        parameters.inflight_parts_per_file,
        parameters.encryption_worker_count,
        parameters.encrypted_part_queue_depth,
    )
}

fn control_parameter_value(
    parameters: TransferControlParameters,
    parameter: TunableParameter,
) -> u16 {
    match parameter {
        TunableParameter::TransferConnections => parameters.transfer_connection_count,
        TunableParameter::InflightRpcsPerConnection => parameters.inflight_rpcs_per_connection,
        TunableParameter::ActiveFiles => parameters.active_file_count,
        TunableParameter::InflightPartsPerFile => parameters.inflight_parts_per_file,
        TunableParameter::EncryptionWorkers => parameters.encryption_worker_count,
        TunableParameter::EncryptedQueueDepth => parameters.encrypted_part_queue_depth,
    }
}

const fn controller_phase_message_id(phase: ControllerPhase) -> &'static str {
    match phase {
        ControllerPhase::Ramp => "transfer-phase-ramp",
        ControllerPhase::Probe => "transfer-phase-probe",
        ControllerPhase::Stable => "transfer-phase-stable",
        ControllerPhase::Recover => "transfer-phase-recover",
    }
}

const fn bottleneck_message_id(bottleneck: TransferBottleneck) -> &'static str {
    match bottleneck {
        TransferBottleneck::Unknown => "transfer-bottleneck-unknown",
        TransferBottleneck::EncryptionCpu => "transfer-bottleneck-encryption",
        TransferBottleneck::TelegramOrNetwork => "transfer-bottleneck-network",
        TransferBottleneck::Disk => "transfer-bottleneck-disk",
        TransferBottleneck::Memory => "transfer-bottleneck-memory",
    }
}

const fn parameter_message_id(parameter: TunableParameter) -> &'static str {
    match parameter {
        TunableParameter::TransferConnections => "transfer-parameter-connections",
        TunableParameter::InflightRpcsPerConnection => "transfer-parameter-rpcs",
        TunableParameter::ActiveFiles => "transfer-parameter-files",
        TunableParameter::InflightPartsPerFile => "transfer-parameter-parts",
        TunableParameter::EncryptionWorkers => "transfer-parameter-encryption-workers",
        TunableParameter::EncryptedQueueDepth => "transfer-parameter-encrypted-queue",
    }
}

const fn decision_outcome_message_id(outcome: ControllerDecisionOutcome) -> &'static str {
    match outcome {
        ControllerDecisionOutcome::Probe => "transfer-decision-probe",
        ControllerDecisionOutcome::Keep => "transfer-decision-keep",
        ControllerDecisionOutcome::Confirm => "transfer-decision-confirm",
        ControllerDecisionOutcome::Platform => "transfer-decision-platform",
        ControllerDecisionOutcome::Rollback => "transfer-decision-rollback",
        ControllerDecisionOutcome::Recover => "transfer-decision-recover",
        ControllerDecisionOutcome::RespectSoftLimit => "transfer-decision-respect-soft-limit",
        ControllerDecisionOutcome::OverrideSoftLimit => "transfer-decision-override-soft-limit",
        ControllerDecisionOutcome::IgnoreSoftLimit => "transfer-decision-ignore-soft-limit",
        ControllerDecisionOutcome::PauseLane => "transfer-decision-pause-lane",
        ControllerDecisionOutcome::ResumeLane => "transfer-decision-resume-lane",
    }
}

const fn decision_reason_message_id(reason: ControllerDecisionReason) -> &'static str {
    match reason {
        ControllerDecisionReason::InitialRamp => "transfer-reason-initial-ramp",
        ControllerDecisionReason::InflightBelowBdpTarget => "transfer-reason-bdp",
        ControllerDecisionReason::ThroughputImproved => "transfer-reason-improved",
        ControllerDecisionReason::ThroughputNeedsConfirmation => "transfer-reason-confirm",
        ControllerDecisionReason::ThroughputGainBelowThreshold => "transfer-reason-platform",
        ControllerDecisionReason::ThroughputRegressed => "transfer-reason-regressed",
        ControllerDecisionReason::EncryptionStarvedNetwork => "transfer-reason-encryption-starved",
        ControllerDecisionReason::NetworkBackpressuredEncryption => {
            "transfer-reason-network-backpressure"
        }
        ControllerDecisionReason::SmallFileQueueNeedsSlots => "transfer-reason-small-files",
        ControllerDecisionReason::LargeFilePipelineNeedsParts => "transfer-reason-large-file",
        ControllerDecisionReason::MemoryBudgetPressure => "transfer-reason-memory",
        ControllerDecisionReason::DiskLimited => "transfer-reason-disk",
        ControllerDecisionReason::PartRetryRequired => "transfer-reason-part-retry",
        ControllerDecisionReason::FloodWaitRequired => "transfer-reason-flood-wait",
        ControllerDecisionReason::FloodWaitExpired => "transfer-reason-flood-wait-expired",
        ControllerDecisionReason::TelegramSoftLimitConflict => "transfer-reason-soft-limit",
        ControllerDecisionReason::AllParametersAtPlatform => "transfer-reason-all-platform",
    }
}

fn transfer_selection_key(transfer: &TransferRow, index: usize) -> u64 {
    transfer.runtime_task_id.unwrap_or_else(|| {
        if let Some(id) = transfer.vault_transfer_id {
            return 0x2000_0000_0000_0000_u64 | id;
        }
        transfer
            .runtime_batch_id
            .map(|id| 0x4000_0000_0000_0000_u64 | id)
            .unwrap_or(0x8000_0000_0000_0000_u64 | index as u64)
    })
}

fn transfer_state(state: ChannelDownloadState) -> TransferState {
    match state {
        ChannelDownloadState::Queued => TransferState::Waiting,
        ChannelDownloadState::Running => TransferState::Downloading,
        ChannelDownloadState::Paused => TransferState::Paused,
        ChannelDownloadState::Completed => TransferState::Completed,
        ChannelDownloadState::Failed(_) => TransferState::Failed,
        ChannelDownloadState::Cancelled => TransferState::Cancelled,
    }
}

fn transfer_progress(transferred: u64, total: u64, completed: bool) -> f32 {
    if total == 0 {
        if completed { 100.0 } else { 0.0 }
    } else {
        (transferred as f64 * 100.0 / total as f64).clamp(0.0, 100.0) as f32
    }
}

fn aggregate_batch_state(items: &[&ChannelDownloadSnapshot]) -> TransferState {
    aggregate_download_states(items.iter().map(|item| item.state))
}

fn aggregate_download_states(
    states: impl IntoIterator<Item = ChannelDownloadState>,
) -> TransferState {
    let states = states.into_iter().collect::<Vec<_>>();
    if states.contains(&ChannelDownloadState::Running) {
        TransferState::Downloading
    } else if states.contains(&ChannelDownloadState::Queued) {
        TransferState::Waiting
    } else if states.contains(&ChannelDownloadState::Paused) {
        TransferState::Paused
    } else if states
        .iter()
        .any(|state| matches!(state, ChannelDownloadState::Failed(_)))
    {
        TransferState::Failed
    } else if states.contains(&ChannelDownloadState::Cancelled) {
        TransferState::Cancelled
    } else {
        TransferState::Completed
    }
}

fn current_unix_millis() -> Option<i64> {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis();
    i64::try_from(millis).ok()
}

fn native_download_error_message_id(kind: teleark_core::ApplicationErrorKind) -> &'static str {
    use teleark_core::ApplicationErrorKind;

    match kind {
        ApplicationErrorKind::InvalidRequest => "native-download-error-invalid-request",
        ApplicationErrorKind::NotFound => "native-download-error-not-found",
        ApplicationErrorKind::Conflict => "native-download-error-conflict",
        ApplicationErrorKind::Persistence => "native-download-error-persistence",
        ApplicationErrorKind::SourceMissing => "native-download-error-source-missing",
        ApplicationErrorKind::SourceChanged => "native-download-error-source-changed",
        ApplicationErrorKind::PermissionDenied => "native-download-error-permission-denied",
        ApplicationErrorKind::Capacity => "native-download-error-capacity",
        ApplicationErrorKind::Authorization => "native-download-error-authorization",
        ApplicationErrorKind::Network => "native-download-error-network",
        ApplicationErrorKind::Cancelled => "native-download-error-cancelled",
        _ => "native-download-error-unknown",
    }
}

fn toggle_visible_selection(
    selected: &mut std::collections::BTreeSet<u64>,
    visible: &[u64],
    all_visible_selected: bool,
) {
    if all_visible_selected {
        for key in visible {
            selected.remove(key);
        }
    } else {
        selected.extend(visible.iter().copied());
    }
}

fn transfer_matches_nav(
    selection: &str,
    state: TransferState,
    direction: TransferDirection,
) -> bool {
    match selection {
        "nav-uploads" => direction == TransferDirection::Upload,
        "nav-downloads" => direction == TransferDirection::Download,
        "nav-waiting" => matches!(state, TransferState::Waiting | TransferState::Paused),
        "nav-completed" => state == TransferState::Completed,
        "nav-failed" => matches!(state, TransferState::Failed | TransferState::Cancelled),
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
                .w(px(128.0))
                .flex_none()
                .text_color(theme::text_muted())
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .whitespace_normal()
                .text_color(theme::text_secondary())
                .child(value),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bulk_commands_continue_after_failure_and_only_clear_successful_deletions() {
        use teleark_core::{ApplicationError, ApplicationErrorKind};
        let cancelled = std::sync::atomic::AtomicBool::new(false);
        let mut called = Vec::new();
        let (deleted, error) =
            execute_transfer_actions(TransferAction::Delete, &[1, 2, 3], &cancelled, |id| {
                called.push(id);
                if id == 2 {
                    Err(ApplicationError::new(ApplicationErrorKind::Persistence))
                } else {
                    Ok(())
                }
            });
        assert_eq!(called, vec![1, 2, 3]);
        assert_eq!(deleted, vec![1, 3]);
        assert_eq!(error, Some(ApplicationErrorKind::Persistence));
    }

    #[test]
    fn command_owner_cancellation_stops_at_task_boundaries() {
        let cancelled = std::sync::atomic::AtomicBool::new(false);
        let mut called = Vec::new();
        let (deleted, error) =
            execute_transfer_actions(TransferAction::Pause, &[1, 2, 3], &cancelled, |id| {
                called.push(id);
                cancelled.store(true, std::sync::atomic::Ordering::Release);
                Ok(())
            });
        assert_eq!(called, vec![1]);
        assert!(deleted.is_empty());
        assert_eq!(error, None);
    }

    #[test]
    fn batch_selection_expands_children_once_and_ignores_hidden_selection() {
        let mut group = transfers(false).remove(0);
        group.runtime_task_id = None;
        group.runtime_batch_id = Some(7);
        let mut child = group.clone();
        child.runtime_task_id = Some(11);
        child.batch_child = true;
        let mut single = child.clone();
        single.runtime_task_id = Some(13);
        single.runtime_batch_id = None;
        single.batch_child = false;
        let rows = vec![group, child, single];
        let tasks = [(11, Some(7)), (12, Some(7)), (13, None), (99, None)];
        let group_key = transfer_selection_key(&rows[0], 0);
        let selected = std::collections::BTreeSet::from([group_key, 11, 99]);
        assert_eq!(
            transfer_scope_ids(&rows, &selected, &tasks),
            std::collections::BTreeSet::from([11, 12])
        );
        assert_eq!(
            transfer_scope_ids(&rows, &std::collections::BTreeSet::from([13]), &tasks),
            std::collections::BTreeSet::from([13])
        );
        assert_eq!(
            transfer_scope_ids(&rows, &std::collections::BTreeSet::from([99]), &tasks),
            std::collections::BTreeSet::from([11, 12, 13])
        );
        assert!(transfer_scope_ids(&[], &selected, &tasks).is_empty());
    }

    #[test]
    fn action_availability_matches_native_lifecycle() {
        use teleark_core::ApplicationErrorKind;
        for state in [
            ChannelDownloadState::Queued,
            ChannelDownloadState::Running,
            ChannelDownloadState::Paused,
            ChannelDownloadState::Completed,
            ChannelDownloadState::Failed(ApplicationErrorKind::Network),
            ChannelDownloadState::Cancelled,
        ] {
            assert_eq!(
                TransferAction::Pause.supports(state),
                matches!(
                    state,
                    ChannelDownloadState::Queued | ChannelDownloadState::Running
                )
            );
            assert_eq!(
                TransferAction::Resume.supports(state),
                state == ChannelDownloadState::Paused
            );
            assert_eq!(
                TransferAction::Retry.supports(state),
                matches!(
                    state,
                    ChannelDownloadState::Failed(_) | ChannelDownloadState::Cancelled
                )
            );
            assert_ne!(
                TransferAction::Delete.supports(state),
                TransferAction::Cancel.supports(state)
            );
        }
    }

    #[test]
    fn vault_selection_identity_survives_reordering_and_is_disjoint() {
        let mut row = transfers(false).remove(0);
        row.vault_transfer_id = Some(42);
        assert_eq!(
            transfer_selection_key(&row, 0),
            transfer_selection_key(&row, 9)
        );
        assert_ne!(transfer_selection_key(&row, 0), 42);
        assert_ne!(transfer_selection_key(&row, 0), 0x4000_0000_0000_002a);
    }

    #[test]
    fn transfer_sidebar_facets_match_only_the_requested_state() {
        assert!(transfer_matches_nav(
            "nav-completed",
            TransferState::Completed,
            TransferDirection::Download,
        ));
        assert!(!transfer_matches_nav(
            "nav-completed",
            TransferState::Failed,
            TransferDirection::Download,
        ));
        assert!(transfer_matches_nav(
            "nav-failed",
            TransferState::Failed,
            TransferDirection::Download,
        ));
        assert!(transfer_matches_nav(
            "nav-transfers-all",
            TransferState::Waiting,
            TransferDirection::Upload,
        ));
        assert!(transfer_matches_nav(
            "nav-uploads",
            TransferState::Waiting,
            TransferDirection::Upload,
        ));
        assert!(!transfer_matches_nav(
            "nav-uploads",
            TransferState::Downloading,
            TransferDirection::Download,
        ));
    }

    #[test]
    fn transfer_selection_keys_are_stable_for_runtime_tasks_and_disjoint_for_preview_rows() {
        let mut rows = transfers(false);
        assert_eq!(transfer_selection_key(&rows[0], 3), 0x8000_0000_0000_0003);
        rows[0].runtime_task_id = Some(42);
        assert_eq!(transfer_selection_key(&rows[0], 3), 42);
        rows[0].runtime_task_id = None;
        rows[0].runtime_batch_id = Some(42);
        assert_eq!(transfer_selection_key(&rows[0], 3), 0x4000_0000_0000_002a);
    }

    #[test]
    fn batch_state_uses_active_then_actionable_then_terminal_precedence() {
        use teleark_core::ApplicationErrorKind;

        assert_eq!(
            aggregate_download_states([
                ChannelDownloadState::Completed,
                ChannelDownloadState::Running,
                ChannelDownloadState::Failed(ApplicationErrorKind::Network),
            ]),
            TransferState::Downloading
        );
        assert_eq!(
            aggregate_download_states([
                ChannelDownloadState::Completed,
                ChannelDownloadState::Paused,
                ChannelDownloadState::Failed(ApplicationErrorKind::Network),
            ]),
            TransferState::Paused
        );
        assert_eq!(
            aggregate_download_states([
                ChannelDownloadState::Completed,
                ChannelDownloadState::Failed(ApplicationErrorKind::Network),
            ]),
            TransferState::Failed
        );
        assert_eq!(
            aggregate_download_states([
                ChannelDownloadState::Completed,
                ChannelDownloadState::Completed,
            ]),
            TransferState::Completed
        );
    }

    #[test]
    fn select_all_toggles_only_visible_rows_and_preserves_hidden_selection() {
        let mut selected = std::collections::BTreeSet::from([99]);
        toggle_visible_selection(&mut selected, &[1, 2], false);
        assert_eq!(selected, std::collections::BTreeSet::from([1, 2, 99]));
        toggle_visible_selection(&mut selected, &[1, 2], true);
        assert_eq!(selected, std::collections::BTreeSet::from([99]));
    }

    #[test]
    fn every_native_download_failure_has_a_localized_specific_reason() {
        use teleark_core::ApplicationErrorKind;
        use teleark_i18n::{Localizer, MessageId, SupportedLocale};

        let kinds = [
            ApplicationErrorKind::InvalidRequest,
            ApplicationErrorKind::NotFound,
            ApplicationErrorKind::Conflict,
            ApplicationErrorKind::Persistence,
            ApplicationErrorKind::SourceMissing,
            ApplicationErrorKind::SourceChanged,
            ApplicationErrorKind::PermissionDenied,
            ApplicationErrorKind::Capacity,
            ApplicationErrorKind::Authorization,
            ApplicationErrorKind::Network,
            ApplicationErrorKind::Cancelled,
        ];
        for locale in SupportedLocale::ALL {
            let localizer = Localizer::new(locale).expect("localizer");
            for kind in kinds {
                let id = MessageId::new(native_download_error_message_id(kind));
                assert!(localizer.contains(locale, id));
            }
        }
    }
}
