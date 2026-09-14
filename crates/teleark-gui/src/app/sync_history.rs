//! Immutable, bounded history rows; only visible rows are materialized.
use super::*;
use std::sync::Arc;

#[derive(Clone, PartialEq)]
pub(super) struct HistoryRow {
    pub title: SharedString,
    pub source: SharedString,
    pub time: SharedString,
    pub tone: components::Tone,
    pub error: SharedString,
}
#[derive(PartialEq)]
struct HistoryRevision {
    account: Option<i64>,
    sources: u64,
    locale: SupportedLocale,
    channel: Option<(std::time::Instant, usize, u64, Option<std::time::Instant>)>,
    private: Option<(u64, Option<i64>)>,
    catalog: Option<(
        dialogs::Phase,
        Option<teleark_core::ApplicationErrorKind>,
        std::time::Instant,
    )>,
}
impl HistoryRevision {
    fn read(app: &TeleArkApp) -> Self {
        Self {
            account: app.telegram_account.as_ref().map(|a| a.id),
            sources: app.channel_sources_revision,
            locale: app.locale(),
            channel: app.channel_sync_snapshot.as_ref().map(|s| {
                (
                    s.last_activity,
                    s.events.len(),
                    s.dropped_events,
                    s.events.back().map(|e| e.at),
                )
            }),
            private: app
                .channel_sync_snapshot
                .as_ref()
                .and_then(|s| s.managed_watch.as_ref())
                .map(|w| (w.change_count, w.last_changed_at_unix_ms)),
            catalog: app.dialogs.history.back().copied(),
        }
    }
}
pub(super) struct SyncHistory {
    revision: Option<HistoryRevision>,
    rows: Arc<Vec<HistoryRow>>,
    _changes: Subscription,
    scroll: gpui_kit::UniformListScrollHandle,
    #[cfg(test)]
    pub materialized: std::rc::Rc<std::cell::Cell<usize>>,
}
impl SyncHistory {
    pub fn new(owner: Entity<TeleArkApp>, rows: Vec<HistoryRow>, cx: &mut Context<Self>) -> Self {
        let changes = cx.observe(&owner, |this, owner, cx| {
            let owner = owner.read(cx);
            let revision = HistoryRevision::read(owner);
            if this.revision.as_ref() == Some(&revision) {
                return;
            }
            this.revision = Some(revision);
            let rows = owner.sync_history_rows();
            if this.rows.as_ref() != &rows {
                this.rows = Arc::new(rows);
                cx.notify();
            }
        });
        Self {
            revision: None,
            rows: Arc::new(rows),
            _changes: changes,
            scroll: gpui_kit::UniformListScrollHandle::new(),
            #[cfg(test)]
            materialized: Default::default(),
        }
    }
}
impl Render for SyncHistory {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let rows = self.rows.clone();
        #[cfg(test)]
        let counter = self.materialized.clone();
        gpui_kit::uniform_list(
            "sync-history-rows",
            rows.len(),
            move |range: std::ops::Range<usize>, _, _| {
                #[cfg(test)]
                counter.set(counter.get() + range.len());
                range
                    .filter_map(|index| rows.get(index).map(|row| (index, row)))
                    .map(|(index, row)| {
                        let summary = format!(
                            "{} · {} · {}{}",
                            row.title,
                            row.time,
                            row.source,
                            if row.error.is_empty() {
                                String::new()
                            } else {
                                format!(" · {}", row.error)
                            }
                        );
                        components::list_summary(("sync-history-row", index), summary)
                            .debug_selector(move || format!("sync-history-row-{index}"))
                            .text_color(if row.error.is_empty() {
                                theme::text_secondary()
                            } else {
                                theme::red()
                            })
                            .border_l_2()
                            .border_color(row.tone.foreground())
                            .pl_2()
                    })
                    .collect::<Vec<_>>()
            },
        )
        .track_scroll(&self.scroll)
        .w_full()
        .h_full()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[gpui_kit::test]
    fn unrelated_notifications_reuse_history_and_all_sources_survive_pin_lock(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Storage);
        app.update(cx, |app, cx| {
            app.channel_sync_snapshot = Some(super::super::channel_sync::tests::fixture_snapshot());
            app.dialogs.transition(dialogs::Phase::Saving, None);
            app.channel_sync_details = true;
            cx.notify();
        });
        cx.run_until_parked();
        let history = app.read_with(cx, |app, _| app.sync_history.clone().expect("timeline"));
        // First notification seeds the revision without reading a mutably borrowed owner.
        app.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        let original = history.read_with(cx, |history, _| history.rows.clone());
        app.read_with(cx, |app, _| {
            assert!(original.iter().any(|r| r.title == app.tr("dialogs-saving")));
            assert!(
                original
                    .iter()
                    .any(|r| r.source == app.tr("storage-remote-title"))
            );
        });
        for _ in 0..30 {
            app.update(cx, |_, cx| cx.notify());
            cx.run_until_parked();
        }
        history.read_with(cx, |history, _| {
            assert!(Arc::ptr_eq(&original, &history.rows))
        });
        app.update(cx, |app, cx| {
            app.dialogs.transition(dialogs::Phase::Complete, None);
            cx.notify();
        });
        cx.run_until_parked();
        history.read_with(cx, |history, _| {
            assert!(!Arc::ptr_eq(&original, &history.rows))
        });
        app.update(cx, |app, _| {
            let before = app.sync_history_rows();
            app.app_lock.locked = true;
            assert!(before == app.sync_history_rows());
        });
    }
}
