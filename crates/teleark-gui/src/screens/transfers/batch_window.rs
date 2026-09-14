//! Batch presentation shares the parent's revisioned projection and action owner.
//! The auxiliary window retains neither a runtime nor a second polling loop.
use super::*;
use gpui_kit::{
    Bounds, Entity, Render, Subscription, WeakEntity, WindowBounds, WindowOptions, size,
};

pub(super) const INLINE_BATCH_LIMIT: usize = 9;

#[derive(Clone, Copy)]
pub(super) struct TransferRowTarget {
    pub index: usize,
    pub key: u64,
    pub batch: Option<(u64, usize)>,
}

fn auxiliary_bounds(parent: Bounds<gpui_kit::Pixels>) -> Bounds<gpui_kit::Pixels> {
    let child = size(
        (parent.size.width * 0.86).min(px(1040.0)),
        (parent.size.height * 0.82).min(px(680.0)),
    );
    Bounds::new(
        parent.origin
            + gpui_kit::point(
                (parent.size.width - child.width) / 2.0,
                (parent.size.height - child.height) / 2.0,
            ),
        child,
    )
}

struct BatchWindow {
    owner: WeakEntity<TeleArkApp>,
    account: Option<i64>,
    batch: u64,
    scroll: gpui_kit::UniformListScrollHandle,
    title: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl BatchWindow {
    fn new(
        owner: Entity<TeleArkApp>,
        account: Option<i64>,
        batch: u64,
        parent: gpui_kit::WindowId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let updates = window.observe(&owner, cx, |_, window, _| window.refresh());
        let release = window.observe_release(&owner, cx, |_, window, _| window.remove_window());
        let child = window.window_handle();
        let parent_closed = cx.on_window_closed(move |cx, closed| {
            if closed == parent {
                let _ = child.update(cx, |_, window, _| window.remove_window());
            }
        });
        Self {
            owner: owner.downgrade(),
            account,
            batch,
            scroll: gpui_kit::UniformListScrollHandle::new(),
            title: None,
            _subscriptions: vec![updates, release, parent_closed],
        }
    }
}

impl Render for BatchWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(owner) = self.owner.upgrade() else {
            window.remove_window();
            return div().into_any_element();
        };
        owner.update(cx, |app, cx| {
            if app.telegram_account.as_ref().map(|account| account.id) != self.account {
                window.remove_window();
                return div().into_any_element();
            }
            app.render_batch_window(self.batch, self.scroll.clone(), &mut self.title, window, cx)
        })
    }
}

impl TeleArkApp {
    pub(super) fn activate_transfer_row(
        &mut self,
        target: TransferRowTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected_file = target.index;
        self.focused_transfer_key = Some(target.key);
        self.pending_transfer_delete = None;
        self.show_transfer_detail = target.batch.is_none();
        if let Some((batch, count)) = target.batch {
            if count > INLINE_BATCH_LIMIT {
                self.expanded_transfer_batches.remove(&batch);
                self.open_transfer_batch_window(batch, window, cx);
            } else if !self.expanded_transfer_batches.insert(batch) {
                self.expanded_transfer_batches.remove(&batch);
            }
        }
        self.batch_detail_scroll
            .scroll_to_item(0, gpui_kit::ScrollStrategy::Top);
        self.transfer_detail_scroll
            .set_offset(gpui_kit::point(px(0.0), px(0.0)));
        cx.notify();
    }

