//! Owns bounded background observations; no filesystem work runs on the UI thread.
use super::*;
pub(crate) use teleark_runtime::LocalDownloadObservation;
use teleark_runtime::LocalFilePresence;
#[cfg(test)]
use teleark_runtime::{DownloadedFileRecord, DownloadedFilesCursor};

// Keep every candidate for a source so a missing/newer copy can fall back to
// an older present copy without scanning unrelated download history.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum LocalSource {
    Message(i64, i64),
    Package(i64, String),
}
type Candidate = (bool, i64, std::path::PathBuf);

#[derive(Default)]
pub(crate) struct LocalDownloadCache {
    pub(crate) limited: bool,
    revision: u64,
    paths: std::collections::BTreeMap<std::path::PathBuf, LocalDownloadObservation>,
    sources: std::collections::BTreeMap<LocalSource, BTreeSet<Candidate>>,
    ages: BTreeSet<(i64, std::path::PathBuf)>,
}

impl LocalDownloadCache {
    const CAPACITY: usize = 10_000;

    pub(crate) fn clear(&mut self) {
        if !self.paths.is_empty() {
            self.revision = self.revision.wrapping_add(1);
        }
        self.limited = false;
        self.paths.clear();
        self.sources.clear();
        self.ages.clear();
    }

    pub(crate) fn get(&self, path: &std::path::Path) -> Option<&LocalDownloadObservation> {
        self.paths.get(path)
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    fn keys(item: &LocalDownloadObservation) -> impl Iterator<Item = LocalSource> {
        item.file
            .message_id
            .map(|id| LocalSource::Message(item.file.chat_id, id))
            .into_iter()
            .chain(
                item.file
                    .package_id
                    .as_ref()
                    .map(|id| LocalSource::Package(item.file.chat_id, id.clone())),
            )
    }

    fn rank(item: &LocalDownloadObservation) -> Candidate {
        (
            item.presence == LocalFilePresence::Present,
            item.file.completed_at_unix_ms,
            item.file.destination.clone(),
        )
    }

    fn remove(&mut self, path: &std::path::Path) {
        if let Some(item) = self.paths.remove(path) {
            self.revision = self.revision.wrapping_add(1);
            let rank = Self::rank(&item);
            for key in Self::keys(&item) {
                if let Some(candidates) = self.sources.get_mut(&key) {
                    candidates.remove(&rank);
                    if candidates.is_empty() {
                        self.sources.remove(&key);
                    }
                }
            }
            self.ages
                .remove(&(item.file.completed_at_unix_ms, item.file.destination));
        }
    }

    pub(crate) fn insert(
        &mut self,
        path: std::path::PathBuf,
        item: LocalDownloadObservation,
    ) -> bool {
        if self.paths.get(&path) == Some(&item) {
            return false;
        }
        if self.paths.len() == Self::CAPACITY
            && !self.paths.contains_key(&path)
            && self.ages.first().is_some_and(|oldest| {
                (item.file.completed_at_unix_ms, &path) <= (oldest.0, &oldest.1)
            })
        {
            return false;
        }
        self.remove(&path);
        let rank = Self::rank(&item);
        for key in Self::keys(&item) {
            self.sources.entry(key).or_default().insert(rank.clone());
        }
        self.ages
            .insert((item.file.completed_at_unix_ms, path.clone()));
        self.paths.insert(path, item);
        self.revision = self.revision.wrapping_add(1);
        while self.paths.len() > Self::CAPACITY {
            if let Some((_, path)) = self.ages.first().cloned() {
                self.remove(&path);
            }
        }
        true
    }

    #[cfg(test)]
    fn unavailable(&mut self) -> bool {
        let changed: Vec<_> = self
            .paths
            .values()
            .filter(|item| item.presence != LocalFilePresence::Unavailable)
            .cloned()
            .collect();
        let did_change = !changed.is_empty();
        for mut item in changed {
            item.presence = LocalFilePresence::Unavailable;
            self.insert(item.file.destination.clone(), item);
        }
        did_change
    }

    fn source(
        &self,
        chat_id: i64,
        message_id: Option<i64>,
        package_id: Option<&str>,
    ) -> Option<&LocalDownloadObservation> {
        let key = match package_id {
            Some(id) => LocalSource::Package(chat_id, id.to_owned()),
            None => LocalSource::Message(chat_id, message_id?),
        };
        let (_, _, path) = self.sources.get(&key)?.last()?;
        self.paths.get(path)
    }
}

impl TeleArkApp {
    pub(super) fn start_local_file_refresh(&mut self, cx: &mut Context<Self>) {
        let Some(library) = self.library.clone() else {
            return;
        };
        self.local_files_context = Some(cx.observe(&cx.entity(), |this, _, cx| {
            this.sync_local_file_context(cx);
        }));
        let work =
            cx.background_spawn(async move { teleark_runtime::LocalDownloadMonitor::new(library) });
        self.local_files_task = Some(cx.spawn(async move |this, cx| {
            let Ok(monitor) = work.await else { return };
            let mut subscription = monitor.subscribe();
            let Some(entity) = this.upgrade() else { return };
            let task = entity.update(cx, |app, cx| {
                app.local_file_monitor = Some(monitor);
                app.sync_local_file_context(cx);
                let mut initial = true;
                Self::observe_local_file_updates(
                    async move || {
                        if !initial && !subscription.changed().await {
                            return None;
                        }
                        initial = false;
                        Some(subscription.take_updates())
                    },
                    cx,
                )
            });
            drop(entity);
            task.await;
        }));
    }

