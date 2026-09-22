//! Concise shell presentation; detailed phases and timestamps stay in the inspector.
use super::*;
use components::Tone;
use std::sync::{Arc, Weak};
use teleark_runtime::{ChannelDownloadSnapshot, TransferSnapshotView, VaultTransferSnapshot};
use teleark_runtime::{ChannelSyncPhase as Phase, ChannelSyncSnapshot};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct StatusRates {
    pub download: Option<u64>,
    pub upload: Option<u64>,
    pub cleanup: Option<(u64, teleark_runtime::ChannelDownloadCleanupPhase)>,
}

struct CachedRates {
    account: Option<i64>,
    native: Weak<[Arc<ChannelDownloadSnapshot>]>,
    vault: Weak<[Arc<VaultTransferSnapshot>]>,
    revisions: (u64, u64),
    rates: StatusRates,
}

#[derive(Default)]
pub(super) struct RateCache {
    cached: Option<CachedRates>,
    #[cfg(test)]
    rebuilds: usize,
}

impl RateCache {
    pub fn read(
        &mut self,
        account: Option<i64>,
        native: &TransferSnapshotView<ChannelDownloadSnapshot>,
        vault: &TransferSnapshotView<VaultTransferSnapshot>,
    ) -> StatusRates {
        let revisions = (native.revision, vault.revision);
        let native_key = Arc::downgrade(&native.items);
        let vault_key = Arc::downgrade(&vault.items);
        if let Some(cached) = &self.cached
            && cached.account == account
            && cached.revisions == revisions
            && cached.native.ptr_eq(&native_key)
            && cached.vault.ptr_eq(&vault_key)
        {
            return cached.rates;
        }
        #[cfg(test)]
        {
            self.rebuilds += 1;
        }
        let mut rates = StatusRates {
            download: Some(0),
            upload: Some(0),
            cleanup: None,
        };
        let structure_unchanged = self.cached.as_ref().is_some_and(|cached| {
            cached.account == account
                && cached.native.ptr_eq(&native_key)
                && cached.vault.ptr_eq(&vault_key)
        });
        if structure_unchanged {
            rates.cleanup = self.cached.as_ref().and_then(|cached| cached.rates.cleanup);
        } else {
            for item in native
                .items
                .iter()
                .filter(|item| account.is_some() && item.account_id == account)
            {
                if let Some(cleanup) = item.cleanup
                    && (rates.cleanup.is_none()
                        || matches!(
                            cleanup.phase,
                            teleark_runtime::ChannelDownloadCleanupPhase::Failed(_)
                        ))
                {
                    rates.cleanup = Some((item.id, cleanup.phase));
                }
            }
        }
        if let Some(account) = account {
            for totals in [
                native.account_rates.get(&account),
                vault.account_rates.get(&account),
            ]
            .into_iter()
            .flatten()
            {
                rates.download = rates.download.and_then(|sum| {
                    (totals.downloads_sampling == 0)
                        .then(|| sum.saturating_add(totals.download_bytes_per_second))
                });
                rates.upload = rates.upload.and_then(|sum| {
                    (totals.uploads_sampling == 0)
                        .then(|| sum.saturating_add(totals.upload_bytes_per_second))
                });
            }
        }
        self.cached = Some(CachedRates {
            account,
            native: native_key,
            vault: vault_key,
            revisions,
            rates,
        });
        rates
    }
}

pub(super) struct ShellSyncStatus {
    pub label: SharedString,
    pub tone: Tone,
    /// None renders the small status dot; activity icons never animate on a timer.
    pub icon: Option<IconName>,
}

