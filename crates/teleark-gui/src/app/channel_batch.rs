//! Presentation of one bounded, cancellable filter admission operation.
use super::*;
use teleark_i18n::format::format_integer;
use teleark_runtime::{
    ChannelBatchFilter, ChannelBatchPreparation, ChannelBatchPreparationPhase as Phase,
    ChannelBatchPreparationSnapshot, FilteredChannelBatch,
};

pub(crate) struct FilteredBatchUi {
    pub account_id: i64,
    pub chat_id: i64,
    pub progress: ChannelBatchPreparation,
    pub snapshot: ChannelBatchPreparationSnapshot,
    pub result: Option<Result<Option<FilteredChannelBatch>, teleark_core::ApplicationErrorKind>>,
}

impl FilteredBatchUi {
    pub fn active(&self) -> bool {
        self.result.is_none()
    }
}

impl Drop for FilteredBatchUi {
    fn drop(&mut self) {
        // Dropping presentation never joins or waits for its background owner.
        self.progress.cancel();
    }
}

impl TeleArkApp {
    pub(crate) fn filtered_batch_active(&self) -> bool {
        self.filtered_channel_batch
            .as_ref()
            .is_some_and(FilteredBatchUi::active)
    }

    pub(crate) fn download_channel_filter(&mut self, cx: &mut Context<Self>) {
        if self.filtered_batch_active()
            || self.channel_batch_activity == ChannelBatchActivity::Preparing
            || !self.telegram_is_authorized()
        {
            return;
        }
        let (Some(transfers), Some(account), Some(chat_id)) = (
            self.transfers.clone(),
            self.telegram_account.as_ref(),
            self.selected_chat_id,
        ) else {
            return;
        };
        let account_id = account.id;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|time| time.as_millis().min(i64::MAX as u128) as i64)
            .unwrap_or(0);
        let age = match self.channel_batch_period {
            ChannelBatchPeriod::AnyTime => None,
            ChannelBatchPeriod::Past24Hours => Some(86_400_000),
            ChannelBatchPeriod::Past7Days => Some(7 * 86_400_000),
            ChannelBatchPeriod::Past30Days => Some(30 * 86_400_000),
        };
        let filter = ChannelBatchFilter {
            account_id,
            chat_id,
            latest_unix_ms: now,
            earliest_unix_ms: age.map(|age| now.saturating_sub(age)),
            kinds: self.channel_batch_kinds.iter().copied().collect(),
        };
        let progress = ChannelBatchPreparation::default();
        let mut changes = progress.subscribe();
        self.filtered_channel_batch = progress.snapshot().map(|snapshot| FilteredBatchUi {
            account_id,
            chat_id,
            progress: progress.clone(),
            snapshot,
            result: None,
        });
        // Acknowledgment is visible before any database or filesystem work starts.
        cx.notify();
        let work = cx.background_spawn({
            let progress = progress.clone();
            async move { transfers.enqueue_filtered_channel_batch(filter, &progress) }
        });
        self.filtered_channel_batch_task = Some(cx.spawn(async move |this, cx| {
            let mut work = work;
            let mut presented_second = 0;
            loop {
                // Phase and count revisions repaint immediately. Only the
                // displayed elapsed label needs a one-second clock.
                let result = tokio::select! {
                    result = &mut work => Some(result),
                    changed = changes.changed() => {
                        if changed.is_err() { return; }
                        None
                    },
                    () = cx.background_executor().timer(Duration::from_secs(1)) => None,
                };
                let Some(this) = this.upgrade() else {
                    progress.cancel();
                    return;
                };
                let done = result.is_some();
                this.update(cx, |app, cx| {
                    if app.telegram_account.as_ref().map(|account| account.id) != Some(account_id) {
                        progress.cancel();
                        // Keep the retained admission owner until its blocked
                        // dependency drains; a new account cannot replace it.
                        if done {
                            app.filtered_channel_batch = None;
                        }
                        cx.notify();
                        return;
                    }
                    if let Some(ui) = app.filtered_channel_batch.as_mut() {
                        let mut changed = done;
                        if let Some(snapshot) = progress.snapshot() {
                            let second = snapshot.started_at.elapsed().as_secs();
                            changed |= snapshot.phase != ui.snapshot.phase
                                || snapshot.examined != ui.snapshot.examined
                                || snapshot.matched != ui.snapshot.matched
                                || second != presented_second;
                            presented_second = second;
                            ui.snapshot = snapshot;
                        }
                        if let Some(result) = result {
                            ui.result = Some(result.map_err(|error| error.kind()));
                        }
                        if changed {
                            cx.notify();
                        }
                    }
                });
                if done {
                    break;
                }
            }
        }));
    }

    pub(crate) fn cancel_filtered_channel_batch(&mut self, cx: &mut Context<Self>) {
        if let Some(ui) = &mut self.filtered_channel_batch {
            ui.progress.cancel();
            if let Some(snapshot) = ui.progress.snapshot() {
                ui.snapshot = snapshot;
            }
        }
        cx.notify();
    }

    pub(crate) fn filtered_batch_status(&self) -> Option<(SharedString, Tone)> {
        let ui = self.filtered_channel_batch.as_ref()?;
        if self.telegram_account.as_ref().map(|account| account.id) != Some(ui.account_id) {
            return None;
        }
        let id = match &ui.result {
            Some(Ok(Some(batch))) => {
                return Some((
                    self.tr_with(
                        "channel-filter-batch-queued",
                        MessageArgs::new()
                            .with("count", format_integer(self.locale(), batch.count as u64))
                            .with(
                                "folder",
                                batch
                                    .directory
                                    .file_name()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .into_owned(),
                            ),
                    ),
                    Tone::Green,
                ));
            }
            Some(Ok(None)) => "channel-filter-batch-no-matches",
            Some(Err(teleark_core::ApplicationErrorKind::Capacity)) => {
                "channel-filter-batch-capacity"
            }
            Some(Err(teleark_core::ApplicationErrorKind::Cancelled)) => {
                "channel-filter-batch-cancelled"
            }
            Some(Err(_)) => "channel-filter-batch-failed",
            None => match ui.snapshot.phase {
                Phase::Discovering => "channel-filter-batch-discovering",
                Phase::PreparingFolder => "channel-filter-batch-folder",
                Phase::Queuing => "channel-filter-batch-admitting",
                Phase::Cancelled => "channel-filter-batch-cancelled",
                Phase::Completed => "channel-filter-batch-phase-complete",
                Phase::Failed(_) => "channel-filter-batch-failed",
            },
        };
        let tone = match &ui.result {
            Some(Err(_)) => Tone::Amber,
            Some(Ok(_)) => Tone::Green,
            None => Tone::Blue,
        };
        Some((
            self.tr_with(
                id,
                MessageArgs::new()
                    .with(
                        "examined",
                        format_integer(self.locale(), ui.snapshot.examined),
                    )
                    .with(
                        "matched",
                        format_integer(self.locale(), ui.snapshot.matched as u64),
                    )
                    .with(
                        "seconds",
                        format_integer(
                            self.locale(),
                            ui.snapshot.phase_started_at.elapsed().as_secs(),
                        ),
                    )
                    .with(
                        "idle",
                        format_integer(
                            self.locale(),
                            ui.snapshot.last_activity_at.elapsed().as_secs(),
                        ),
                    ),
            ),
            tone,
        ))
    }

    pub(crate) fn filtered_batch_timeline(&self) -> SharedString {
        let Some(ui) = &self.filtered_channel_batch else {
            return "".into();
        };
        ui.snapshot
            .events
            .iter()
            .map(|(phase, at)| {
                let phase = self.tr(match phase {
                    Phase::Discovering => "channel-filter-batch-phase-discovery",
                    Phase::PreparingFolder => "channel-filter-batch-phase-folder",
                    Phase::Queuing => "channel-filter-batch-phase-queue",
                    Phase::Completed => "channel-filter-batch-phase-complete",
                    Phase::Cancelled => "channel-filter-batch-cancelled",
                    Phase::Failed(_) => "channel-filter-batch-failed",
                });
                self.tr_with(
                    "channel-filter-batch-event",
                    MessageArgs::new().with("phase", phase.to_string()).with(
                        "seconds",
                        format_integer(
                            self.locale(),
                            at.saturating_duration_since(ui.snapshot.started_at)
                                .as_secs(),
                        ),
                    ),
                )
                .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
            .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{TestAppContext, size};

    #[gpui_kit::test]
    fn filter_batch_is_available_without_selection_and_feedback_survives_navigation(
        cx: &mut TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Channel);
        for full in [false, true] {
            cx.simulate_resize(size(
                px(if full { 1440.0 } else { 900.0 }),
                px(if full { 900.0 } else { 600.0 }),
            ));
            cx.update(|window, cx| {
                if window.is_fullscreen() != full {
                    window.toggle_fullscreen();
                }
                assert_eq!(window.is_fullscreen(), full);
                app.update(cx, |app, cx| {
                    theme::apply_appearance(AppearancePreference::Light, window, cx);
                    app.set_page(Page::Channel, cx);
                    app.selected_channel_message_ids.clear();
                    app.channel_batch_expanded = false;
                    app.filtered_channel_batch = None;
                    cx.notify();
                });
            });
            cx.run_until_parked();
            let button = cx
                .debug_bounds("channel-filter-batch-download")
                .expect("batch without selection");
            let toolbar = cx.debug_bounds("channel-compact-toolbar").expect("toolbar");
            assert!(button.left() >= toolbar.left() && button.right() <= toolbar.right());
            assert!(
                cx.debug_bounds("channel-files-download").is_none(),
                "selection action stays independent"
            );
            app.update(cx, |app, cx| {
                let progress = ChannelBatchPreparation::default();
                app.filtered_channel_batch = Some(FilteredBatchUi {
                    account_id: app.telegram_account.as_ref().expect("preview account").id,
                    chat_id: app.selected_chat_id.expect("preview source"),
                    snapshot: progress.snapshot().expect("phase before work"),
                    progress,
                    result: None,
                });
                app.set_page(Page::Transfers, cx);
            });
            cx.run_until_parked();
            let global = cx
                .debug_bounds("shell-filter-batch")
                .expect("navigation cannot hide discovery");
            let bar = cx
                .debug_bounds("global-background-status")
                .expect("global status");
            assert!(global.left() >= bar.left() && global.right() <= bar.right());
            app.update(cx, |app, cx| {
                app.cancel_filtered_channel_batch(cx);
                let state = app.filtered_channel_batch.as_ref().expect("retained state");
                assert_eq!(state.snapshot.phase, Phase::Cancelled);
                assert!(
                    app.filtered_batch_status()
                        .expect("cancel reason")
                        .0
                        .contains("cancelled")
                );
            });
        }
    }

    #[gpui_kit::test]
    fn account_sign_out_fences_preparation_and_hides_prior_account_result(cx: &mut TestAppContext) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Channel);
        app.update(cx, |app, cx| {
            let progress = ChannelBatchPreparation::default();
            app.filtered_channel_batch = Some(FilteredBatchUi {
                account_id: app.telegram_account.as_ref().expect("account").id,
                chat_id: app.selected_chat_id.expect("chat"),
                snapshot: progress.snapshot().expect("state"),
                progress: progress.clone(),
                result: None,
            });
            app.finish_telegram_sign_out(cx);
            assert_eq!(progress.snapshot().expect("fence").phase, Phase::Cancelled);
            assert!(app.filtered_batch_status().is_none());
            assert!(
                app.filtered_batch_active(),
                "background owner must drain before replacement"
            );
        });
    }
}
