//! Retained presentation for the shared payload budgets; no disk or network I/O.
use super::*;
use gpui_kit::{
    StatefulInteractiveElement as _,
    base::Disableable as _,
    component::{input::Input, scroll::ScrollableElement as _},
    rgba,
};
use teleark_runtime::{BandwidthEventKind, BandwidthSnapshot, TransferSpeedLimits};

pub(crate) struct SpeedLimitsUi {
    pub open: bool,
    upload: Entity<InputState>,
    download: Entity<InputState>,
    invalid: bool,
    history: bool,
    snapshots: Option<(BandwidthSnapshot, BandwidthSnapshot)>,
    observer: Option<Task<()>>,
    scroll: gpui_kit::ScrollHandle,
}
impl SpeedLimitsUi {
    pub(super) fn new(
        limits: TransferSpeedLimits,
        window: &mut Window,
        cx: &mut Context<TeleArkApp>,
    ) -> Self {
        let mut input = |bytes: u64| {
            cx.new(|cx| {
                let mut input = InputState::new(window, cx);
                input.set_value(bytes.div_ceil(1024).to_string(), window, cx);
                input
            })
        };
        Self {
            open: false,
            upload: input(limits.upload),
            download: input(limits.download),
            invalid: false,
            history: false,
            snapshots: None,
            observer: None,
            scroll: gpui_kit::ScrollHandle::new(),
        }
    }
}
fn parse_limit(value: &str) -> Option<u64> {
    let value = value.trim();
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse::<u64>().ok()?.checked_mul(1024)
}
impl TeleArkApp {
    pub(crate) fn preview_speed_limits(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.visual_preview {
            return;
        }
        self.preferences.speed_limits = TransferSpeedLimits {
            upload: 262_144,
            download: 1_048_576,
        };
        let now = std::time::Instant::now();
        let snapshot = |rate| BandwidthSnapshot {
            bytes_per_second: rate,
            waiting: 3,
            waiting_millis: 12000,
            last_activity_millis: Some(5000),
            revision: 42,
            events: (0..16)
                .map(|index| teleark_runtime::BandwidthEvent {
                    sequence: index + 27,
                    kind: match index % 3 {
                        0 => BandwidthEventKind::LimitChanged,
                        1 => BandwidthEventKind::Resumed,
                        _ => BandwidthEventKind::Waiting,
                    },
                    at: (now - Duration::from_secs(32 - index)).into(),
                })
                .collect(),
            omitted_events: 26,
        };
        self.speed_limits.snapshots = Some((snapshot(262_144), snapshot(1_048_576)));
        self.open_speed_limits(window, cx);
        self.speed_limits.history = true;
    }
    pub(crate) fn speed_limits_button(
        &self,
        id: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        components::button(
            id,
            self.tr("speed-limits-title"),
            Some(IconName::Settings2),
            false,
        )
        .debug_selector(move || id.into())
        .on_click(cx.listener(|this, _, window, cx| this.open_speed_limits(window, cx)))
        .into_any_element()
    }
    fn open_speed_limits(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for (input, value) in [
            (
                &self.speed_limits.upload,
                self.preferences.speed_limits.upload,
            ),
            (
                &self.speed_limits.download,
                self.preferences.speed_limits.download,
            ),
        ] {
            input.update(cx, |input, cx| {
                input.set_value(value.div_ceil(1024).to_string(), window, cx)
            });
        }
        self.speed_limits.invalid = false;
        self.speed_limits
            .scroll
            .set_offset(gpui_kit::point(px(0.0), px(0.0)));
        self.speed_limits.open = true;
        cx.notify();
    }
    fn save_speed_limits(&mut self, cx: &mut Context<Self>) {
        if self.preference_persistence == PreferencePersistence::Saving {
            return;
        }
        let limits = parse_limit(&self.speed_limits.upload.read(cx).value())
            .zip(parse_limit(&self.speed_limits.download.read(cx).value()));
        let Some((upload, download)) = limits else {
            self.speed_limits.invalid = true;
            cx.notify();
            return;
        };
        self.speed_limits.invalid = false;
        self.preferences.speed_limits = TransferSpeedLimits { upload, download };
        self.persist_preferences(cx);
        if self.visual_preview {
            self.preference_persistence = PreferencePersistence::Saved;
        }
        cx.notify();
    }
    pub(super) fn start_bandwidth_observer(&mut self, cx: &mut Context<Self>) {
        let Some(library) = self.library.clone() else {
            return;
        };
        self.speed_limits.snapshots = Some(library.bandwidth_snapshot());
        self.speed_limits.observer = Some(cx.spawn(async move |this, cx| {
            loop {
                // Presentation sampling only: bounded memory snapshots, never service/SQL polling.
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                let snapshots = library.bandwidth_snapshot();
                let Some(this) = this.upgrade() else {
                    break;
                };
                this.update(cx, |this, cx| {
                    let old = this.speed_limits.snapshots.as_ref();
                    let changed = old.is_none_or(|old| {
                        old.0.revision != snapshots.0.revision
                            || old.1.revision != snapshots.1.revision
                    });
                    let ticking = snapshots.0.waiting > 0
                        || snapshots.1.waiting > 0
                        || (this.speed_limits.open && this.speed_limits.history);
                    this.speed_limits.snapshots = Some(snapshots);
                    if changed || ticking {
                        cx.notify();
                    }
                });
            }
        }));
    }
    fn limit_label(&self, rate: u64) -> SharedString {
        if rate == 0 {
            self.tr("speed-limits-unlimited")
        } else {
            format_speed(self.locale(), rate).into()
        }
    }
    pub(super) fn render_bandwidth_status(&self, cx: &mut Context<Self>) -> AnyElement {
        let limits = self.speed_limits.snapshots.as_ref().map_or(
            self.preferences.speed_limits,
            |(up, down)| TransferSpeedLimits {
                upload: up.bytes_per_second,
                download: down.bytes_per_second,
            },
        );
        let waiting = self
            .speed_limits
            .snapshots
            .as_ref()
            .map_or(0, |(up, down)| up.waiting + down.waiting);
        if limits == TransferSpeedLimits::default() && waiting == 0 {
            return div().into_any_element();
        }
        div()
            .debug_selector(|| "speed-limits-status".into())
            .flex_none()
            .px_3()
            .py_1()
            .flex()
            .items_center()
            .gap_2()
            .bg(theme::blue_pale())
            .text_xs()
            .child(
                div().flex_1().child(
                    self.tr_with(
                        "speed-limits-summary",
                        MessageArgs::new()
                            .with("upload", self.limit_label(limits.upload).to_string())
                            .with("download", self.limit_label(limits.download).to_string())
                            .with("waiting", waiting as u64)
                            .with(
                                "seconds",
                                self.speed_limits
                                    .snapshots
                                    .as_ref()
                                    .map_or(0, |(up, down)| {
                                        up.waiting_millis.max(down.waiting_millis) / 1000
                                    }),
                            ),
                    ),
                ),
            )
            .when_some(
                self.speed_limits.snapshots.as_ref().filter(|_| waiting > 0),
                |bar, (up, down)| {
                    let age = up
                        .last_activity_millis
                        .into_iter()
                        .chain(down.last_activity_millis)
                        .min();
                    bar.child(div().max_w(px(180.0)).child(match age {
                        Some(age) => self.tr_with(
                            "speed-limits-last-activity",
                            MessageArgs::new().with("seconds", age / 1000),
                        ),
                        None => self.tr("speed-limits-no-activity"),
                    }))
                },
            )
            .child(self.speed_limits_button("global-speed-limits", cx))
            .into_any_element()
    }
    fn render_budget_detail(&self, id: &'static str, snapshot: &BandwidthSnapshot) -> AnyElement {
        let mut panel = div().text_xs().child(self.tr(id)).child(
            self.tr_with(
                "speed-limits-budget",
                MessageArgs::new()
                    .with(
                        "limit",
                        self.limit_label(snapshot.bytes_per_second).to_string(),
                    )
                    .with("waiting", snapshot.waiting as u64)
                    .with("seconds", snapshot.waiting_millis / 1000),
            ),
        );
        panel = panel.child(match snapshot.last_activity_millis {
            Some(age) => self.tr_with(
                "speed-limits-last-activity",
                MessageArgs::new().with("seconds", age / 1000),
            ),
            None => self.tr("speed-limits-no-activity"),
        });
        if self.speed_limits.history {
            for event in &snapshot.events {
                let kind = match event.kind {
                    BandwidthEventKind::LimitChanged => "speed-limits-event-changed",
                    BandwidthEventKind::Waiting => "speed-limits-event-waiting",
                    BandwidthEventKind::Resumed => "speed-limits-event-resumed",
                };
                panel = panel.child(
                    self.tr_with(
                        "speed-limits-event",
                        MessageArgs::new()
                            .with("event", self.tr(kind).to_string())
                            .with("seconds", event.at.elapsed().as_secs()),
                    ),
                );
            }
            if snapshot.omitted_events > 0 {
                panel = panel.child(self.tr_with(
                    "speed-limits-omitted",
                    MessageArgs::new().with("count", snapshot.omitted_events),
                ));
            }
        }
        panel.into_any_element()
    }
    pub(super) fn render_speed_limits(&self, cx: &mut Context<Self>) -> AnyElement {
        let saving = self.preference_persistence == PreferencePersistence::Saving;
        let mut body = div()
            .id("speed-limits-content")
            .debug_selector(|| "speed-limits-content".into())
            .max_h(px(340.0))
            .overflow_y_scroll()
            .track_scroll(&self.speed_limits.scroll)
            .relative()
            .flex()
            .flex_col()
            .gap_3()
            .child(self.tr("speed-limits-description"));
        for (id, input) in [
            ("speed-limits-upload", &self.speed_limits.upload),
            ("speed-limits-download", &self.speed_limits.download),
        ] {
            body = body.child(
                div()
                    .child(self.tr(id))
                    .child(Input::new(input).h(px(36.0)).disabled(saving)),
            );
        }
        body = body.child(
            div()
                .text_xs()
                .text_color(theme::text_secondary())
                .child(self.tr("speed-limits-scope")),
        );
        if self.speed_limits.invalid {
            body = body.child(self.tr("speed-limits-invalid"));
        }
        let status = match self.preference_persistence {
            PreferencePersistence::Saving => "settings-preferences-saving",
            PreferencePersistence::Saved => "settings-preferences-saved",
            PreferencePersistence::Failed => "speed-limits-save-failed",
            PreferencePersistence::Idle => "speed-limits-ready",
        };
        body = body.child(self.tr(status));
        if let Some((up, down)) = &self.speed_limits.snapshots {
            body = body
                .child(self.render_budget_detail("speed-limits-upload", up))
                .child(self.render_budget_detail("speed-limits-download", down))
                .child(
                    components::button(
                        "speed-limits-history",
                        self.tr("speed-limits-history"),
                        None,
                        false,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.speed_limits.history = !this.speed_limits.history;
                        cx.notify();
                    })),
                );
        }
        let body = body.vertical_scrollbar(&self.speed_limits.scroll);
        let popup = components::card()
            .occlude()
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .w(px(520.0))
            .max_w_full()
            .p_5()
            .child(components::section_title(self.tr("speed-limits-title")))
            .child(div().mt_3().child(body))
            .child(
                div()
                    .mt_4()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        components::button(
                            "speed-limits-close",
                            self.tr("speed-limits-close"),
                            None,
                            false,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.speed_limits.open = false;
                            cx.notify();
                        })),
                    )
                    .child(
                        components::button("speed-limits-save", self.tr("common-save"), None, true)
                            .debug_selector(|| "speed-limits-save".into())
                            .disabled(saving)
                            .on_click(cx.listener(|this, _, _, cx| this.save_speed_limits(cx))),
                    ),
            );
        let cancel = cx.listener(|this, _, _, cx| {
            this.speed_limits.open = false;
            cx.notify();
        });
        gpui_kit::base::Dialog::new(cx)
            .focus_handle(self.modal_focus.clone())
            .flex()
            .items_center()
            .justify_center()
            .backdrop(div().absolute().inset_0().bg(rgba(0x18203366)))
            .popup(popup)
            .close_on_backdrop_press(false)
            .on_cancel(move |event, window, cx| {
                cancel(event, window, cx);
                false
            })
            .into_any_element()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn speed_input_is_integer_kib_per_second_and_rejects_overflow() {
        assert_eq!(parse_limit("0"), Some(0));
        assert_eq!(parse_limit(" 1024 "), Some(1_048_576));
        for invalid in ["", "-1", "1.5", "NaN", "1MiB", "18446744073709551615"] {
            assert_eq!(parse_limit(invalid), None);
        }
    }
    #[gpui_kit::test]
    fn english_speed_editor_validates_saves_and_keeps_controls_reachable(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Transfers);
        for (width, height) in [(900.0, 600.0), (1120.0, 680.0)] {
            cx.simulate_resize(gpui_kit::size(px(width), px(height)));
            for appearance in [AppearancePreference::Light, AppearancePreference::Dark] {
                cx.update(|window, cx| {
                    app.update(cx, |app, cx| {
                        assert_eq!(app.locale(), SupportedLocale::EnUs);
                        theme::apply_appearance(appearance, window, cx);
                        app.open_speed_limits(window, cx);
                        app.speed_limits
                            .upload
                            .update(cx, |input, cx| input.set_value("-1", window, cx));
                        app.save_speed_limits(cx);
                        assert!(app.speed_limits.invalid);
                        assert_eq!(app.preferences.speed_limits.upload, 0);
                        app.speed_limits
                            .upload
                            .update(cx, |input, cx| input.set_value("1024", window, cx));
                        app.speed_limits
                            .download
                            .update(cx, |input, cx| input.set_value("256", window, cx));
                        app.save_speed_limits(cx);
                        assert!(!app.speed_limits.invalid);
                        assert_eq!(
                            app.preferences.speed_limits,
                            TransferSpeedLimits {
                                upload: 1_048_576,
                                download: 262_144
                            }
                        );
                        assert_eq!(app.preference_persistence, PreferencePersistence::Saved);
                        app.preview_speed_limits(window, cx);
                    })
                });
                cx.run_until_parked();
                for id in ["speed-limits-content", "speed-limits-save"] {
                    let bounds = cx.debug_bounds(id).expect("visible editor");
                    assert!(bounds.left() >= px(0.0) && bounds.right() <= px(width));
                    assert!(bounds.top() >= px(0.0) && bounds.bottom() <= px(height));
                }
                let viewport = cx
                    .debug_bounds("speed-limits-content")
                    .expect("history viewport");
                let before = app.read_with(cx, |app, _| app.transfer_scroll.logical_scroll_top());
                for delta in [-120.0, -10000.0, -120.0, 10000.0, 120.0] {
                    cx.simulate_event(gpui_kit::ScrollWheelEvent {
                        position: viewport.center(),
                        delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.0), px(delta))),
                        ..Default::default()
                    });
                    cx.run_until_parked();
                    app.read_with(cx, |app, _| {
                        let after = app.transfer_scroll.logical_scroll_top();
                        assert_eq!(
                            (before.item_ix, before.offset_in_item),
                            (after.item_ix, after.offset_in_item),
                            "modal scrolling must not reach the transfer list"
                        );
                        if delta == -120.0 {
                            assert!(app.speed_limits.scroll.offset().y < px(0.0));
                        }
                    });
                    assert!(
                        cx.debug_bounds("speed-limits-save")
                            .expect("pinned Save")
                            .bottom()
                            <= px(height)
                    );
                }
                cx.update(|window, cx| {
                    app.update(cx, |app, cx| {
                        app.speed_limits
                            .upload
                            .update(cx, |input, cx| input.set_value("0", window, cx));
                        app.speed_limits
                            .download
                            .update(cx, |input, cx| input.set_value("0", window, cx));
                        app.save_speed_limits(cx);
                        assert_eq!(app.preferences.speed_limits, TransferSpeedLimits::default());
                        app.speed_limits.open = false;
                        cx.notify();
                    })
                });
            }
        }
    }
    #[gpui_kit::test]
    fn bandwidth_wait_feedback_survives_navigation_and_saving_blocks_reentry(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Transfers);
        cx.simulate_resize(gpui_kit::size(px(900.0), px(600.0)));
        let snapshot = BandwidthSnapshot {
            bytes_per_second: 1024,
            waiting: 3,
            waiting_millis: 12000,
            last_activity_millis: Some(5000),
            revision: 1,
            events: Vec::new(),
            omitted_events: 50,
        };
        app.update(cx, |app, cx| {
            app.speed_limits.snapshots = Some((snapshot.clone(), snapshot));
            app.preference_persistence = PreferencePersistence::Saving;
            let before = app.preferences.clone();
            app.save_speed_limits(cx);
            assert_eq!(app.preferences, before);
            assert_eq!(app.preference_persistence, PreferencePersistence::Saving);
        });
        for page in [Page::Transfers, Page::Storage, Page::Settings] {
            app.update(cx, |app, cx| {
                app.page = page;
                cx.notify();
            });
            cx.run_until_parked();
            let status = cx
                .debug_bounds("speed-limits-status")
                .expect("global wait feedback");
            assert!(status.bottom() <= px(600.0) && status.right() <= px(900.0));
        }
    }
}
