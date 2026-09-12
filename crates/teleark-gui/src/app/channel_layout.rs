//! Channel-list resizing uses the shared Kit divider and persists only completed drags.

use super::*;
use gpui_kit::component::resizable::{h_resizable, resizable_panel};
use teleark_runtime::CHANNEL_SIDEBAR_MIN_WIDTH;

impl TeleArkApp {
    // Drop unfinished Kit drag ownership when its surface disappears. Old
    // callbacks cannot save over a newer layout; completed widths live in preferences.
    pub(super) fn reset_channel_layout(&mut self, cx: &mut Context<Self>) {
        self.channel_layout = cx.new(|_| ResizableState::default());
        cx.notify();
    }

    pub(super) fn render_workspace(
        &mut self,
        window: &mut Window,
        layout: LayoutPolicy,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let page = self
            .page_view
            .get_or_insert_with(|| {
                let owner = cx.entity();
                cx.new(|cx| workspace_view::ContentView::new(owner, layout, false, cx))
            })
            .clone();
        page.update(cx, |view, cx| view.set_layout(layout, cx));
        let mut content_style = gpui_kit::StyleRefinement::default();
        content_style.size.width = Some(gpui_kit::relative(1.0).into());
        content_style.size.height = Some(gpui_kit::relative(1.0).into());
        let content = div()
            .debug_selector(|| "workspace-content".into())
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .when(self.page != Page::Account, |body| {
                body.child(self.render_header(window, layout, cx))
            })
            .child(div().flex_1().min_h_0().min_w_0().overflow_hidden().child(
                if window.is_a11y_active() {
                    page.into_any_element()
                } else {
                    page.cached(content_style.clone()).into_any_element()
                },
            ));
        if self.page != Page::Channel {
            return content.into_any_element();
        }
        let sources = self
            .source_view
            .get_or_insert_with(|| {
                let owner = cx.entity();
                cx.new(|cx| workspace_view::ContentView::new(owner, layout, true, cx))
            })
            .clone();
        sources.update(cx, |view, cx| view.set_layout(layout, cx));
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                h_resizable("channel-workspace")
                    .with_state(&self.channel_layout)
                    .child(
                        resizable_panel()
                            .size(px(layout.channel_sidebar_width()))
                            .size_range(
                                px(f32::from(CHANNEL_SIDEBAR_MIN_WIDTH))
                                    ..px(layout.channel_sidebar_max_width()),
                            )
                            .flex_none()
                            .child(
                                div()
                                    .debug_selector(|| "channel-list-panel".into())
                                    .size_full()
                                    .child(if window.is_a11y_active() {
                                        sources.into_any_element()
                                    } else {
                                        sources.cached(content_style).into_any_element()
                                    }),
                            ),
                    )
                    .child(
                        resizable_panel()
                            .size_range(px(crate::layout::CHANNEL_CONTENT_MIN_WIDTH)..px(f32::MAX))
                            .child(content),
                    )
                    .on_resize(
                        cx.listener(|this, state: &Entity<ResizableState>, window, cx| {
                            if state.entity_id() != this.channel_layout.entity_id() {
                                return;
                            }
                            let Some(width) = state.read(cx).sizes().first().copied() else {
                                return;
                            };
                            let width = LayoutPolicy::from_window(window)
                                .with_sidebar_collapsed(this.preferences.sidebar_collapsed)
                                .with_channel_sidebar_width(width.into())
                                .channel_sidebar_width()
                                .round() as u16;
                            if this.preferences.channel_sidebar_width != width {
                                this.preferences.channel_sidebar_width = width;
                                this.persist_preferences(cx);
                            }
                        }),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit as gpui;
    use gpui_kit::{Modifiers, MouseButton, TestAppContext, VisualTestContext, point, size};

    fn drag_to(cx: &mut VisualTestContext, target: f32) {
        let panel = cx.debug_bounds("channel-list-panel").expect("channel list");
        let start = point(panel.right() - px(2.0), panel.center().y);
        cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(
            start + point(px(10.0), px(0.0)),
            Some(MouseButton::Left),
            Modifiers::default(),
        );
        cx.simulate_mouse_move(
            point(panel.left() + px(target), start.y),
            Some(MouseButton::Left),
            Modifiers::default(),
        );
        cx.run_until_parked();
    }

    #[gpui::test]
    fn channel_divider_drag_updates_layout_then_persists_on_release(cx: &mut TestAppContext) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Channel);
        cx.simulate_resize(size(px(1360.0), px(760.0)));
        cx.run_until_parked();
        assert_eq!(
            cx.debug_bounds("channel-list-panel")
                .expect("channel list")
                .size
                .width,
            px(208.0)
        );
        drag_to(cx, 420.0);
        assert_eq!(
            cx.debug_bounds("channel-list-panel")
                .expect("channel list")
                .size
                .width,
            px(420.0)
        );
        app.read_with(cx, |app, _| {
            assert_eq!(
                app.preferences.channel_sidebar_width, 208,
                "no writes during drag"
            );
        });
        let release = point(px(484.0), px(400.0));
        cx.simulate_mouse_up(release, MouseButton::Left, Modifiers::default());
        cx.run_until_parked();
        app.read_with(cx, |app, _| {
            assert_eq!(app.preferences.channel_sidebar_width, 420)
        });
        cx.simulate_mouse_move(point(px(900.0), px(400.0)), None, Modifiers::default());
        cx.run_until_parked();
        assert_eq!(
            cx.debug_bounds("channel-list-panel")
                .expect("channel list")
                .size
                .width,
            px(420.0)
        );
        app.update(cx, |app, cx| {
            app.set_page(Page::Transfers, cx);
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("channel-list-panel").is_none());
        app.update(cx, |app, cx| {
            app.set_page(Page::Channel, cx);
        });
        cx.run_until_parked();
        assert_eq!(
            cx.debug_bounds("channel-list-panel")
                .expect("channel list")
                .size
                .width,
            px(420.0)
        );
        cx.simulate_resize(size(px(900.0), px(600.0)));
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("workspace-content")
                .expect("workspace content")
                .size
                .width
                >= px(500.0)
        );
        app.read_with(cx, |app, _| {
            assert_eq!(app.preferences.channel_sidebar_width, 420)
        });
        cx.simulate_resize(size(px(1360.0), px(760.0)));
        cx.run_until_parked();
        assert_eq!(
            cx.debug_bounds("channel-list-panel")
                .expect("channel list")
                .size
                .width,
            px(420.0)
        );
    }

