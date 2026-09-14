//! Bounded upload presentation from Runtime's acknowledgement events.
use super::*;
use teleark_runtime::VaultUploadPartState;

impl TeleArkApp {
    pub(super) fn render_upload_activity(
        &self,
        activity: &VaultUploadActivity,
        active: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let replay = self.transfer_inspector_replay || !active;
        let count = activity.events.len();
        let cursor = self.transfer_replay_cursor.min(count.saturating_sub(1));
        let end = if replay {
            activity
                .events
                .get(cursor)
                .map(|event| event.at)
                .unwrap_or(activity.last_activity)
        } else {
            activity.last_activity
        };
        let elapsed = |at: std::time::Instant| {
            at.saturating_duration_since(activity.started)
                .as_millis()
                .min(u64::MAX as u128) as u64
        };
        let end_millis = elapsed(end);
        let samples = activity
            .samples
            .iter()
            .filter(|sample| !replay || sample.elapsed_millis <= end_millis)
            .collect::<Vec<_>>();
        let peak = samples
            .iter()
            .map(|sample| sample.bytes_per_second)
            .max()
            .unwrap_or(1)
            .max(1);
        let span = samples
            .iter()
            .map(|sample| sample.interval_millis)
            .sum::<u64>()
            .max(1);
        let bars = samples.iter().enumerate().map(|(index, sample)| {
            let label = self.tr_with(
                "upload-chart-sample",
                MessageArgs::new()
                    .with(
                        "time",
                        format_duration_millis(self.locale(), sample.elapsed_millis),
                    )
                    .with(
                        "interval",
                        format_duration_millis(self.locale(), sample.interval_millis),
                    )
                    .with(
                        "speed",
                        format_speed(self.locale(), sample.bytes_per_second),
                    ),
            );
            div()
                .id(("upload-speed-sample", index))
                .w(gpui_kit::relative(
                    sample.interval_millis as f32 / span as f32,
                ))
                .h(px(
                    (sample.bytes_per_second as f32 / peak as f32 * 64.0).max(1.0)
                ))
                .bg(theme::blue())
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(label.clone()).build(window, cx)
                })
        });
        let group_size = activity.parts.len().div_ceil(128).max(1);
        let parts = activity
            .parts
            .chunks(group_size)
            .enumerate()
            .map(|(index, group)| {
                let state = if group
                    .iter()
                    .any(|p| p.state == VaultUploadPartState::Waiting)
                {
                    VaultUploadPartState::Waiting
                } else if group
                    .iter()
                    .any(|p| p.state == VaultUploadPartState::Uploading)
                {
                    VaultUploadPartState::Uploading
                } else if group
                    .iter()
                    .all(|p| p.state == VaultUploadPartState::Acknowledged)
                {
                    VaultUploadPartState::Acknowledged
                } else {
                    VaultUploadPartState::Queued
                };
                let (tone, label) = match state {
                    VaultUploadPartState::Queued => (Tone::Neutral, "upload-part-queued"),
                    VaultUploadPartState::Uploading => (Tone::Blue, "upload-part-active"),
                    VaultUploadPartState::Waiting => (Tone::Amber, "upload-part-waiting"),
                    VaultUploadPartState::Acknowledged => (Tone::Green, "upload-part-confirmed"),
                };
                let text = self.tr_with(
                    "upload-part-group",
                    MessageArgs::new()
                        .with(
                            "first",
                            format_integer(self.locale(), (index * group_size + 1) as u64),
                        )
                        .with(
                            "last",
                            format_integer(
                                self.locale(),
                                (index * group_size + group.len()) as u64,
                            ),
                        )
                        .with(
                            "confirmed",
                            format_integer(
                                self.locale(),
                                group
                                    .iter()
                                    .filter(|p| p.state == VaultUploadPartState::Acknowledged)
                                    .count() as u64,
                            ),
                        )
                        .with("state", self.tr(label).to_string()),
                );
                div()
                    .id(("upload-part-cell", index))
                    .size(px(24.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_xs()
                    .bg(tone.background())
                    .text_color(tone.foreground())
                    .when(group_size == 1, |cell| {
                        cell.child(format_integer(self.locale(), (index + 1) as u64))
                    })
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(text.clone()).build(window, cx)
                    })
            });
        let selected = if replay {
            activity.events.get(cursor).into_iter().collect::<Vec<_>>()
        } else {
            activity.events.iter().rev().take(12).collect()
        };
        let events = selected.into_iter().enumerate().map(|(index, event)| {
            let phase = self.tr(if event.acknowledged {
                "upload-part-confirmed"
            } else {
                upload_phase_message_id(event.phase)
            });
            let label = self.tr_with(
                "upload-timeline-event",
                MessageArgs::new()
                    .with(
                        "time",
                        format_duration_millis(self.locale(), elapsed(event.at)),
                    )
                    .with("phase", phase.to_string())
                    .with(
                        "part",
                        event.part.map_or_else(
                            || self.tr("transfer-value-unavailable").to_string(),
                            |index| format_integer(self.locale(), u64::from(index) + 1),
                        ),
                    )
                    .with(
                        "attempt",
                        format_integer(self.locale(), u64::from(event.attempt)),
                    )
                    .with(
                        "wait",
                        format_duration_millis(self.locale(), event.wait_millis),
                    ),
            );
            components::list_summary(SharedString::from(format!("upload-event-{index}")), label)
        });
        let live = components::button(
            "upload-activity-live",
            self.tr("transfer-mode-live"),
            None,
            !replay,
        )
        .disabled(!active)
        .on_click(cx.listener(|this, _, _, cx| {
            this.transfer_inspector_replay = false;
            cx.notify();
        }));
        let replay_button = components::button(
            "upload-activity-replay",
            self.tr("transfer-mode-replay"),
            None,
            replay,
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            this.transfer_inspector_replay = true;
            this.transfer_replay_cursor = count.saturating_sub(1);
            cx.notify();
        }));
        let previous = components::button(
            "upload-event-previous",
            self.tr("transfer-replay-previous"),
            Some(IconName::ChevronLeft),
            false,
        )
        .disabled(!replay || cursor == 0)
        .on_click(cx.listener(|this, _, _, cx| {
            this.transfer_replay_cursor = this.transfer_replay_cursor.saturating_sub(1);
            cx.notify();
        }));
        let next = components::button(
            "upload-event-next",
            self.tr("transfer-replay-next"),
            Some(IconName::ChevronRight),
            false,
        )
        .disabled(!replay || cursor + 1 >= count)
        .on_click(cx.listener(move |this, _, _, cx| {
            this.transfer_replay_cursor =
                (this.transfer_replay_cursor + 1).min(count.saturating_sub(1));
            cx.notify();
        }));
        components::card()
            .p_3()
            .flex()
            .flex_col()
            .gap_3()
            .text_xs()
            .debug_selector(|| "upload-pipeline-inspector".to_owned())
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.tr("upload-pipeline-title")),
            )
            .child(div().flex().gap_2().child(live).child(replay_button))
            .child(
                self.tr_with(
                    "upload-pipeline-queue",
                    MessageArgs::new()
                        .with(
                            "queued",
                            format_integer(self.locale(), u64::from(activity.queued)),
                        )
                        .with(
                            "active",
                            format_integer(self.locale(), u64::from(activity.active)),
                        )
                        .with(
                            "idle",
                            format_duration_millis(
                                self.locale(),
                                if active {
                                    activity
                                        .last_activity
                                        .elapsed()
                                        .as_millis()
                                        .min(u64::MAX as u128)
                                        as u64
                                } else {
                                    0
                                },
                            ),
                        )
                        .with(
                            "wait",
                            format_duration_millis(
                                self.locale(),
                                activity.wait_until.map_or(0, |until| {
                                    until
                                        .saturating_duration_since(std::time::Instant::now())
                                        .as_millis()
                                        .min(u64::MAX as u128)
                                        as u64
                                }),
                            ),
                        ),
                ),
            )
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.tr("upload-chart-title")),
            )
            .when(samples.is_empty(), |card| {
                card.child(self.tr("upload-chart-empty"))
            })
            .when(!samples.is_empty(), |card| {
                card.child(format_speed(self.locale(), peak))
                    .child(div().h(px(64.0)).w_full().flex().items_end().children(bars))
                    .child(
                        self.tr_with(
                            "upload-chart-range",
                            MessageArgs::new()
                                .with(
                                    "from",
                                    format_duration_millis(
                                        self.locale(),
                                        samples.first().map_or(0, |sample| {
                                            sample
                                                .elapsed_millis
                                                .saturating_sub(sample.interval_millis)
                                        }),
                                    ),
                                )
                                .with(
                                    "to",
                                    format_duration_millis(
                                        self.locale(),
                                        samples.last().map_or(0, |sample| sample.elapsed_millis),
                                    ),
                                ),
                        ),
                    )
            })
            .child(self.tr("upload-chart-explanation"))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.tr("upload-part-map-title")),
            )
            .child(self.tr("upload-part-map-legend"))
            .when(group_size > 1, |card| {
                card.child(
                    self.tr_with(
                        "upload-part-map-grouping",
                        MessageArgs::new()
                            .with(
                                "count",
                                format_integer(self.locale(), activity.parts.len() as u64),
                            )
                            .with("size", format_integer(self.locale(), group_size as u64)),
                    ),
                )
            })
            .child(div().flex().flex_wrap().gap_1().children(parts))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.tr("upload-timeline-title")),
            )
            .when(
                activity.omitted_events > 0 || activity.omitted_samples > 0,
                |card| {
                    card.child(
                        self.tr_with(
                            "upload-history-omitted",
                            MessageArgs::new()
                                .with(
                                    "events",
                                    format_integer(self.locale(), activity.omitted_events),
                                )
                                .with(
                                    "samples",
                                    format_integer(self.locale(), activity.omitted_samples),
                                ),
                        ),
                    )
                },
            )
            .when(!replay && count > 12, |card| {
                card.child(self.tr("upload-timeline-recent"))
            })
            .children(events)
            .when(replay, |card| {
                card.child(div().flex().gap_2().child(previous).child(next))
            })
            .child(self.tr("settings-upload-resume-window"))
            .into_any_element()
    }
}