    // Keeping the revision source separate lets UI ownership and repaint tests
    // use a controlled executor, while Runtime tests exercise the real workers.
    fn observe_local_file_updates(
        mut next: impl AsyncFnMut() -> Option<teleark_runtime::LocalDownloadUpdates> + 'static,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        cx.spawn(async move |this, cx| {
            while let Some(update) = next().await {
                let Some(entity) = this.upgrade() else { return };
                entity.update(cx, |app, cx| app.apply_local_file_updates(update, cx));
                // Never retain the window while waiting for another revision.
                drop(entity);
            }
        })
    }

    fn sync_local_file_context(&mut self, cx: &mut Context<Self>) {
        let scope = self
            .telegram_account
            .as_ref()
            .map(|account| (account.id, self.telegram_login_generation));
        if self.local_files_scope != scope {
            self.local_files_scope = scope;
            self.local_downloads.clear();
            self.channel_file_table.update(cx, |_, cx| cx.notify());
            cx.notify();
        }
        if let Some(monitor) = &self.local_file_monitor {
            let chat = matches!(self.page, Page::Channel | Page::Storage)
                .then_some(self.selected_chat_id)
                .flatten();
            monitor.set_context(scope, chat);
        }
    }

    fn apply_local_file_updates(
        &mut self,
        update: teleark_runtime::LocalDownloadUpdates,
        cx: &mut Context<Self>,
    ) {
        let scope = self
            .telegram_account
            .as_ref()
            .map(|account| (account.id, self.telegram_login_generation));
        if update.scope != scope {
            return;
        }
        let mut changed = update.reset && !self.local_downloads.paths.is_empty();
        if update.reset {
            self.local_downloads.clear();
        }
        changed |= self.local_downloads.limited != update.limited;
        self.local_downloads.limited = update.limited;
        for (path, observation) in update.changes {
            if let Some(observation) = observation {
                changed |= self.local_downloads.insert(path, observation);
            } else {
                changed |= self.local_downloads.paths.contains_key(&path);
                self.local_downloads.remove(&path);
            }
        }
        if changed {
            self.channel_file_table.update(cx, |_, cx| cx.notify());
            cx.notify();
        }
    }

    pub(crate) fn local_download_actions(
        &self,
        observation: &LocalDownloadObservation,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::component::{Disableable as _, IconName};
        use gpui_kit::{IntoElement as _, ParentElement as _, Styled as _, div};
        let available = observation.presence == LocalFilePresence::Present && !self.visual_preview;
        let open = observation.file.destination.clone();
        let reveal = open.clone();
        div()
            .mt_3()
            .flex()
            .flex_wrap()
            .gap_2()
            .child(
                crate::components::button(
                    "local-output-open",
                    self.tr("action-open-file"),
                    Some(IconName::ArrowRight),
                    false,
                )
                .disabled(!available)
                .on_click(move |_, _, cx| cx.open_with_system(&open)),
            )
            .child(
                crate::components::button(
                    "local-output-reveal",
                    self.tr("action-show-in-folder"),
                    Some(IconName::FolderOpen),
                    false,
                )
                .disabled(!available)
                .on_click(move |_, _, cx| cx.reveal_path(&reveal)),
            )
            .into_any_element()
    }

    pub(crate) fn local_presence_for_path(
        &self,
        path: &std::path::Path,
    ) -> Option<LocalFilePresence> {
        self.local_downloads.get(path).map(|item| item.presence)
    }

    pub(crate) fn local_download_for_source(
        &self,
        chat_id: i64,
        message_id: Option<i64>,
        package_id: Option<&str>,
    ) -> Option<&LocalDownloadObservation> {
        self.local_downloads.source(chat_id, message_id, package_id)
    }

    pub(crate) fn local_presence_label(&self, presence: Option<LocalFilePresence>) -> SharedString {
        self.tr(match presence {
            Some(LocalFilePresence::Present) => "local-file-present",
            Some(LocalFilePresence::Missing) => "local-file-missing",
            Some(LocalFilePresence::SizeChanged) => "local-file-size-changed",
            Some(LocalFilePresence::Unavailable) => "local-file-unavailable",
            None | Some(LocalFilePresence::Checking) => "local-file-checking",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui_kit::test]
    fn channel_badges_follow_fresh_observations_and_reject_old_account_results(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Channel);
        for full in [false, true] {
            cx.simulate_resize(gpui_kit::size(px(900.0), px(600.0)));
            cx.update(|window, _| {
                if window.is_fullscreen() != full {
                    window.toggle_fullscreen();
                }
                assert_eq!(window.is_fullscreen(), full);
            });
            app.update(cx, |app, cx| {
                app.local_downloads.clear();
                app.show_channel_detail = false;
                cx.notify();
            });
            for presence in [
                LocalFilePresence::Present,
                LocalFilePresence::Checking,
                LocalFilePresence::Missing,
                LocalFilePresence::SizeChanged,
                LocalFilePresence::Unavailable,
                LocalFilePresence::Present,
            ] {
                let message = app.update(cx, |app, cx| {
                    let message = app.telegram_files[0].message_id;
                    let mut item = observation(1, message, presence);
                    item.file.chat_id = app.selected_chat_id.expect("channel");
                    let scope = Some((1, app.telegram_login_generation));
                    app.apply_local_file_updates(
                        teleark_runtime::LocalDownloadUpdates {
                            scope,
                            changes: std::collections::BTreeMap::from([(
                                item.file.destination.clone(),
                                Some(item),
                            )]),
                            ..Default::default()
                        },
                        cx,
                    );
                    message
                });
                cx.run_until_parked();
                assert_eq!(message, 5000);
                let selector = match presence {
                    LocalFilePresence::Present => "channel-local-state-5000-Present",
                    LocalFilePresence::Checking => "channel-local-state-5000-Checking",
                    LocalFilePresence::Missing => "channel-local-state-5000-Missing",
                    LocalFilePresence::SizeChanged => "channel-local-state-5000-SizeChanged",
                    LocalFilePresence::Unavailable => "channel-local-state-5000-Unavailable",
                };
                assert!(cx.debug_bounds(selector).is_some());
                if presence != LocalFilePresence::Present {
                    assert!(
                        cx.debug_bounds("channel-local-state-5000-Present")
                            .is_none()
                    );
                }
            }
            app.update(cx, |app, cx| {
                let message = app.telegram_files[0].message_id;
                let mut item = observation(1, message, LocalFilePresence::Missing);
                item.file.chat_id = app.selected_chat_id.expect("channel");
                let scope = Some((1, app.telegram_login_generation));
                let changes = std::collections::BTreeMap::from([(
                    item.file.destination.clone(),
                    Some(item.clone()),
                )]);
                app.apply_local_file_updates(
                    teleark_runtime::LocalDownloadUpdates {
                        scope,
                        changes,
                        ..Default::default()
                    },
                    cx,
                );
                item.presence = LocalFilePresence::Present;
                app.apply_local_file_updates(
                    teleark_runtime::LocalDownloadUpdates {
                        scope: Some((1, app.telegram_login_generation.wrapping_sub(1))),
                        reset: true,
                        changes: std::collections::BTreeMap::from([(
                            item.file.destination.clone(),
                            Some(item),
                        )]),
                        ..Default::default()
                    },
                    cx,
                );
                assert_eq!(
                    app.local_download_for_source(
                        app.selected_chat_id.expect("channel"),
                        Some(message),
                        None
                    )
                    .expect("current observation")
                    .presence,
                    LocalFilePresence::Missing
                );
            });
        }
    }

    #[gpui_kit::test]
    fn unchanged_local_probes_do_not_repaint_or_retain_a_closed_window(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Channel);
        #[derive(Default)]
        struct Revisions {
            queued: std::collections::VecDeque<teleark_runtime::LocalDownloadUpdates>,
            waker: Option<std::task::Waker>,
        }
        impl Revisions {
            fn publish(&mut self, update: teleark_runtime::LocalDownloadUpdates) {
                self.queued.push_back(update);
                if let Some(waker) = self.waker.take() {
                    waker.wake();
                }
            }
        }
        // The native monitor is covered by Runtime's worker/drop tests. Waking
        // GPUI's deterministic executor from an OS worker is not a valid UI test.
        let revisions = std::rc::Rc::new(std::cell::RefCell::new(Revisions::default()));
        let source = revisions.clone();
        let notifications = std::rc::Rc::new(std::cell::Cell::new(0));
        let counter = notifications.clone();
        let subscription =
            cx.update(|_, cx| cx.observe(&app, move |_, _| counter.set(counter.get() + 1)));
        app.update(cx, |app, cx| {
            app.local_files_task = Some(TeleArkApp::observe_local_file_updates(
                async move || {
                    std::future::poll_fn(|cx| {
                        let mut source = source.borrow_mut();
                        if let Some(update) = source.queued.pop_front() {
                            std::task::Poll::Ready(Some(update))
                        } else {
                            source.waker = Some(cx.waker().clone());
                            std::task::Poll::Pending
                        }
                    })
                    .await
                },
                cx,
            ));
        });
        cx.run_until_parked();
        let scope = app.read_with(cx, |app, _| {
            app.telegram_account
                .as_ref()
                .map(|account| (account.id, app.telegram_login_generation))
        });
        let item = observation(1, 1, LocalFilePresence::Present);
        let update = || teleark_runtime::LocalDownloadUpdates {
            scope,
            changes: std::collections::BTreeMap::from([(
                item.file.destination.clone(),
                Some(item.clone()),
            )]),
            ..Default::default()
        };
        let before_change = notifications.get();
        revisions.borrow_mut().publish(update());
        cx.run_until_parked();
        assert_eq!(notifications.get(), before_change + 1);
        let baseline = notifications.get();
        revisions.borrow_mut().publish(update());
        cx.run_until_parked();
        cx.background_executor.advance_clock(Duration::from_secs(3));
        cx.run_until_parked();
        assert_eq!(notifications.get(), baseline);
        assert!(
            revisions.borrow().waker.is_some(),
            "consumer waits for a revision"
        );
        drop(subscription);
        let weak = app.downgrade();
        cx.update(|window, _| window.remove_window());
        drop(app);
        cx.run_until_parked();
        assert!(
            weak.upgrade().is_none(),
            "waiting consumer must not retain the app"
        );
        // A late revision after the window closes must not revive the consumer.
        revisions.borrow_mut().publish(update());
        cx.background_executor
            .advance_clock(Duration::from_secs(10));
        cx.run_until_parked();
        assert_eq!(notifications.get(), baseline);
        assert_eq!(
            std::rc::Rc::strong_count(&revisions),
            1,
            "source released on close"
        );
    }

    fn observation(id: u64, message: i64, presence: LocalFilePresence) -> LocalDownloadObservation {
        LocalDownloadObservation {
            file: DownloadedFileRecord {
                cursor: DownloadedFilesCursor { kind: 0, id },
                account_id: 1,
                chat_id: 20,
                message_id: Some(message),
                package_id: Some(format!("{:032x}", message)),
                destination: format!("/tmp/teleark-cache-fixture-{id}").into(),
                size_bytes: 10,
                completed_at_unix_ms: id as i64,
            },
            presence,
        }
    }

    #[test]
    fn local_source_index_matches_history_selection_through_changes_and_failure() {
        let mut cache = LocalDownloadCache::default();
        for id in 0..1_000 {
            let item = observation(
                id,
                (id % 17) as i64,
                if id % 3 == 0 {
                    LocalFilePresence::Present
                } else {
                    LocalFilePresence::Missing
                },
            );
            assert!(cache.insert(item.file.destination.clone(), item.clone()));
            assert!(!cache.insert(item.file.destination.clone(), item));
        }
        // Replace a path's source and downgrade a present copy; both old and new
        // source indexes must still agree with the historical selection rule.
        let item = observation(999, 500, LocalFilePresence::SizeChanged);
        assert!(cache.insert(item.file.destination.clone(), item));
        for message in (0..17).chain([500]) {
            let reference = cache
                .paths
                .values()
                .filter(|item| item.file.message_id == Some(message))
                .max_by_key(|item| {
                    (
                        item.presence == LocalFilePresence::Present,
                        item.file.completed_at_unix_ms,
                    )
                });
            assert_eq!(cache.source(20, Some(message), None), reference);
            assert_eq!(
                cache.source(20, None, Some(&format!("{:032x}", message))),
                reference
            );
            assert!(cache.source(21, Some(message), None).is_none());
        }
        assert!(cache.unavailable());
        assert!(!cache.unavailable());
        assert!(
            cache
                .paths
                .values()
                .all(|item| item.presence == LocalFilePresence::Unavailable)
        );
        cache.clear();
        assert!(cache.paths.is_empty() && cache.sources.is_empty() && cache.ages.is_empty());
    }

    #[test]
    fn local_output_retention_and_source_candidates_stay_bounded() {
        let mut cache = LocalDownloadCache::default();
        for id in 0..10_100 {
            let item = observation(id, id as i64, LocalFilePresence::Present);
            assert!(cache.insert(item.file.destination.clone(), item));
        }
        assert_eq!(cache.paths.len(), LocalDownloadCache::CAPACITY);
        assert_eq!(cache.ages.len(), LocalDownloadCache::CAPACITY);
        assert_eq!(
            cache.sources.values().map(BTreeSet::len).sum::<usize>(),
            2 * LocalDownloadCache::CAPACITY
        );
        assert!(cache.source(20, Some(99), None).is_none());
        assert!(cache.source(20, Some(100), None).is_some());
        let evicted = observation(1, 1, LocalFilePresence::Present);
        assert!(!cache.insert(evicted.file.destination.clone(), evicted));
    }

    #[test]
    #[ignore = "manual comparative benchmark; no wall-time assertion"]
    fn perf_local_source_lookup() {
        let mut cache = LocalDownloadCache::default();
        for id in 0..10_000 {
            let item = observation(id, id as i64, LocalFilePresence::Present);
            cache.insert(item.file.destination.clone(), item);
        }
        let started = std::time::Instant::now();
        for message in 0..1_000 {
            std::hint::black_box(
                cache
                    .paths
                    .values()
                    .filter(|item| item.file.chat_id == 20 && item.file.message_id == Some(message))
                    .max_by_key(|item| {
                        (
                            item.presence == LocalFilePresence::Present,
                            item.file.completed_at_unix_ms,
                        )
                    }),
            );
        }
        let old = started.elapsed();
        let started = std::time::Instant::now();
        for message in 0..1_000 {
            std::hint::black_box(cache.source(20, Some(message), None));
        }
        eprintln!(
            "local_source_lookup records=10000 queries=1000 old_us={} new_us={}",
            old.as_micros(),
            started.elapsed().as_micros()
        );
    }
}
