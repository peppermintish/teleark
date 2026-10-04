use super::*;
use crate::app::ChannelBatchPeriod;
use teleark_core::FileKind;
use teleark_i18n::{MessageArgs, format::format_integer};

impl TeleArkApp {
    pub(super) fn render_managed_download_controls(
        &self,
        count: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.selected_managed_package_ids.len();
        let downloadable_count = self.managed_projection.borrow().downloadable_count();
        let busy = self.vault_download_in_flight || self.vault_locked;
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
            .flex_none()
            .when(
                self.show_channel_detail
                    && self
                        .managed_projection
                        .borrow()
                        .selected(self.selected_telegram_message_id)
                        .is_some(),
                |body| body.mr(px(theme::MANAGED_FILE_INSPECTOR_WIDTH)),
            )
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(theme::border())
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .min_w_0()
                            .text_xs()
                            .text_color(theme::text_secondary())
                            .child(self.tr_with(
                                if selected > 0 {
                                    "telegram-files-selected"
                                } else {
                                    "channel-file-count"
                                },
                                MessageArgs::new().with(
                                    "count",
                                    format_integer(
                                        self.locale(),
                                        if selected > 0 { selected } else { count } as u64,
                                    ),
                                ),
                            )),
                    )
                    .child(
                        components::compact_button(
                            "managed-filter-toggle",
                            self.tr("transfer-filter-title"),
                            Some(IconName::Settings2),
                            false,
                        )
                        .ghost()
                        .debug_selector(|| "managed-filter-toggle".into())
                        .when(self.channel_batch_expanded, |button| {
                            button.bg(theme::blue_soft()).text_color(theme::blue())
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.channel_batch_expanded = !this.channel_batch_expanded;
                            if this.channel_batch_expanded {
                                this.show_channel_detail = false;
                            }
                            cx.notify();
                        })),
                    )
                    .child(
                        components::compact_button(
                            "managed-select-all",
                            self.tr("channel-select-all-compact"),
                            None,
                            false,
                        )
                        .ghost()
                        .disabled(downloadable_count == 0)
                        .debug_selector(|| "managed-select-all".into())
                        .tooltip(self.tr("managed-download-select-all-help"))
                        .on_click(cx.listener(|this, _, _, cx| this.select_all_managed_files(cx))),
                    )
                    .child(
                        components::compact_button(
                            "managed-download-matching",
                            self.tr("managed-download-matching-action"),
                            Some(IconName::ArrowDown),
                            false,
                        )
                        .disabled(busy || downloadable_count == 0)
                        .debug_selector(|| "managed-download-matching".into())
                        .tooltip(self.tr("managed-download-matching-help"))
                        .on_click(
                            cx.listener(|this, _, _, cx| this.download_managed_files(true, cx)),
                        ),
                    )
                    .when(selected > 0, |row| {
                        row.child(
                            components::compact_button(
                                "managed-clear-selection",
                                self.tr("telegram-files-clear-selection"),
                                Some(IconName::Close),
                                false,
                            )
                            .ghost()
                            .debug_selector(|| "managed-clear-selection".into())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.selected_managed_package_ids.clear();
                                cx.notify();
                            })),
                        )
                        .child(
                            components::compact_button(
                                "managed-download-selected",
                                self.tr("telegram-batch-download-action"),
                                Some(IconName::ArrowDown),
                                true,
                            )
                            .disabled(busy)
                            .debug_selector(|| "managed-download-selected".into())
                            .on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.download_managed_files(false, cx)
                                }),
                            ),
                        )
                    }),
            )
            .when(self.channel_batch_expanded, |body| {
                body.child(
                    div()
                        .mt_2()
                        .text_xs()
                        .text_color(theme::text_secondary())
                        .child(self.tr("managed-download-matching-help")),
                )
                .child(
                    div()
                        .mt_2()
                        .text_xs()
                        .text_color(theme::text_muted())
                        .child(self.tr("telegram-batch-period-label")),
                )
                .child(
                    div()
                        .mt_1()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .children(periods.into_iter().map(|(period, label)| {
                            components::compact_button(
                                ("managed-period", period as usize),
                                self.tr(label),
                                None,
                                self.channel_batch_period == period,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.channel_batch_period = period;
                                    cx.notify();
                                },
                            ))
                        })),
                )
                .child(
                    div()
                        .mt_2()
                        .text_xs()
                        .text_color(theme::text_muted())
                        .child(self.tr("telegram-batch-kind-label")),
                )
                .child(
                    div()
                        .mt_1()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .child(
                            components::compact_button(
                                "managed-kind-all",
                                self.tr("telegram-batch-kind-all"),
                                None,
                                self.channel_batch_kinds.is_empty(),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.channel_batch_kinds.clear();
                                cx.notify();
                            })),
                        )
                        .children(kinds.into_iter().enumerate().map(|(index, (kind, label))| {
                            components::compact_button(
                                ("managed-kind", index),
                                self.tr(label),
                                None,
                                self.channel_batch_kinds.contains(&kind),
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    if !this.channel_batch_kinds.remove(&kind) {
                                        this.channel_batch_kinds.insert(kind);
                                    }
                                    cx.notify();
                                },
                            ))
                        })),
                )
            })
            .when_some(self.managed_batch_status(), |body, (label, tone)| {
                body.child(
                    div()
                        .mt_2()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .min_w_0()
                                .text_xs()
                                .text_color(tone.foreground())
                                .debug_selector(|| "managed-download-status".into())
                                .child(label),
                        )
                        .when(self.managed_batch_active(), |row| {
                            row.child(
                                components::compact_button(
                                    "managed-download-stop",
                                    self.tr("managed-download-stop"),
                                    None,
                                    false,
                                )
                                .debug_selector(|| "managed-download-stop".into())
                                .disabled(self.managed_download_batch.as_ref().is_some_and(
                                    |batch| {
                                        batch.snapshot.phase
                                            == teleark_runtime::VaultDownloadBatchPhase::Stopping
                                    },
                                ))
                                .on_click(cx.listener(
                                    |this, _, _, cx| this.stop_managed_download_batch(cx),
                                )),
                            )
                        }),
                )
                .when(self.channel_batch_expanded, |body| {
                    body.child(self.render_managed_download_timeline())
                })
                .when(
                    self.managed_download_batch
                        .as_ref()
                        .is_some_and(|batch| batch.finished && batch.snapshot.failed > 0),
                    |row| {
                        row.child(
                            components::compact_button(
                                "managed-download-retry",
                                self.tr("managed-download-retry"),
                                None,
                                false,
                            )
                            .disabled(busy)
                            .on_click(
                                cx.listener(|this, _, _, cx| this.retry_managed_download_batch(cx)),
                            ),
                        )
                    },
                )
            })
            .into_any_element()
    }

    fn render_managed_download_timeline(&self) -> AnyElement {
        let Some(batch) = &self.managed_download_batch else {
            return div().into_any_element();
        };
        let snapshot = &batch.snapshot;
        div()
            .id("managed-download-timeline")
            .max_h(theme::ROW_HEIGHT * 3.0)
            .overflow_y_scroll()
            .text_xs()
            .when(snapshot.dropped_events > 0, |body| {
                body.child(self.tr_with(
                    "managed-download-history-truncated",
                    MessageArgs::new().with(
                        "count",
                        format_integer(self.locale(), snapshot.dropped_events),
                    ),
                ))
            })
            .children(snapshot.events.iter().rev().map(|event| {
                div().child(
                    self.tr_with(
                        "managed-download-event",
                        MessageArgs::new()
                            .with("time", self.sync_event_time(event.at).to_string())
                            .with(
                                "phase",
                                self.tr(match event.phase {
                                    teleark_runtime::VaultDownloadBatchPhase::Queued => {
                                        "transfer-state-waiting"
                                    }
                                    teleark_runtime::VaultDownloadBatchPhase::Restoring => {
                                        "transfer-state-downloading"
                                    }
                                    teleark_runtime::VaultDownloadBatchPhase::Stopping => {
                                        "managed-download-stop"
                                    }
                                    teleark_runtime::VaultDownloadBatchPhase::Completed => {
                                        "transfer-state-completed"
                                    }
                                    teleark_runtime::VaultDownloadBatchPhase::Cancelled => {
                                        "transfer-state-cancelled"
                                    }
                                    teleark_runtime::VaultDownloadBatchPhase::Failed => {
                                        "transfer-state-failed"
                                    }
                                })
                                .to_string(),
                            )
                            .with(
                                "error",
                                event.failure.map_or_else(String::new, |kind| {
                                    self.application_error_message(kind).to_string()
                                }),
                            ),
                    ),
                )
            }))
            .into_any_element()
    }
}
