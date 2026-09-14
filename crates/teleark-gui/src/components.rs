use gpui_kit::component::{
    IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    progress::Progress,
    scroll::ScrollableElement as _,
};
use gpui_kit::{
    AnyElement, Div, ElementId, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Rgba, ScrollHandle, SharedString, Stateful,
    StatefulInteractiveElement as _, Styled as _, div, prelude::FluentBuilder as _, px,
};

use crate::theme;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Tone {
    Blue,
    Green,
    Amber,
    Red,
    Purple,
    Neutral,
}

impl Tone {
    pub fn foreground(self) -> Rgba {
        match self {
            Self::Blue => theme::blue(),
            Self::Green => theme::green(),
            Self::Amber => theme::amber(),
            Self::Red => theme::red(),
            Self::Purple => theme::purple(),
            Self::Neutral => theme::text_secondary(),
        }
    }

    pub fn background(self) -> Rgba {
        match self {
            Self::Blue => theme::blue_soft(),
            Self::Green => theme::green_soft(),
            Self::Amber => theme::amber_soft(),
            Self::Red => theme::red_soft(),
            Self::Purple => theme::purple_soft(),
            Self::Neutral => theme::border_subtle(),
        }
    }
}

pub fn card() -> Div {
    div()
        .rounded(theme::RADIUS_MEDIUM)
        .border_1()
        .border_color(theme::border())
        .bg(theme::surface())
}

/// Shared visual boundary for background work, with a named task and a semantic state.
/// Callers append the explanation and actions below this header.
pub fn activity_card(
    title: impl Into<SharedString>,
    state: impl Into<SharedString>,
    tone: Tone,
    icon: IconName,
) -> Div {
    card().rounded(theme::RADIUS_LARGE).overflow_hidden().child(
        div()
            .p_4()
            .flex()
            .items_center()
            .gap_3()
            .child(
                div()
                    .size(px(36.0))
                    .flex_none()
                    .rounded(theme::RADIUS_MEDIUM)
                    .bg(tone.background())
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        gpui_kit::component::Icon::new(icon)
                            .size(px(18.0))
                            .text_color(tone.foreground()),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::text_primary())
                    .child(title.into()),
            )
            .child(badge(state, tone)),
    )
}

/// A bounded inspector surface isolates both scrolling and pointer hit testing
/// from the file list below it, including at the ends of its scroll range.
pub fn inspector_panel(id: &'static str, width: f32) -> Stateful<Div> {
    div()
        .id(id)
        .w(px(width))
        .h_full()
        .min_h_0()
        .flex_none()
        .flex()
        .flex_col()
        .overflow_hidden()
        .bg(theme::surface())
        .border_l_1()
        .border_color(theme::border())
        .occlude()
        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
}

/// The viewport owns the bounded height; content keeps its intrinsic height.
/// Sharing this primitive prevents nested wrappers from swallowing overflow.
pub fn inspector_body(
    id: &'static str,
    handle: &ScrollHandle,
    content: impl IntoElement,
) -> AnyElement {
    div()
        .id(id)
        .relative()
        .flex_1()
        .min_h_0()
        .min_w_0()
        .overflow_y_scroll()
        .track_scroll(handle)
        .child(div().w_full().h_auto().child(content))
        .vertical_scrollbar(handle)
        .into_any_element()
}

pub fn button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    icon: Option<IconName>,
    primary: bool,
) -> Button {
    Button::new(id)
        .h(px(34.0))
        .rounded(theme::RADIUS_SMALL)
        .label(label)
        .when_some(icon, |button, icon| button.icon(icon))
        .when(primary, |button| button.primary())
}

pub fn icon_button(
    id: impl Into<ElementId>,
    icon: impl Into<gpui_kit::component::Icon>,
    tooltip: impl Into<SharedString>,
) -> Button {
    let tooltip = tooltip.into();
    Button::new(id)
        .accessibility_label(tooltip.clone())
        .size(px(34.0))
        .rounded(theme::RADIUS_SMALL)
        .icon(icon)
        .tooltip(tooltip)
}

/// Compact desktop actions share one size and typography across dense toolbars.
pub fn compact_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    icon: Option<IconName>,
    primary: bool,
) -> Button {
    button(id, label, icon, primary)
        .xsmall()
        .h(px(theme::COMPACT_CONTROL_HEIGHT))
        .text_size(px(12.0))
}

/// Shared geometry and typography for data rows, menu entries and summaries.
pub fn list_row() -> Div {
    div()
        .h(theme::ROW_HEIGHT)
        .min_h(theme::ROW_HEIGHT)
        .max_h(theme::ROW_HEIGHT)
        .flex_none()
        .flex()
        .items_center()
        .gap_2()
        .text_size(theme::LIST_TEXT_SIZE)
        .line_height(theme::LIST_LINE_HEIGHT)
}

