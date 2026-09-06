use gpui::{
    Div, ElementId, FontWeight, ParentElement as _, Rgba, SharedString, Styled as _, div,
    prelude::FluentBuilder as _, px,
};
use gpui_component::{
    IconName,
    button::{Button, ButtonVariants as _},
    progress::Progress,
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
    icon: IconName,
    tooltip: impl Into<SharedString>,
) -> Button {
    Button::new(id)
        .size(px(34.0))
        .rounded(theme::RADIUS_SMALL)
        .icon(icon)
        .tooltip(tooltip)
}

pub fn badge(label: impl Into<SharedString>, tone: Tone) -> Div {
    div()
        .flex_none()
        .h(px(22.0))
        .px_2()
        .flex()
        .items_center()
        .rounded(theme::RADIUS_SMALL)
        .bg(tone.background())
        .text_color(tone.foreground())
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .child(label.into())
}

pub fn progress(value: f32, tone: Tone) -> Progress {
    Progress::new()
        .h(px(4.0))
        .bg(tone.foreground())
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
