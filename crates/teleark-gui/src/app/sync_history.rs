//! Immutable, bounded history rows; only visible rows are materialized.
use super::*;
use std::sync::Arc;

#[derive(Clone, PartialEq)]
pub(super) struct HistoryRow {
    pub title: SharedString,
    pub source: SharedString,
    pub error: SharedString,
}
pub(super) struct SyncHistory {
    rows: Arc<Vec<HistoryRow>>,
    _changes: Subscription,
    scroll: gpui_kit::UniformListScrollHandle,
    #[cfg(test)]
    pub materialized: std::rc::Rc<std::cell::Cell<usize>>,
}
impl SyncHistory {
    pub fn new(owner: Entity<TeleArkApp>, rows: Vec<HistoryRow>, cx: &mut Context<Self>) -> Self {
        let changes = cx.observe(&owner, |this, owner, cx| {
            let rows = owner.read(cx).sync_history_rows();
            if this.rows.as_ref() != &rows {
                this.rows = Arc::new(rows);
                cx.notify();
            }
        });
        Self {
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
                            "{} · {}{}",
                            row.title,
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
                            .border_color(theme::border())
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
