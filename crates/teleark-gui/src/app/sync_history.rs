//! Immutable, bounded history rows; only visible rows are materialized.
use super::*;
use gpui_kit::StatefulInteractiveElement as _;
use gpui_kit::component::tooltip::Tooltip;
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
                        let tooltip = format!("{}\n{}\n{}", row.title, row.source, row.error);
                        div()
                            .id(("sync-history-row", index))
                            .h(px(76.0))
                            .w_full()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .py_2()
                            .child(div().truncate().child(row.title.clone()))
                            .when(!row.source.is_empty(), |row_el| {
                                row_el.child(div().truncate().child(row.source.clone()))
                            })
                            .when(!row.error.is_empty(), |row_el| {
                                row_el.child(div().truncate().child(row.error.clone()))
                            })
                            .tooltip(move |window, cx| {
                                Tooltip::new(tooltip.clone()).build(window, cx)
                            })
                    })
                    .collect::<Vec<_>>()
            },
        )
        .track_scroll(&self.scroll)
        .w_full()
        .h_full()
    }
}
