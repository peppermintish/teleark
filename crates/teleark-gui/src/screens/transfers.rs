use gpui::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px,
};
use gpui_component::{Disableable as _, Icon, IconName, scroll::ScrollableElement as _};
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
    mock::{
        ActivityLog, ConnectionRow, TransferDirection, TransferRow, TransferState, activity_logs,
        connections, transfers,
    },
    theme,
};

const CONNECTION_CARD_WIDTH: f32 = 440.0;
const CONNECTION_CLIENT_WIDTH: f32 = 82.0;
const CONNECTION_LATENCY_WIDTH: f32 = 72.0;

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
        let runtime_backed = self.transfers.is_some();
        let all_transfer_rows = self.transfer_rows();
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
            .filter(|(_, transfer)| {
                transfer.runtime_task_id.is_some() || transfer.runtime_batch_id.is_none()
            })
            .map(|(index, transfer)| transfer_selection_key(transfer, index))
            .collect();
        let all_visible_selected = !visible_transfer_keys.is_empty()
            && visible_transfer_keys
                .iter()
                .all(|key| self.selected_transfer_keys.contains(key));
        let selected_completed_destination = transfer_rows
            .iter()
            .enumerate()
            .filter(|(index, transfer)| {
                transfer.state == TransferState::Completed
                    && self
                        .selected_transfer_keys
                        .contains(&transfer_selection_key(transfer, *index))
            })
            .map(|(_, transfer)| std::path::PathBuf::from(transfer.destination.as_ref()))
            .next();
        let runtime_snapshots = self
            .transfers
            .as_ref()
            .and_then(|transfers| transfers.snapshots().ok())
            .unwrap_or_default();
        let runtime_active_ids: Vec<_> = runtime_snapshots
            .iter()
            .filter(|snapshot| {
                matches!(
                    snapshot.state,
                    ChannelDownloadState::Queued | ChannelDownloadState::Running
                )
            })
            .map(|snapshot| snapshot.id)
            .collect();
        let runtime_paused_ids: Vec<_> = runtime_snapshots
            .iter()
            .filter(|snapshot| snapshot.state == ChannelDownloadState::Paused)
            .map(|snapshot| snapshot.id)
            .collect();
        let retry_action = if layout.is_compact() {
            components::icon_button(
                "transfers-retry",
                IconName::Redo2,
                self.tr("action-retry-failed"),
            )
            .into_any_element()
        } else {
            components::button(
                "transfers-retry",
                self.tr("action-retry-failed"),
                Some(IconName::Redo2),
                false,
            )
            .into_any_element()
        };
        let clear_action = if layout.is_compact() {
            components::icon_button(
                "transfers-clear",
                IconName::Delete,
                self.tr("action-clear-completed"),
            )
            .into_any_element()
        } else {
            components::button(
                "transfers-clear",
                self.tr("action-clear-completed"),
                Some(IconName::Delete),
                false,
            )
            .into_any_element()
        };
        let new_queue_action = if layout.is_compact() {
            components::icon_button(
                "transfers-new-queue",
                IconName::Plus,
                self.tr("action-new-queue"),
            )
            .into_any_element()
        } else {
            components::button(
                "transfers-new-queue",
                self.tr("action-new-queue"),
                Some(IconName::Plus),
                false,
            )
            .into_any_element()
        };
        let status_filter = if layout.is_compact() {
            components::icon_button(
                "transfers-status-filter",
                IconName::Settings2,
                self.tr("filter-all-statuses"),
            )
            .into_any_element()
        } else {
            components::button(
                "transfers-status-filter",
                self.tr("filter-all-statuses"),
                Some(IconName::Settings2),
                false,
            )
            .into_any_element()
        };

        let summary = div()
            .h(px(if layout.is_spacious() {
                120.0
            } else if runtime_backed {
                202.0
            } else {
                170.0
            }))
            .px(px(padding))
            .py(if layout.is_spacious() {
                px(12.0)
            } else {
                px(8.0)
            })
            .when(layout.is_spacious(), |summary| summary.flex())
            .when(!layout.is_spacious(), |summary| summary.grid().grid_cols(3))
            .gap(if layout.is_spacious() {
                px(12.0)
            } else {
                px(8.0)
            })
            .child(summary_card(
                IconName::ArrowUp,
                self.tr("transfer-summary-uploading"),
                format_integer(self.locale(), uploading as u64),
                self.tr_with(
                    "transfer-summary-task-count",
                    MessageArgs::new()
                        .with("count", format_integer(self.locale(), uploading as u64)),
                ),
                Tone::Purple,
            ))
            .child(summary_card(
                IconName::ArrowDown,
                self.tr("transfer-summary-downloading"),
                format_integer(self.locale(), downloading as u64),
                self.tr_with(
                    "transfer-summary-task-count",
                    MessageArgs::new()
                        .with("count", format_integer(self.locale(), downloading as u64)),
                ),
                Tone::Blue,
            ))
            .child(summary_card(
                IconName::Calendar,
                self.tr("transfer-summary-waiting"),
                format_integer(self.locale(), waiting as u64),
                self.tr("transfer-summary-ready"),
                Tone::Amber,
            ))
            .child(summary_card(
                IconName::CircleCheck,
                self.tr("transfer-summary-completed"),
                format_integer(self.locale(), completed as u64),
                self.tr("transfer-summary-completed-note"),
                Tone::Green,
            ))
            .child(summary_card(
                IconName::CircleX,
                self.tr("transfer-summary-failed"),
                format_integer(self.locale(), failed as u64),
                self.tr("transfer-summary-retry"),
                Tone::Red,
            ))
            .child(summary_card(
                IconName::ArrowDown,
                self.tr("transfer-summary-total-speed"),
                if total_speed == 0 {
                    self.tr("transfer-value-unavailable").to_string()
                } else {
                    format_speed(self.locale(), total_speed)
                },
                self.tr("transfer-summary-live-runtime"),
                Tone::Blue,
            ));

        let toolbar = div()
            .min_h(px(46.0))
            .px(px(padding))
            .py_2()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .when(!runtime_backed, |toolbar| {
                toolbar.child(components::button(
                    "transfers-start-all",
                    self.tr("action-start-all"),
                    Some(IconName::ArrowRight),
                    true,
                ))
            })
            .when(
                runtime_backed
                    && (!runtime_active_ids.is_empty() || !runtime_paused_ids.is_empty()),
                |toolbar| {
                    let pause_all = !runtime_active_ids.is_empty();
                    let task_ids = if pause_all {
                        runtime_active_ids.clone()
                    } else {
                        runtime_paused_ids.clone()
                    };
                    toolbar.child(
                        components::button(
                            "transfers-runtime-pause-all",
                            if pause_all {
                                self.tr("action-pause-all")
                            } else {
                                self.tr("action-resume-all")
                            },
                            Some(if pause_all {
                                IconName::Dash
                            } else {
                                IconName::ArrowRight
                            }),
                            false,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(transfers) = this.transfers.as_ref() {
                                for id in &task_ids {
                                    let _ = if pause_all {
                                        transfers.pause(*id)
                                    } else {
                                        transfers.resume(*id)
                                    };
                                }
                            }
                            cx.notify();
                        })),
                    )
                },
            )
            .when(!runtime_backed, |toolbar| {
                toolbar.child(
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
            })
            .when(!runtime_backed, |toolbar| toolbar.child(retry_action))
            .when(!runtime_backed, |toolbar| toolbar.child(clear_action))
            .when(!runtime_backed, |toolbar| toolbar.child(new_queue_action))
            .when(!layout.is_compact(), |toolbar| {
                toolbar.child(div().flex_1())
            })
            .child(status_filter)
            .when_some(selected_completed_destination, |toolbar, destination| {
                toolbar.child(
                    components::button(
                        "transfers-reveal-selected",
                        self.tr("action-show-in-folder"),
                        Some(IconName::FolderOpen),
                        false,
                    )
                    .on_click(move |_, _, cx| cx.reveal_path(&destination)),
                )
            })
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
            .child(
                div()
                    .id("transfers-select-all")
                    .w(px(28.0))
                    .cursor_pointer()
                    .focusable()
                    .tab_index(0)
                    .child(selection_box(all_visible_selected))
                    .on_click(cx.listener({
                        let visible_transfer_keys = visible_transfer_keys.clone();
                        move |this, _, _, cx| {
                            if all_visible_selected {
                                toggle_visible_selection(
                                    &mut this.selected_transfer_keys,
                                    &visible_transfer_keys,
                                    true,
                                );
                            } else {
                                toggle_visible_selection(
                                    &mut this.selected_transfer_keys,
                                    &visible_transfer_keys,
                                    false,
                                );
                            }
                            cx.notify();
                        }
                    }))
                    .on_key_down(cx.listener({
                        let visible_transfer_keys = visible_transfer_keys.clone();
                        move |this, event: &gpui::KeyDownEvent, _, cx| {
                            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                if all_visible_selected {
                                    toggle_visible_selection(
                                        &mut this.selected_transfer_keys,
                                        &visible_transfer_keys,
                                        true,
                                    );
                                } else {
                                    toggle_visible_selection(
                                        &mut this.selected_transfer_keys,
                                        &visible_transfer_keys,
                                        false,
                                    );
                                }
                                cx.notify();
                            }
                        }
                    })),
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
                    .child(transfer_header(self.tr("table-eta"), Some(64.0)))
            })
            .child(transfer_header(
                self.tr("table-status"),
                Some(layout.transfer_status_width()),
            ))
            .when(layout.shows_transfer_destination(), |header| {
                header.child(transfer_header(self.tr("table-destination"), Some(118.0)))
            });

        let rows = transfer_rows
            .into_iter()
            .enumerate()
            .map(|(index, transfer)| self.render_transfer_row(index, transfer, layout, cx));
        let has_rows = selected.is_some();

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
                    format_integer(self.locale(), self.selected_transfer_keys.len() as u64),
                ),
            ))
            .child(self.tr_with(
                "transfer-footer-total-live",
                MessageArgs::new().with(
                    "count",
                    format_integer(
                        self.locale(),
                        (downloading + waiting + completed + failed) as u64,
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
            .when(!layout.is_compact(), |footer| footer.child(div().flex_1()))
            .when(!runtime_backed, |footer| {
                footer
                    .child(div().text_color(theme::blue()).child("↓ 23.6 MB/s"))
                    .child(div().text_color(theme::blue()).child("↑ 14.6 MB/s"))
                    .child(self.tr("transfer-footer-unlimited"))
            });

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

        let bottom = div()
            .h(px(if layout.is_compact() { 132.0 } else { 184.0 }))
            .px(px(padding))
            .py_2()
            .flex()
            .gap_3()
            .child(self.render_activity_log(layout.is_compact()))
            .child(self.render_connections(layout))
            .overflow_x_scrollbar();

        let main = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(summary)
            .child(toolbar)
            .child(table)
            .when(!runtime_backed, |main| main.child(bottom));

        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .bg(theme::canvas())
            .child(main)
            .when_some(selected, |page, selected| {
                page.child(self.render_transfer_detail(selected, layout, cx))
            })
            .into_any_element()
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
        let batch_group_id = transfer
            .runtime_batch_id
            .filter(|_| transfer.runtime_task_id.is_none() && !transfer.batch_child);
        let batch_expanded =
            batch_group_id.is_some_and(|id| self.expanded_transfer_batches.contains(&id));

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
            .when(focused, |row| {
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
            .when(batch_group_id.is_none(), |row| {
                row.child(
                    div()
                        .id(("transfer-select", selection_key))
                        .w(px(28.0))
                        .cursor_pointer()
                        .child(selection_box(selected))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            if !this.selected_transfer_keys.insert(selection_key) {
                                this.selected_transfer_keys.remove(&selection_key);
                            }
                            cx.notify();
                        })),
                )
            })
            .when(batch_group_id.is_some(), |row| row.child(div().w(px(28.0))))
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
                            .child(format!("{:.1}%", transfer.progress)),
                    )
                    .child(components::progress(transfer.progress, tone)),
            )
            .when(layout.shows_transfer_speed(), |row| {
                row.child(transfer_cell(transfer.speed, 82.0))
                    .child(transfer_cell(transfer.eta, 64.0))
            })
            .child(
                div()
                    .w(px(layout.transfer_status_width()))
                    .child(components::badge(
                        self.tr(transfer.state.message_id()),
                        tone,
                    )),
            )
            .when(layout.shows_transfer_destination(), |row| {
                row.child(transfer_cell(transfer.destination, 118.0))
            })
            .into_any_element()
    }

    fn render_activity_log(&self, compact: bool) -> AnyElement {
        let rows = activity_logs().into_iter().map(|log| {
            let message = self.tr(log.message_id);
            render_log_row(log, message)
        });
        components::card()
            .when(compact, |card| card.w(px(420.0)).flex_none())
            .when(!compact, |card| card.flex_1())
            .h_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(bottom_header(self.tr("transfer-tab-log")))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .px_3()
                    .py_1()
                    .children(rows),
            )
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

    fn render_connections(&self, layout: LayoutPolicy) -> AnyElement {
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
            .child(connection_cell(
                self.tr("connection-client"),
                Some(CONNECTION_CLIENT_WIDTH),
            ))
            .child(connection_cell(
                self.tr("connection-latency"),
                Some(CONNECTION_LATENCY_WIDTH),
            ));
        let rows = connections().into_iter().map(render_connection_row);

        components::card()
            .when(!layout.is_spacious(), |card| {
                card.w(px(CONNECTION_CARD_WIDTH)).flex_none()
            })
            .when(layout.is_spacious(), |card| card.flex_1())
            .h_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(bottom_header(self.tr("connection-title")))
            .child(header)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .children(rows),
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

        let primary_action = if (runtime_backed || (vault_backed && !upload)) && completed {
            let destination = std::path::PathBuf::from(transfer.destination.as_ref());
            components::button(
                "detail-open",
                self.tr("action-open-file"),
                Some(IconName::FolderOpen),
                true,
            )
            .on_click(move |_, _, cx| cx.open_with_system(&destination))
        } else if let Some(snapshot) = runtime_snapshot.as_ref() {
            let id = snapshot.id;
            match snapshot.state {
                ChannelDownloadState::Queued | ChannelDownloadState::Running => components::button(
                    "detail-runtime-pause",
                    self.tr("action-pause"),
                    Some(IconName::Dash),
                    true,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(transfers) = this.transfers.as_ref() {
                        let _ = transfers.pause(id);
                    }
                    cx.notify();
                })),
                ChannelDownloadState::Paused => components::button(
                    "detail-runtime-resume",
                    self.tr("action-resume"),
                    Some(IconName::ArrowRight),
                    true,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(transfers) = this.transfers.as_ref() {
                        let _ = transfers.resume(id);
                    }
                    cx.notify();
                })),
                ChannelDownloadState::Failed(_) => components::button(
                    "detail-runtime-retry",
                    self.tr("action-retry"),
                    Some(IconName::Redo2),
                    true,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(transfers) = this.transfers.as_ref() {
                        let _ = transfers.retry(id);
                    }
                    cx.notify();
                })),
                ChannelDownloadState::Cancelled => components::button(
                    "detail-runtime-cancelled",
                    self.tr("transfer.state.cancelled"),
                    Some(IconName::CircleX),
                    true,
                )
                .disabled(true),
                ChannelDownloadState::Completed => components::button(
                    "detail-runtime-completed",
                    self.tr("transfer.state.completed"),
                    Some(IconName::CircleCheck),
                    true,
                )
                .disabled(true),
            }
        } else if vault_backed {
            components::button(
                "detail-vault-state",
                self.tr(if completed {
                    "transfer.state.completed"
                } else if failed {
                    "transfer.state.failed"
                } else {
                    "detail-vault-controls-unavailable"
                }),
                Some(if completed {
                    IconName::CircleCheck
                } else if failed {
                    IconName::CircleX
                } else {
                    IconName::LoaderCircle
                }),
                true,
            )
            .disabled(true)
        } else {
            match transfer.state {
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
                TransferState::Waiting if preview_upload => components::button(
                    "detail-upload-preview",
                    self.tr("prototype-demo-badge"),
                    Some(IconName::ArrowUp),
                    true,
                )
                .disabled(true),
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
                TransferState::Paused => components::button(
                    "detail-resume",
                    self.tr("action-resume"),
                    Some(IconName::ArrowRight),
                    true,
                ),
                TransferState::Cancelled => components::button(
                    "detail-cancelled",
                    self.tr("transfer.state.cancelled"),
                    Some(IconName::CircleX),
                    true,
                )
                .disabled(true),
            }
        };
        let reveal_action = if (runtime_backed || (vault_backed && !upload)) && completed {
            let destination = std::path::PathBuf::from(transfer.destination.as_ref());
            Some(
                components::button(
                    "detail-reveal",
                    self.tr("action-show-in-folder"),
                    Some(IconName::FolderOpen),
                    false,
                )
                .on_click(move |_, _, cx| cx.reveal_path(&destination)),
            )
        } else {
            None
        };
        let cancel_action = runtime_snapshot.as_ref().and_then(|snapshot| {
            matches!(
                snapshot.state,
                ChannelDownloadState::Queued
                    | ChannelDownloadState::Running
                    | ChannelDownloadState::Paused
            )
            .then(|| {
                let id = snapshot.id;
                components::button(
                    "detail-runtime-cancel",
                    self.tr("action-cancel"),
                    Some(IconName::CircleX),
                    false,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(transfers) = this.transfers.as_ref() {
                        let _ = transfers.cancel(id);
                    }
                    cx.notify();
                }))
            })
        });
        let delete_action = runtime_snapshot.as_ref().and_then(|snapshot| {
            matches!(
                snapshot.state,
                ChannelDownloadState::Completed
                    | ChannelDownloadState::Failed(_)
                    | ChannelDownloadState::Cancelled
            )
            .then(|| {
                let id = snapshot.id;
                let awaiting_confirmation = self.pending_transfer_delete == Some(id);
                components::button(
                    "detail-runtime-delete",
                    self.tr(if awaiting_confirmation {
                        "action-confirm-delete-task"
                    } else {
                        "action-delete-task"
                    }),
                    Some(IconName::Delete),
                    false,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.pending_transfer_delete == Some(id) {
                        if let Some(transfers) = this.transfers.as_ref()
                            && transfers.delete(id).is_ok()
                        {
                            this.pending_transfer_delete = None;
                            this.selected_transfer_keys.clear();
                            this.selected_file = 0;
                        }
                    } else {
                        this.pending_transfer_delete = Some(id);
                    }
                    cx.notify();
                }))
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
            .child(
                div()
                    .p_4()
                    .flex_none()
                    .border_t_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(primary_action.flex_1())
                            .when_some(cancel_action, |actions, cancel| actions.child(cancel))
                            .when_some(delete_action, |actions, delete| actions.child(delete))
                            .when_some(reveal_action, |actions, reveal| actions.child(reveal)),
                    ),
            )
            .into_any_element()
    }
}