fn channel_status(snapshot: &ChannelSyncSnapshot) -> (&'static str, Tone, Option<IconName>, usize) {
    let mut channels = std::collections::BTreeSet::new();
    for event in &snapshot.active {
        if let Some(chat) = event.chat_id {
            channels.insert(chat);
        }
    }
    let managed_queued = snapshot
        .managed_scan
        .as_ref()
        .is_some_and(|scan| scan.phase == Phase::ManifestQueued);
    if let Some(scan) = snapshot
        .managed_scan
        .as_ref()
        .filter(|scan| scan.active() && !managed_queued)
    {
        channels.insert(scan.chat_id);
    }
    let busy = matches!(
        snapshot.phase,
        Phase::ManifestReading
            | Phase::ManifestReceiving
            | Phase::ManifestVerifying
            | Phase::ReadingLocal
            | Phase::Discovering
            | Phase::Seeding
            | Phase::History
            | Phase::Receiving
            | Phase::Persisting
            | Phase::Verifying
    );
    if busy && let Some(chat) = snapshot.chat_id {
        channels.insert(chat);
    }
    let attention = snapshot.failed_channels > 0
        || snapshot.failure.is_some()
        || matches!(snapshot.phase, Phase::Failed | Phase::ManifestFailed)
        || snapshot.managed_scan.as_ref().is_some_and(|scan| {
            scan.phase == Phase::ManifestFailed
                || scan
                    .failure
                    .is_some_and(|failure| failure != teleark_core::ApplicationErrorKind::Cancelled)
        });
    let count = channels.len();
    if count > 0 {
        return (
            if attention {
                "shell-sync-active-attention"
            } else if count == 1 {
                "shell-sync-active-channel"
            } else {
                "shell-sync-active-channels"
            },
            if attention { Tone::Amber } else { Tone::Blue },
            Some(if attention {
                IconName::TriangleAlert
            } else {
                IconName::Redo2
            }),
            count,
        );
    }
    if attention {
        return (
            "shell-sync-attention",
            Tone::Amber,
            Some(IconName::TriangleAlert),
            0,
        );
    }
    match snapshot.phase {
        Phase::Connecting => (
            "shell-sync-connecting",
            Tone::Blue,
            Some(IconName::Redo2),
            0,
        ),
        Phase::Waiting | Phase::RateLimited => {
            ("shell-sync-waiting", Tone::Amber, Some(IconName::Pause), 0)
        }
        Phase::Cancelled | Phase::ManifestCancelled => {
            ("shell-sync-paused", Tone::Neutral, Some(IconName::Pause), 0)
        }
        _ if busy || !snapshot.active.is_empty() => {
            ("shell-sync-active", Tone::Blue, Some(IconName::Redo2), 0)
        }
        _ if snapshot.queued > 0
            || managed_queued
            || matches!(snapshot.phase, Phase::Queued | Phase::ManifestQueued) =>
        {
            ("shell-sync-queued", Tone::Neutral, Some(IconName::Pause), 0)
        }
        _ if snapshot.managed_review_pending => {
            ("shell-sync-active", Tone::Blue, Some(IconName::Redo2), 0)
        }
        _ if snapshot
            .managed_scan
            .as_ref()
            .is_some_and(|scan| scan.phase == Phase::ManifestCancelled) =>
        {
            ("shell-sync-paused", Tone::Neutral, Some(IconName::Pause), 0)
        }
        _ if snapshot.last_completed_at.is_some() => ("shell-sync-complete", Tone::Green, None, 0),
        _ => ("shell-sync-ready", Tone::Neutral, None, 0),
    }
}

impl TeleArkApp {
    pub(super) fn toggle_sync_details(&mut self, cx: &mut Context<Self>) {
        // Recheck access when handling the event: a rendered button may outlive
        // application locking or a Telegram session-loss notification.
        if self.app_is_locked() || !self.telegram_is_authorized() {
            return;
        }
        self.channel_sync_details = !self.channel_sync_details;
        self.dialogs.details = false;
        cx.notify();
    }

