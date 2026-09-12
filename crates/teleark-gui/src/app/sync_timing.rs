//! A visible-only presentation clock. Observes business events but never notifies the app entity.
use super::*;
use gpui_kit::WeakEntity;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use teleark_i18n::format::format_duration_millis;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct TimedText {
    pub id: &'static str,
    pub args: MessageArgs,
    pub times: Vec<(&'static str, Instant, bool)>,
    frozen_at: Option<Instant>,
}
impl TimedText {
    pub fn new(id: &'static str, args: MessageArgs) -> Self {
        Self {
            id,
            args,
            times: Vec::new(),
            frozen_at: None,
        }
    }
    pub fn elapsed(mut self, name: &'static str, at: Instant) -> Self {
        self.times.push((name, at, false));
        self
    }
    pub fn remaining(mut self, name: &'static str, at: Instant) -> Self {
        self.times.push((name, at, true));
        self
    }
    pub fn freeze_if(mut self, frozen: bool, at: Instant) -> Self {
        self.frozen_at = frozen.then_some(at);
        self
    }
    fn format(&self, app: &TeleArkApp, now: Instant) -> SharedString {
        let now = self.frozen_at.unwrap_or(now);
        let mut args = self.args.clone();
        for (name, at, remaining) in &self.times {
            let duration = if *remaining {
                at.saturating_duration_since(now)
            } else {
                now.saturating_duration_since(*at)
            };
            // Whole seconds are sufficient for human-facing phase timing. Older
            // activity ages advance by minutes; retry countdowns retain seconds.
            let seconds = if *remaining {
                duration
                    .as_secs()
                    .saturating_add(u64::from(duration.subsec_nanos() > 0))
            } else {
                duration.as_secs()
            };
            let seconds = if !remaining && *name == "activity" && seconds >= 60 {
                seconds / 60 * 60
            } else {
                seconds
            };
            args.set(
                name,
                format_duration_millis(app.locale(), seconds.saturating_mul(1000)),
            );
        }
        app.tr_with(self.id, args)
    }
}

pub(super) struct SyncTiming {
    owner: WeakEntity<TeleArkApp>,
    model: Vec<TimedText>,
    text: Vec<SharedString>,
    locale: SupportedLocale,
    active: bool,
    running: bool,
    timer: Option<Task<()>>,
    _activation: Subscription,
    _changes: Subscription,
    #[cfg(test)]
    pub ticks: usize,
}
impl SyncTiming {
    pub fn new(owner: Entity<TeleArkApp>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let activation = cx.observe_window_activation(window, |this, window, cx| {
            this.active = window.is_window_active();
            this.refresh(Instant::now(), cx);
            this.schedule(cx);
        });
        let changes = cx.observe(&owner, |this, owner, cx| {
            let app = owner.read(cx);
            let model = app.sync_timing_model();
            let text = Self::formatted(&model, app, Instant::now());
            let locale = app.locale();
            let running = app.sync_clock_needed();
            this.set(model, text, locale, running, cx);
        });
        Self {
            owner: owner.downgrade(),
            _changes: changes,
            model: Vec::new(),
            text: Vec::new(),
            locale: SupportedLocale::EnUs,
            active: window.is_window_active(),
            running: false,
            timer: None,
            _activation: activation,
            #[cfg(test)]
            ticks: 0,
        }
    }
    // Called from the parent's event-driven render. Formatting happens before
    // this update, so there is no nested read of a mutably borrowed parent.
    pub fn set(
        &mut self,
        model: Vec<TimedText>,
        text: Vec<SharedString>,
        locale: SupportedLocale,
        running: bool,
        cx: &mut Context<Self>,
    ) {
        if self.model != model || self.text != text || self.locale != locale {
            self.model = model;
            self.text = text;
            self.locale = locale;
            cx.notify();
        }
        self.running = running;
        self.schedule(cx);
    }
    pub fn formatted(model: &[TimedText], app: &TeleArkApp, now: Instant) -> Vec<SharedString> {
        model.iter().map(|line| line.format(app, now)).collect()
    }
    fn refresh(&mut self, now: Instant, cx: &mut Context<Self>) {
        let Some(owner) = self.owner.upgrade() else {
            self.running = false;
            return;
        };
        let text = Self::formatted(&self.model, owner.read(cx), now);
        if text != self.text {
            self.text = text;
            cx.notify();
        }
    }
    fn schedule(&mut self, cx: &mut Context<Self>) {
        if !self.active || !self.running {
            self.timer = None;
            return;
        }
        if self.timer.is_some() {
            return;
        }
        self.timer = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let Ok(keep) = this.update(cx, |this, cx| {
                    if !this.active || !this.running {
                        return false;
                    }
                    #[cfg(test)]
                    {
                        this.ticks += 1;
                    }
                    this.refresh(Instant::now(), cx);
                    this.active && this.running
                }) else {
                    break;
                };
                if !keep {
                    break;
                }
            }
        }));
    }
}
impl Render for SyncTiming {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("sync-live-timing")
            .flex()
            .flex_col()
            .gap_3()
            .children(self.text.iter().cloned())
    }
}

