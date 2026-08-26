//! TeleArk's visual design tokens.
//!
//! Screen modules deliberately consume this palette instead of scattering
//! one-off color values. Geometry constants live here for the same reason.

use gpui::{Pixels, Rgba, px, rgb};

pub const HEADER_HEIGHT: Pixels = px(58.0);
pub const ROW_HEIGHT: Pixels = px(42.0);
pub const RADIUS_SMALL: Pixels = px(6.0);
pub const RADIUS_MEDIUM: Pixels = px(10.0);
pub const RADIUS_LARGE: Pixels = px(14.0);

pub fn canvas() -> Rgba {
    rgb(0xf5f7fb)
}

pub fn surface() -> Rgba {
    rgb(0xffffff)
}

pub fn sidebar() -> Rgba {
    rgb(0xf8fafc)
}

pub fn text_primary() -> Rgba {
    rgb(0x182033)
}

pub fn text_secondary() -> Rgba {
    rgb(0x667085)
}

pub fn text_muted() -> Rgba {
    rgb(0x98a2b3)
}

pub fn border() -> Rgba {
    rgb(0xe2e7ef)
}

pub fn border_subtle() -> Rgba {
    rgb(0xedf1f6)
}

pub fn blue() -> Rgba {
    rgb(0x1677ff)
}

pub fn blue_soft() -> Rgba {
    rgb(0xeaf3ff)
}

pub fn blue_pale() -> Rgba {
    rgb(0xf3f8ff)
}

pub fn green() -> Rgba {
    rgb(0x19a45b)
}

pub fn green_soft() -> Rgba {
    rgb(0xeaf8f0)
}

pub fn amber() -> Rgba {
    rgb(0xf5a524)
}

pub fn amber_soft() -> Rgba {
    rgb(0xfff6e6)
}

pub fn red() -> Rgba {
    rgb(0xee4455)
}

pub fn red_soft() -> Rgba {
    rgb(0xffeef0)
}

pub fn purple() -> Rgba {
    rgb(0x7c5ce7)
}

pub fn purple_soft() -> Rgba {
    rgb(0xf2edff)
}

pub fn cyan() -> Rgba {
    rgb(0x18a4c7)
}

pub fn cyan_soft() -> Rgba {
    rgb(0xe8f8fb)
}