    pub(super) fn shell_sync_status(&self) -> ShellSyncStatus {
        if self.authorization_snapshot.is_some_and(|snapshot| {
            snapshot.account_id == self.telegram_account.as_ref().map(|account| account.id)
                && snapshot.phase == teleark_runtime::AuthorizationPhase::Checking
        }) {
            return ShellSyncStatus {
                label: self.tr("session-loss-checking"),
                tone: Tone::Amber,
                icon: Some(IconName::Redo2),
            };
        }
        use dialogs::Phase as Dialog;
        let (mut id, mut tone, mut icon, count) = self
            .channel_sync_snapshot
            .as_ref()
            .map(channel_status)
            .unwrap_or(("shell-sync-ready", Tone::Neutral, None, 0));
        let attention = tone == Tone::Amber && matches!(icon, Some(IconName::TriangleAlert))
            || self.library_sync_error.is_some()
            || self.dialogs.phase() == Dialog::Failed;
        let other_active =
            self.library_sync_loading || self.account_restoring || self.dialogs.active();
        let active = count > 0 || matches!(icon, Some(IconName::Redo2)) || other_active;
        if attention {
            id = if active && count > 0 {
                "shell-sync-active-attention"
            } else if active {
                "shell-sync-working-attention"
            } else {
                "shell-sync-attention"
            };
            tone = Tone::Amber;
            icon = Some(IconName::TriangleAlert);
        } else if other_active {
            id = if self.account_restoring {
                "shell-sync-connecting"
            } else if self.dialogs.phase() == Dialog::Waiting && !self.library_sync_loading {
                "shell-sync-waiting"
            } else if count == 0 {
                "shell-sync-active"
            } else {
                id
            };
            tone = if self.dialogs.phase() == Dialog::Waiting {
                Tone::Amber
            } else {
                Tone::Blue
            };
            icon = Some(IconName::Redo2);
        } else if self.channel_sync_snapshot.is_none() {
            id = match self.dialogs.phase() {
                Dialog::Complete => "shell-sync-complete",
                Dialog::Cancelled => "shell-sync-paused",
                _ => id,
            };
        }
        ShellSyncStatus {
            label: self.tr_with(
                id,
                MessageArgs::new().with(
                    "count",
                    teleark_i18n::format::format_integer(self.locale(), count as u64),
                ),
            ),
            tone,
            icon,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{TestAppContext, size};

    #[gpui_kit::test]
    fn synced_click_opens_and_closes_details_only_while_the_application_is_unlocked(
        cx: &mut TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Channel);
        let pin = teleark_runtime::AppPinRecord::create("123456".into()).expect("synthetic PIN");
        app.update(cx, |app, cx| {
            let mut snapshot = channel_sync::tests::fixture_snapshot();
            snapshot.phase = Phase::Idle;
            snapshot.managed_watch = None;
            snapshot.managed_review_pending = false;
            snapshot.queued = 0;
            app.channel_sync_snapshot = Some(snapshot);
            app.app_lock.record = Some(pin);
            cx.notify();
        });
        for full in [false, true] {
            cx.simulate_resize(size(px(900.0), px(600.0)));
            cx.update(|window, _| {
                if window.is_fullscreen() != full {
                    window.toggle_fullscreen();
                }
            });
            cx.run_until_parked();
            app.read_with(cx, |app, _| {
                assert_eq!(app.shell_sync_status().label.as_ref(), "Synced")
            });
            let status = cx
                .debug_bounds("global-sync-details")
                .expect("unlocked sync button");
            cx.simulate_click(status.center(), gpui_kit::Modifiers::default());
            cx.run_until_parked();
            assert!(cx.debug_bounds("channel-sync-inspector").is_some());
            let close = cx
                .debug_bounds("channel-sync-close-details")
                .expect("close button");
            cx.update(|window, _| {
                assert!(close.right() <= window.viewport_size().width);
                assert!(close.bottom() <= window.viewport_size().height);
            });
            cx.simulate_click(status.center(), gpui_kit::Modifiers::default());
            cx.run_until_parked();
            assert!(cx.debug_bounds("channel-sync-inspector").is_none());
            cx.simulate_click(status.center(), gpui_kit::Modifiers::default());
            cx.run_until_parked();
            cx.update(|window, cx| app.update(cx, |app, cx| app.lock_application(window, cx)));
            cx.run_until_parked();
            assert!(cx.debug_bounds("channel-sync-inspector").is_none());
            assert!(cx.debug_bounds("global-sync-details").is_none());
            let locked = cx
                .debug_bounds("locked-background-status")
                .expect("locked status");
            cx.simulate_click(
                gpui_kit::point(locked.left() + px(35.0), locked.center().y),
                gpui_kit::Modifiers::default(),
            );
            cx.run_until_parked();
            app.update(cx, |app, cx| {
                app.toggle_sync_details(cx); // Late callback from the previously visible button.
                assert!(!app.channel_sync_details);
                assert_eq!(app.shell_sync_status().label.as_ref(), "Synced");
                app.app_lock.locked = false;
                cx.notify();
            });
            cx.run_until_parked();
            let status = cx
                .debug_bounds("global-sync-details")
                .expect("restored unlocked button");
            cx.simulate_click(status.center(), gpui_kit::Modifiers::default());
            cx.run_until_parked();
            assert!(cx.debug_bounds("channel-sync-inspector").is_some());
            let close = cx
                .debug_bounds("channel-sync-close-details")
                .expect("close button");
            cx.simulate_click(close.center(), gpui_kit::Modifiers::default());
            cx.run_until_parked();
        }
        app.update(cx, |app, cx| {
            app.telegram_auth = TelegramAuthState::Unauthorized;
            app.toggle_sync_details(cx);
            assert!(
                !app.channel_sync_details,
                "signed-out callbacks are also fenced"
            );
        });
    }

    #[gpui_kit::test]
    fn preparation_status_opens_the_merged_inspector_without_starting_work(
        cx: &mut TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Storage);
        app.update(cx, |app, cx| {
            app.channel_sync_snapshot = None;
            app.dialogs.transition(dialogs::Phase::Saving, None);
            cx.notify();
        });
        cx.run_until_parked();
        let status = cx
            .debug_bounds("dialogs-status")
            .expect("preparation status");
        cx.simulate_click(status.center(), gpui_kit::Modifiers::default());
        cx.run_until_parked();
        assert!(cx.debug_bounds("channel-sync-inspector").is_some());
        app.read_with(cx, |app, _| {
            assert!(!app.dialogs.details, "one merged inspector");
            assert_eq!(app.dialogs.phase(), dialogs::Phase::Saving);
            assert!(app.telegram_task.is_none());
            assert!(app.channel_sync_task.is_none());
        });
    }