pub fn list_icon_button(
    id: impl Into<ElementId>,
    icon: impl Into<gpui_kit::component::Icon>,
    tooltip: impl Into<SharedString>,
) -> Button {
    icon_button(id, icon.into().size(theme::LIST_ICON_SIZE), tooltip)
        .xsmall()
        .ghost()
        .size(theme::LIST_CONTROL_SIZE)
        .text_size(theme::LIST_TEXT_SIZE)
}

/// Menu labels align to the same text column, including short channel names.
pub fn list_navigation_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    icon: Option<gpui_kit::component::Icon>,
) -> Button {
    let label = label.into();
    button(id, "", None, false)
        .xsmall()
        .ghost()
        .w_full()
        .h(theme::ROW_HEIGHT)
        .accessibility_label(label.clone())
        .when_some(icon, |button, icon| {
            button.icon(icon.size(theme::LIST_ICON_SIZE))
        })
        .when(!label.is_empty(), |button| {
            button.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_left()
                    .truncate()
                    .text_size(theme::LIST_TEXT_SIZE)
                    .line_height(theme::LIST_LINE_HEIGHT)
                    .child(label),
            )
        })
}

/// Keep the complete value available when a dense summary is truncated.
pub fn list_summary(id: impl Into<ElementId>, summary: impl Into<SharedString>) -> Stateful<Div> {
    let summary = summary.into();
    list_row()
        .id(id)
        .w_full()
        .child(div().flex_1().min_w_0().truncate().child(summary.clone()))
        .tooltip(move |window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(summary.clone()).build(window, cx)
        })
}

pub fn badge(label: impl Into<SharedString>, tone: Tone) -> Div {
    div()
        .flex_none()
        .h(theme::LIST_BADGE_HEIGHT)
        .px_2()
        .flex()
        .items_center()
        .rounded(theme::RADIUS_SMALL)
        .bg(tone.background())
        .text_color(tone.foreground())
        .text_size(theme::LIST_SECONDARY_TEXT_SIZE)
        .line_height(theme::LIST_LINE_HEIGHT)
        .font_weight(FontWeight::MEDIUM)
        .child(label.into())
}

pub fn progress(value: f32, tone: Tone, label: impl Into<SharedString>) -> Progress {
    Progress::new("file-progress")
        .h(px(4.0))
        .color(tone.foreground())
        .accessibility_label(label)
        .value(value)
}

pub fn section_title(title: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(18.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme::text_primary())
        .child(title.into())
}

/// Fixed page chrome shared by browsing, detail and preference routes.
pub fn page_toolbar(padding: f32) -> Div {
    div()
        .flex_none()
        .min_h(px(58.0))
        .px(px(padding))
        .py_2()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_3()
}

/// Original TeleArk artwork, clipped by the native view at every display size.
pub fn app_mark(size: f32) -> Div {
    div()
        .size(px(size))
        .flex_none()
        .rounded(px(size * 0.24))
        .overflow_hidden()
        .child(
            gpui_kit::img("teleark/app-icon.png")
                .size_full()
                .rounded(px(size * 0.24)),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit as gpui;
    use gpui_kit::{ScrollDelta, ScrollWheelEvent, TestAppContext, point, size};

    struct InspectorFixture {
        main: ScrollHandle,
        detail: ScrollHandle,
    }
    impl gpui::Render for InspectorFixture {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            div()
                .w(px(800.0))
                .h(px(400.0))
                .relative()
                .child(
                    div()
                        .id("underlying-list")
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&self.main)
                        .child(div().w_full().h(px(1600.0))),
                )
                .child(
                    inspector_panel("test-inspector", 300.0)
                        .absolute()
                        .right_0()
                        .top_0()
                        .child(div().h(px(48.0)).flex_none())
                        .child(inspector_body(
                            "test-inspector-body",
                            &self.detail,
                            div().w_full().h(px(1400.0)),
                        )),
                )
        }
    }

    #[gpui::test]
    fn inspector_content_scrolls_without_moving_the_list_at_either_boundary(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let main = ScrollHandle::new();
        let detail = ScrollHandle::new();
        let (_, cx) = cx.add_window_view(|_, _| InspectorFixture {
            main: main.clone(),
            detail: detail.clone(),
        });
        cx.simulate_resize(size(px(800.0), px(400.0)));
        cx.run_until_parked();
        for delta in [-120.0, -4000.0, -120.0, 4000.0, 120.0] {
            let before = detail.offset();
            cx.simulate_event(ScrollWheelEvent {
                position: point(px(650.0), px(200.0)),
                delta: ScrollDelta::Pixels(point(px(0.0), px(delta))),
                ..Default::default()
            });
            assert_eq!(
                main.offset().y,
                px(0.0),
                "inspector wheel leaked to the list"
            );
            if delta == -120.0 && before.y == px(0.0) {
                assert!(
                    detail.offset().y < px(0.0),
                    "bounded inspector content must scroll"
                );
            }
            cx.run_until_parked();
        }
        assert_eq!(detail.offset().y, px(0.0));
        cx.simulate_event(ScrollWheelEvent {
            position: point(px(20.0), px(200.0)),
            delta: ScrollDelta::Pixels(point(px(0.0), px(-120.0))),
            ..Default::default()
        });
        assert!(
            main.offset().y < px(0.0),
            "list should still scroll outside the inspector"
        );
    }
}