    pub(super) fn open_transfer_batch_window(
        &mut self,
        batch: u64,
        parent: &Window,
        cx: &mut Context<Self>,
    ) {
        if let Some((previous, handle)) = self.transfer_batch_window.take() {
            if previous == batch
                && handle
                    .update(cx, |_, window, _| window.activate_window())
                    .is_ok()
            {
                self.transfer_batch_window = Some((batch, handle));
                return;
            }
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }
        self.transfer_batch_window_request = Some(batch);
        let owner = cx.entity().downgrade();
        let account = self.telegram_account.as_ref().map(|account| account.id);
        let parent_id = parent.window_handle().window_id();
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(auxiliary_bounds(parent.bounds()))),
            window_min_size: Some(size(px(620.0), px(360.0))),
            titlebar: Some(gpui_kit::TitlebarOptions {
                title: Some(self.tr("transfer-batch-window-title")),
                ..Default::default()
            }),
            ..Default::default()
        };
        // open_window performs its first render synchronously. Defer until the
        // parent's event/update lease has ended, since the child reads that owner.
        cx.defer(move |cx| {
            let Some(owner) = owner.upgrade() else { return };
            if owner.read(cx).transfer_batch_window_request != Some(batch)
                || owner
                    .read(cx)
                    .telegram_account
                    .as_ref()
                    .map(|account| account.id)
                    != account
            {
                return;
            }
            let view_owner = owner.clone();
            let result = cx.open_window(options, move |window, cx| {
                let view = cx
                    .new(|cx| BatchWindow::new(view_owner, account, batch, parent_id, window, cx));
                cx.new(|cx| gpui_kit::component::Root::new(view, window, cx))
            });
            owner.update(cx, |app, cx| {
                app.transfer_batch_window_request = None;
                match result {
                    Ok(handle) => app.transfer_batch_window = Some((batch, handle)),
                    Err(_) => {
                        app.transfer_action_error =
                            Some(teleark_core::ApplicationErrorKind::InvalidRequest)
                    }
                }
                cx.notify();
            });
        });
    }

    fn render_batch_window(
        &self,
        batch: u64,
        scroll: gpui_kit::UniformListScrollHandle,
        title: &mut Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let items = self.transfer_items();
        let Some(group) = items.iter().find(|item| {
            !item.child() && item.batch_count() > 0 && item.parent_key() == Some(batch)
        }) else {
            return div()
                .size_full()
                .p_4()
                .bg(theme::surface())
                .text_color(theme::text_primary())
                .child(self.tr("transfer-batch-unavailable"))
                .into_any_element();
        };
        let group = group.row(self);
        let members = std::sync::Arc::new(
            items
                .iter()
                .filter(|item| item.child() && item.parent_key() == Some(batch))
                .cloned()
                .collect::<Vec<_>>(),
        );
        let selected = members
            .iter()
            .enumerate()
            .find(|(index, item)| self.focused_transfer_key == Some(item.key(*index)))
            .map(|(_, item)| item.row(self));
        let count = members.len();
        let layout = LayoutPolicy::from_window(window);
        let palette = theme::batch_palette(cx);
        if title.as_ref() != Some(&group.name) {
            window.set_window_title(&group.name);
            *title = Some(group.name.clone());
        }
        // Context::processor retains its entity. A detached list must not keep
        // the main app alive after its window is closed.
        let owner = cx.entity().downgrade();
        let list = gpui_kit::uniform_list(
            "batch-window-members",
            count,
            move |range: std::ops::Range<usize>, _: &mut Window, cx: &mut gpui_kit::App| {
                owner
                    .update(cx, |app, cx| {
                        range
                            .filter_map(|index| members.get(index).map(|item| (index, item)))
                            .map(|(index, item)| {
                                app.render_transfer_row(
                                    index,
                                    item.row(app),
                                    BatchRowPosition::WindowMember,
                                    cx,
                                )
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            },
        )
        .track_scroll(&scroll)
        .w_full()
        .flex_1()
        .min_h_0();
        let content = div()
            .debug_selector(|| "transfer-batch-group".into())
            .w_full()
            .flex_1()
            .min_h_0()
            .relative()
            .flex()
            .flex_col()
            .bg(palette.body)
            .border_1()
            .border_color(palette.border)
            .rounded(theme::RADIUS_MEDIUM)
            .overflow_hidden()
            .child(
                div()
                    .debug_selector(|| "transfer-batch-window-header".into())
                    .relative()
                    .flex_none()
                    .h(theme::BATCH_ROW_HEIGHT)
                    .px_4()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .bg(palette.header)
                    .border_b_1()
                    .border_color(palette.border)
                    .child(
                        div()
                            .text_size(theme::LIST_TEXT_SIZE)
                            .line_height(theme::LIST_LINE_HEIGHT)
                            .font_weight(FontWeight::SEMIBOLD)
                            .truncate()
                            .child(group.name.clone()),
                    )
                    .when_some(group.batch_summary.as_ref(), |header, summary| {
                        header.child(
                            div()
                                .text_size(theme::LIST_SECONDARY_TEXT_SIZE)
                                .line_height(px(14.0))
                                .truncate()
                                .text_color(theme::text_secondary())
                                .child(self.batch_progress_label(summary)),
                        )
                    }),
            )
            .when(self.transfer_action_job.is_some(), |page| {
                page.child(
                    div()
                        .flex_none()
                        .p_2()
                        .text_xs()
                        .text_color(theme::blue())
                        .child(self.tr("transfer-actions-applying")),
                )
            })
            .when_some(self.transfer_action_error, |page, error| {
                page.child(
                    div()
                        .flex_none()
                        .p_2()
                        .text_xs()
                        .text_color(theme::red())
                        .child(self.tr(native_download_error_message_id(error))),
                )
            })
            .when_some(self.pending_transfer_delete, |page, id| {
                page.child(self.render_transfer_delete_confirmation(vec![id], cx))
            })
            .child(list)
            .child(
                div()
                    .debug_selector(|| "transfer-batch-window-rail".into())
                    .absolute()
                    .left_0()
                    .top(theme::RADIUS_MEDIUM)
                    .bottom(theme::RADIUS_MEDIUM)
                    .w(theme::BATCH_RAIL_WIDTH)
                    .bg(palette.rail),
            );
        div()
            .id("transfer-batch-window")
            .debug_selector(|| "transfer-batch-window".into())
            .size_full()
            .p_3()
            .relative()
            .flex()
            .flex_col()
            .bg(theme::canvas())
            .text_color(theme::text_primary())
            .child(content)
            .child(
                components::list_summary(
                    "transfer-batch-window-footer",
                    self.tr("transfer-batch-window-live"),
                )
                .text_color(theme::text_secondary()),
            )
            .when_some(
                selected.filter(|_| self.show_transfer_detail),
                |page, selected| {
                    page.child(
                        div()
                            .absolute()
                            .right_0()
                            .top_0()
                            .bottom_0()
                            .shadow_lg()
                            .child(self.render_transfer_detail(selected, layout, cx)),
                    )
                },
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn batch_window_is_centered_and_smaller_than_parent() {
        for (width, height) in [(900.0, 600.0), (1360.0, 760.0), (2400.0, 1600.0)] {
            let parent = Bounds::new(
                gpui_kit::point(px(30.0), px(60.0)),
                size(px(width), px(height)),
            );
            let child = auxiliary_bounds(parent);
            assert!(child.size.width < parent.size.width && child.size.height < parent.size.height);
            assert_eq!(child.center(), parent.center());
        }
    }
}