    #[gpui_kit::test]
    fn cleanup_stays_visible_after_navigation_without_inventing_throughput(
        cx: &mut TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Transfers);
        for (width, height) in [(900.0, 600.0), (1440.0, 900.0)] {
            cx.simulate_resize(size(px(width), px(height)));
            for failed in [false, true] {
                app.update(cx, |app, cx| {
                    app.preview_native_cleanup(failed);
                    app.set_page(Page::Library, cx);
                    let account = app.telegram_account.as_ref().map(|account| account.id);
                    let mut cache = RateCache::default();
                    for _ in 0..20 {
                        let status = cache.read(
                            account,
                            &app.native_transfer_view,
                            &app.vault_transfer_view,
                        );
                        assert_eq!(status.download, Some(0));
                        assert_eq!(
                            status.cleanup,
                            Some((
                                901,
                                app.native_transfer_view.items[0]
                                    .cleanup
                                    .expect("cleanup")
                                    .phase
                            ))
                        );
                    }
                    assert_eq!(cache.rebuilds, 1);
                    assert!(
                        cache
                            .read(None, &app.native_transfer_view, &app.vault_transfer_view)
                            .cleanup
                            .is_none()
                    );
                    cx.notify();
                });
                cx.run_until_parked();
                let cleanup = cx
                    .debug_bounds("shell-download-cleanup")
                    .expect("visible cleanup outside transfers");
                let bar = cx.debug_bounds("global-background-status").expect("bar");
                assert!(cleanup.size.width > px(0.0));
                assert!(cleanup.left() >= bar.left() && cleanup.right() <= bar.right());
                assert!(cleanup.top() >= bar.top() && cleanup.bottom() <= bar.bottom());
                cx.simulate_click(cleanup.center(), gpui_kit::Modifiers::default());
                cx.run_until_parked();
                app.update(cx, |app, _| {
                    assert_eq!(app.page, Page::Transfers);
                    assert_eq!(app.focused_transfer_key, Some(901));
                    assert!(app.show_transfer_detail);
                });
                assert!(cx.debug_bounds("transfer-activity-detail").is_some());
            }
        }
    }

    #[test]
    fn brief_status_counts_distinct_active_channels_and_never_hides_failures() {
        let mut snapshot = channel_sync::tests::fixture_snapshot();
        snapshot.phase = Phase::Idle;
        snapshot.managed_watch = None;
        snapshot.managed_review_pending = false;
        snapshot.queued = 0;
        assert_eq!(channel_status(&snapshot).0, "shell-sync-complete");
        snapshot.queued = 3;
        assert_eq!(channel_status(&snapshot).0, "shell-sync-queued");
        snapshot.phase = Phase::Queued;
        snapshot.chat_id = Some(10);
        assert_eq!(channel_status(&snapshot).3, 0);
        assert_eq!(channel_status(&snapshot).0, "shell-sync-queued");
        snapshot.queued = 0;
        snapshot.phase = Phase::RateLimited;
        assert_eq!(channel_status(&snapshot).0, "shell-sync-waiting");
        snapshot.phase = Phase::Idle;
        for chat in [Some(10), Some(20), Some(10)] {
            snapshot.active.push(teleark_runtime::ChannelSyncEvent {
                phase: Phase::Receiving,
                chat_id: chat,
                at: std::time::Instant::now(),
                failure: None,
            });
        }
        assert_eq!(channel_status(&snapshot).3, 2);
        snapshot.failed_channels = 1;
        assert_eq!(channel_status(&snapshot).0, "shell-sync-active-attention");
        snapshot.active.clear();
        assert_eq!(channel_status(&snapshot).0, "shell-sync-attention");
        snapshot.failed_channels = 0;
        snapshot.active.push(teleark_runtime::ChannelSyncEvent {
            phase: Phase::Discovering,
            chat_id: None,
            at: std::time::Instant::now(),
            failure: None,
        });
        assert_eq!(channel_status(&snapshot).0, "shell-sync-active");
        snapshot.active.clear();
        let now = std::time::Instant::now();
        snapshot.managed_scan = Some(teleark_runtime::ManagedScanStatus {
            retry_after: None,
            chat_id: 90,
            phase: Phase::ManifestQueued,
            phase_started: now,
            last_activity: now,
            completed: 0,
            total: None,
            cached: 0,
            rejected: 0,
            failure: None,
        });
        assert_eq!(channel_status(&snapshot).0, "shell-sync-queued");
        assert_eq!(channel_status(&snapshot).3, 0);
        for (phase, expected) in [
            (Phase::ManifestReading, "shell-sync-active-channel"),
            (Phase::ManifestFailed, "shell-sync-attention"),
            (Phase::ManifestCancelled, "shell-sync-paused"),
            (Phase::ManifestCompleted, "shell-sync-complete"),
        ] {
            snapshot.managed_scan.as_mut().expect("scan").phase = phase;
            assert_eq!(channel_status(&snapshot).0, expected);
        }
        snapshot.managed_scan = None;
        snapshot.last_completed_at = None;
        assert_eq!(channel_status(&snapshot).0, "shell-sync-ready");
    }

    #[gpui_kit::test]
    fn key_status_and_rates_share_one_row_in_both_themes(cx: &mut TestAppContext) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Channel);
        for (width, height) in [(900.0, 600.0), (1440.0, 900.0)] {
            cx.simulate_resize(size(px(width), px(height)));
            for appearance in [AppearancePreference::Light, AppearancePreference::Dark] {
                cx.update(|window, cx| {
                    app.update(cx, |app, cx| {
                        theme::apply_appearance(appearance, window, cx);
                        let mut snapshot = channel_sync::tests::fixture_snapshot();
                        snapshot.phase = Phase::Idle;
                        snapshot.managed_watch = None;
                        snapshot.managed_review_pending = false;
                        snapshot.queued = 0;
                        app.channel_sync_snapshot = Some(snapshot);
                        app.vault_locked = true;
                        app.vault_status.configured = true;
                        app.vault_status.active_key_locked = true;
                        cx.notify();
                    })
                });
                cx.run_until_parked();
                let status = cx.debug_bounds("global-sync-details").expect("status");
                let locked = cx
                    .debug_bounds("managed-key-status-notice")
                    .expect("inline lock");
                let disk = cx.debug_bounds("status-disk-space").expect("disk on right");
                let download = cx.debug_bounds("shell-download-rate").expect("download");
                let upload = cx.debug_bounds("shell-upload-rate").expect("upload");
                let bar = cx.debug_bounds("global-background-status").expect("bar");
                assert_eq!(bar.size.height, px(28.0));
                assert!(locked.left() >= status.right());
                assert!(locked.bottom() <= bar.bottom() && locked.top() >= bar.top());
                assert!(locked.size.height <= px(18.0));
                assert!(disk.left() > locked.right());
                assert!(download.left() >= disk.right() && upload.left() >= download.right());
                assert!(upload.right() <= px(width));
                app.update(cx, |app, _| {
                    assert_eq!(app.shell_sync_status().label.as_ref(), "Synced")
                });
            }
        }
    }

    #[gpui_kit::test]
    fn status_rates_reuse_unchanged_history_and_invalidate_unknown_and_terminal_samples(
        cx: &mut TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Transfers);
        app.update(cx, |app, _| {
            app.preview_upload_history();
            let fixture = app.vault_transfer_view.items[0].as_ref().clone();
            let mut rows = (0..1000)
                .map(|id| {
                    let mut item = fixture.clone();
                    item.id = id;
                    Arc::new(item)
                })
                .collect::<Vec<_>>();
            let mut running = fixture;
            running.id = 1001;
            running.restored = false;
            running.direction = teleark_runtime::VaultTransferDirection::Upload;
            running.state = teleark_runtime::VaultTransferState::Running;
            running.average_bytes_per_second = None;
            rows.push(Arc::new(running.clone()));
            let mut view = TransferSnapshotView {
                revision: 1,
                items: rows.into(),
                omitted_items: 0,
                account_rates: Arc::new(
                    [(
                        1,
                        teleark_runtime::TransferRates {
                            uploads_sampling: 1,
                            ..Default::default()
                        },
                    )]
                    .into(),
                ),
                ..Default::default()
            };
            let mut cache = RateCache::default();
            for _ in 0..100 {
                assert_eq!(
                    cache.read(Some(1), &app.native_transfer_view, &view).upload,
                    None
                );
            }
            assert_eq!(
                cache.rebuilds, 1,
                "ordinary renders never rescan retained history"
            );
            running.average_bytes_per_second = Some(1024);
            running.telemetry.goodput_bytes_per_second = 9999;
            view.account_rates = Arc::new(
                [(
                    1,
                    teleark_runtime::TransferRates {
                        upload_bytes_per_second: 2048,
                        ..Default::default()
                    },
                )]
                .into(),
            );
            view.items = vec![Arc::new(running.clone())].into();
            // Replacement owners can restart their revision sequence.
            assert_eq!(
                cache.read(Some(1), &app.native_transfer_view, &view).upload,
                Some(2048)
            );
            assert_eq!(cache.rebuilds, 2);
            assert_eq!(
                cache.read(Some(2), &app.native_transfer_view, &view).upload,
                Some(0),
                "previous account samples stay isolated"
            );
            running.state = teleark_runtime::VaultTransferState::Completed;
            view.account_rates = Arc::default();
            view.items = vec![Arc::new(running)].into();
            assert_eq!(
                cache.read(Some(1), &app.native_transfer_view, &view).upload,
                Some(0)
            );
            assert_eq!(cache.rebuilds, 4);
        });
    }

    #[gpui_kit::test]
    fn volume_space_is_event_driven_without_idle_polling(cx: &mut TestAppContext) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Transfers);
        let directory = std::env::temp_dir().join(format!(
            "teleark-volume-space-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).expect("temporary fixture");
        let library = DesktopLibrary::open_synthetic(directory.join("catalog.sqlite3"))
            .expect("temporary catalog");
        library
            .managed_directories()
            .expect("temporary output directories");
        let notifications = std::rc::Rc::new(std::cell::Cell::new(0));
        let counter = notifications.clone();
        let _subscription =
            cx.update(|_, cx| cx.observe(&app, move |_, _| counter.set(counter.get() + 1)));

        app.update(cx, |app, _| {
            app.library = Some(library.clone());
            app.volume_space = None;
        });

        // Explicit event-driven refresh queries the volume space.
        app.update(cx, |app, cx| app.refresh_volume_space(cx));
        cx.run_until_parked();

        app.update(cx, |app, _| {
            assert!(
                app.volume_space.is_some(),
                "volume space should be populated"
            );
        });

        // Coalesced queries: when in flight, another request marks pending.
        app.update(cx, |app, cx| {
            app.volume_space_in_flight = true;
            app.refresh_volume_space(cx);
            assert!(app.volume_space_pending);
            app.volume_space_in_flight = false;
        });

        // Idle time passing must NOT trigger repeated queries or repaint notifications.
        let baseline = notifications.get();
        cx.background_executor
            .advance_clock(std::time::Duration::from_secs(60));
        cx.run_until_parked();
        assert_eq!(notifications.get(), baseline, "no idle polling or repaints");

        // Teardown cleanly.
        app.update(cx, |app, _| {
            app.library = None;
            app.volume_space_task = None;
        });
        cx.run_until_parked();
        drop(_subscription);
        drop(library);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while let Err(err) = std::fs::remove_dir_all(&directory) {
            if std::time::Instant::now() >= deadline {
                panic!("cleanup: {err}");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}