/// Map monotonic event instants to a fixed wall-clock anchor once per app.
/// Subsequent renders and wall-clock adjustments cannot age historical rows.
pub(super) struct TimeAnchor {
    instant: Instant,
    unix_millis: i64,
}
impl TimeAnchor {
    pub fn new() -> Self {
        Self {
            instant: Instant::now(),
            unix_millis: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()
                .and_then(|d| i64::try_from(d.as_millis()).ok())
                .unwrap_or(0),
        }
    }
    pub fn unix_millis(&self, at: Instant) -> i64 {
        if at >= self.instant {
            self.unix_millis.saturating_add(
                i64::try_from(at.duration_since(self.instant).as_millis()).unwrap_or(i64::MAX),
            )
        } else {
            self.unix_millis.saturating_sub(
                i64::try_from(self.instant.duration_since(at).as_millis()).unwrap_or(i64::MAX),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn event_anchor_is_fixed_and_handles_instants_on_both_sides() {
        let now = Instant::now();
        let anchor = TimeAnchor {
            instant: now,
            unix_millis: 100_000,
        };
        assert_eq!(anchor.unix_millis(now - Duration::from_secs(3)), 97_000);
        assert_eq!(anchor.unix_millis(now + Duration::from_secs(7)), 107_000);
        assert_eq!(anchor.unix_millis(now - Duration::from_secs(3)), 97_000);
    }

    #[gpui_kit::test]
    fn timing_quantizes_seconds_and_freezes_terminal_durations(cx: &mut gpui_kit::TestAppContext) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Account);
        let now = Instant::now();
        app.update(cx, |app, _| {
            let elapsed = TimedText::new("channel-sync-retry-after", MessageArgs::new())
                .elapsed("duration", now);
            assert_eq!(
                elapsed.format(app, now + Duration::from_millis(1200)),
                elapsed.format(app, now + Duration::from_millis(1900))
            );
            assert_ne!(
                elapsed.format(app, now + Duration::from_millis(1900)),
                elapsed.format(app, now + Duration::from_secs(2))
            );
            let frozen = elapsed.freeze_if(true, now + Duration::from_secs(3));
            assert_eq!(
                frozen.format(app, now + Duration::from_secs(4)),
                frozen.format(app, now + Duration::from_secs(400))
            );
            let retry = TimedText::new("channel-sync-retry-after", MessageArgs::new())
                .remaining("duration", now + Duration::from_secs(3));
            assert_eq!(
                retry.format(app, now + Duration::from_millis(10)),
                retry.format(app, now + Duration::from_millis(800))
            );
        });
    }

    #[gpui_kit::test]
    fn timing_updates_reuse_page_and_history_and_stop_when_hidden(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Transfers);
        app.update(cx, |app, cx| {
            app.channel_sync_snapshot = Some(super::super::channel_sync::tests::fixture_snapshot());
            app.channel_sync_details = true;
            cx.notify();
        });
        cx.run_until_parked();
        let timing = app
            .update(cx, |app, _| app.sync_timing.clone())
            .expect("visible clock");
        let workspace = app
            .update(cx, |app, _| app.page_view.clone())
            .expect("page");
        let history = app
            .update(cx, |app, _| app.sync_history.clone())
            .expect("history");
        let now = Instant::now();
        let model = vec![
            TimedText::new("channel-sync-timing", MessageArgs::new())
                .elapsed("duration", now)
                .elapsed("activity", now),
        ];
        let text = app.update(cx, |app, _| SyncTiming::formatted(&model, app, now));
        timing.update(cx, |clock, cx| {
            clock.active = true;
            clock.set(model, text, SupportedLocale::EnUs, true, cx);
        });
        cx.run_until_parked();
        let notifications = std::rc::Rc::new(std::cell::Cell::new(0));
        let observed = notifications.clone();
        let _observe =
            cx.update(|_, cx| cx.observe(&app, move |_, _| observed.set(observed.get() + 1)));
        cx.run_until_parked();
        let renders = workspace.update(cx, |v, _| v.renders);
        let rows = history.update(cx, |v, _| v.materialized.get());
        assert!(
            rows > 0 && rows < 128,
            "only visible history rows are built"
        );
        timing.update(cx, |clock, cx| {
            clock.refresh(now + Duration::from_secs(2), cx)
        });
        cx.run_until_parked();
        assert_eq!(
            notifications.get(),
            0,
            "a clock tick must not notify the app"
        );
        assert_eq!(
            workspace.update(cx, |v, _| v.renders),
            renders,
            "a clock tick must reuse the page"
        );
        assert_eq!(
            history.update(cx, |v, _| v.materialized.get()),
            rows,
            "a clock tick must reuse static history"
        );
        let ticks = timing.update(cx, |v, _| v.ticks);
        timing.update(cx, |clock, cx| {
            clock.active = false;
            clock.schedule(cx);
        });
        cx.background_executor.advance_clock(Duration::from_secs(5));
        cx.run_until_parked();
        assert_eq!(
            timing.update(cx, |v, _| v.ticks),
            ticks,
            "inactive windows have no timer wakeups"
        );
        timing.update(cx, |clock, cx| {
            clock.active = true;
            clock.refresh(now + Duration::from_secs(8), cx);
            clock.schedule(cx);
        });
        assert!(timing.update(cx, |v, _| v.timer.is_some()));
        assert!(timing.update(cx, |v, _| {
            v.text[0].contains(&format_duration_millis(SupportedLocale::EnUs, 8_000))
        }));
        app.update(cx, |app, cx| {
            app.channel_sync_snapshot.as_mut().expect("snapshot").phase =
                teleark_runtime::ChannelSyncPhase::Idle;
            cx.notify();
        });
        cx.run_until_parked();
        assert!(
            timing.update(cx, |v, _| v.timer.is_none()),
            "completion stops the timer on its event"
        );
        app.update(cx, |app, cx| {
            app.channel_sync_snapshot.as_mut().expect("snapshot").phase =
                teleark_runtime::ChannelSyncPhase::Receiving;
            cx.notify();
        });
        cx.run_until_parked();
        assert!(
            timing.update(cx, |v, _| v.timer.is_some()),
            "new work restarts visible timing on its event"
        );
        let weak = timing.downgrade();
        app.update(cx, |app, cx| {
            app.channel_sync_details = false;
            cx.notify();
        });
        drop(timing);
        cx.run_until_parked();
        cx.background_executor.advance_clock(Duration::from_secs(3));
        cx.run_until_parked();
        assert!(
            weak.upgrade().is_none(),
            "closing details releases the clock and timer"
        );
    }
}