/// Fixed list summaries stay on one line, including in compact windows.
/// Horizontal overflow remains reachable without growing or shrinking the bar.
pub fn list_footer(id: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .debug_selector(move || id.into())
        .h(theme::LIST_FOOTER_HEIGHT)
        .min_h(theme::LIST_FOOTER_HEIGHT)
        .max_h(theme::LIST_FOOTER_HEIGHT)
        .flex_none()
        .px_3()
        .flex()
        .items_center()
        .gap_2()
        .whitespace_nowrap()
        .overflow_x_scroll()
        .border_t_1()
        .border_color(theme::border())
        .text_xs()
        .line_height(px(16.0))
        .text_color(theme::text_secondary())
}

#[cfg(test)]
mod list_footer_tests {
    use super::*;
    use gpui_kit::{ScrollDelta, ScrollWheelEvent, point, size};

    struct FooterFixture {
        scroll: ScrollHandle,
    }

    impl gpui_kit::Render for FooterFixture {
        fn render(
            &mut self,
            _: &mut gpui_kit::Window,
            _: &mut gpui_kit::Context<Self>,
        ) -> impl IntoElement {
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .debug_selector(|| "footer-test-list".into()),
                )
                .child(
                    list_footer("footer-test-bar")
                        .track_scroll(&self.scroll)
                        .child(
                            div()
                                .flex_none()
                                .w(px(1800.0))
                                .child("Retained tasks and omitted history"),
                        )
                        .child(
                            button("footer-test-retry", "Retry", None, false)
                                .h(px(22.0))
                                .flex_none()
                                .debug_selector(|| "footer-test-retry".into()),
                        ),
                )
        }
    }

    #[gpui_kit::test]
    fn overflowing_footer_keeps_list_space_and_scrolls_to_its_action(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let scroll = ScrollHandle::new();
        let (_, cx) = cx.add_window_view(|_, _| FooterFixture {
            scroll: scroll.clone(),
        });
        for (width, height) in [(900.0, 600.0), (1440.0, 900.0)] {
            cx.simulate_resize(size(px(width), px(height)));
            scroll.set_offset(point(px(0.0), px(0.0)));
            cx.run_until_parked();
            let footer = cx.debug_bounds("footer-test-bar").expect("footer");
            let list = cx.debug_bounds("footer-test-list").expect("list");
            assert_eq!(footer.size.height, px(24.0));
            assert_eq!(footer.bottom(), px(height));
            assert_eq!(list.bottom(), footer.top());
            cx.simulate_event(ScrollWheelEvent {
                position: footer.center(),
                delta: ScrollDelta::Pixels(point(px(-4000.0), px(0.0))),
                ..Default::default()
            });
            cx.run_until_parked();
            assert!(scroll.offset().x < px(0.0), "overflow stays reachable");
            let action = cx.debug_bounds("footer-test-retry").expect("retry");
            assert!(action.left() >= footer.left() && action.right() <= footer.right());
            assert!(action.top() >= footer.top() && action.bottom() <= footer.bottom());
            assert_eq!(
                cx.debug_bounds("footer-test-bar")
                    .expect("footer remains visible"),
                footer
            );
        }
    }

    #[gpui_kit::test]
    fn transfer_library_and_storage_summaries_stay_single_line(cx: &mut gpui_kit::TestAppContext) {
        use crate::app::Page;
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Transfers);
        for (width, height) in [(900.0, 600.0), (1440.0, 900.0)] {
            cx.simulate_resize(size(px(width), px(height)));
            for (page, selector) in [
                (Page::Transfers, "transfer-list-footer"),
                (Page::Library, "library-list-footer"),
                (Page::Storage, "storage-list-footer"),
            ] {
                app.update(cx, |app, cx| {
                    app.page = page;
                    app.vault_locked = false;
                    cx.notify();
                });
                cx.run_until_parked();
                let footer = cx.debug_bounds(selector).expect(selector);
                assert_eq!(footer.size.height, px(24.0), "{selector}");
                assert!(footer.bottom() <= px(height), "{selector} is visible");
            }
        }
    }
}