    #[gpui::test]
    fn abandoning_a_drag_cannot_resize_or_save_after_navigation(cx: &mut TestAppContext) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Channel);
        cx.simulate_resize(size(px(1360.0), px(760.0)));
        cx.run_until_parked();
        drag_to(cx, 420.0);
        app.update(cx, |app, cx| app.set_page(Page::Transfers, cx));
        cx.run_until_parked();
        cx.simulate_mouse_up(
            point(px(484.0), px(400.0)),
            MouseButton::Left,
            Modifiers::default(),
        );
        app.update(cx, |app, cx| app.set_page(Page::Channel, cx));
        cx.run_until_parked();
        cx.simulate_mouse_move(point(px(800.0), px(400.0)), None, Modifiers::default());
        cx.run_until_parked();
        assert_eq!(
            cx.debug_bounds("channel-list-panel")
                .expect("channel list")
                .size
                .width,
            px(208.0)
        );
        app.read_with(cx, |app, _| {
            assert_eq!(app.preferences.channel_sidebar_width, 208)
        });

        // A failed write keeps the in-memory layout and exposes a retry.
        app.update(cx, |app, _| app.visual_preview = false);
        drag_to(cx, 420.0);
        cx.simulate_mouse_up(
            point(px(484.0), px(400.0)),
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.run_until_parked();
        app.read_with(cx, |app, _| {
            assert_eq!(app.preferences.channel_sidebar_width, 420);
            assert_eq!(app.preference_persistence, PreferencePersistence::Failed);
        });
        let retry = cx
            .debug_bounds("channel-width-save-retry")
            .expect("retry visible");
        assert!(retry.right() <= px(484.0) && retry.bottom() <= px(760.0));
        cx.simulate_click(retry.center(), Modifiers::default());
        cx.run_until_parked();
        assert_eq!(
            cx.debug_bounds("channel-list-panel")
                .expect("channel list")
                .size
                .width,
            px(420.0)
        );
    }

    #[gpui::test]
    fn channel_width_is_bounded_at_small_windows_in_every_locale_and_theme(
        cx: &mut TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Channel);
        cx.simulate_resize(size(px(900.0), px(600.0)));
        {
            let locale = SupportedLocale::EnUs;
            for dark in [false, true] {
                for collapsed in [false, true] {
                    cx.update(|window, cx| {
                        app.update(cx, |app, cx| {
                            app.localizer = Localizer::new(locale).expect("catalog");
                            theme::apply_appearance(
                                if dark {
                                    AppearancePreference::Dark
                                } else {
                                    AppearancePreference::Light
                                },
                                window,
                                cx,
                            );
                            app.preferences.sidebar_collapsed = collapsed;
                            cx.notify();
                        })
                    });
                    cx.run_until_parked();
                    drag_to(cx, 1000.0);
                    cx.simulate_mouse_up(
                        point(px(1200.0), px(400.0)),
                        MouseButton::Left,
                        Modifiers::default(),
                    );
                    cx.run_until_parked();
                    let panel = cx.debug_bounds("channel-list-panel").expect("channel list");
                    let content = cx
                        .debug_bounds("workspace-content")
                        .expect("workspace content");
                    assert_eq!(panel.size.width, px(if collapsed { 336.0 } else { 216.0 }));
                    assert!(content.size.width >= px(crate::layout::CHANNEL_CONTENT_MIN_WIDTH));
                    assert!(content.right() <= px(900.0) && content.bottom() <= px(600.0));
                    for selector in [
                        "channel-files-refresh",
                        "channel-filter-toggle",
                        "channel-files-download",
                        "channel-list-width-feedback",
                    ] {
                        let action = cx.debug_bounds(selector).expect("visible channel control");
                        assert!(
                            action.left() >= px(0.0) && action.right() <= px(900.0),
                            "{selector}: {action:?}"
                        );
                        assert!(action.top() >= px(0.0) && action.bottom() <= px(600.0));
                    }
                    drag_to(cx, 10.0);
                    cx.simulate_mouse_up(
                        point(px(10.0), px(400.0)),
                        MouseButton::Left,
                        Modifiers::default(),
                    );
                    cx.run_until_parked();
                    assert_eq!(
                        cx.debug_bounds("channel-list-panel")
                            .expect("channel list")
                            .size
                            .width,
                        px(180.0)
                    );
                }
            }
        }
    }
}