fn summary_card(
    icon: IconName,
    label: SharedString,
    value: impl Into<SharedString>,
    hint: SharedString,
    tone: Tone,
) -> AnyElement {
    components::card()
        .flex_1()
        .min_w(px(0.0))
        .p_2()
        .flex()
        .items_start()
        .gap_2()
        .child(
            div()
                .size(px(30.0))
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
                        .child(value.into()),
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
        .child(connection_cell(
            connection.client,
            Some(CONNECTION_CLIENT_WIDTH),
        ))
        .child(connection_cell(
            connection.latency,
            Some(CONNECTION_LATENCY_WIDTH),
        ))
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

fn transfer_selection_key(transfer: &TransferRow, index: usize) -> u64 {
    transfer.runtime_task_id.unwrap_or_else(|| {
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

fn selection_box(selected: bool) -> AnyElement {
    div()
        .size(px(14.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(3.0))
        .border_1()
        .border_color(if selected {
            theme::blue()
        } else {
            theme::border()
        })
        .bg(if selected {
            theme::blue()
        } else {
            theme::surface()
        })
        .when(selected, |checkbox| {
            checkbox.child(
                Icon::new(IconName::Check)
                    .size(px(11.0))
                    .text_color(gpui::white()),
            )
        })
        .into_any_element()
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
