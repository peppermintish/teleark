//! TeleArk's visual design tokens.
//!
//! Screen modules deliberately consume this palette instead of scattering
//! one-off color values. Geometry constants live here for the same reason.

use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{Context, Pixels, Rgba, Window, WindowAppearance, px, rgb};
use gpui_component::{Theme as ComponentTheme, ThemeMode};
use teleark_runtime::AppearancePreference;

static DARK_PALETTE: AtomicBool = AtomicBool::new(false);

pub const HEADER_HEIGHT: Pixels = px(58.0);
pub const ROW_HEIGHT: Pixels = px(42.0);
pub const RADIUS_SMALL: Pixels = px(6.0);
pub const RADIUS_MEDIUM: Pixels = px(8.0);
pub const RADIUS_LARGE: Pixels = px(12.0);

pub fn apply_appearance<T>(
    preference: AppearancePreference,
    window: &mut Window,
    cx: &mut Context<T>,
) {
    let dark = match preference {
        AppearancePreference::System => matches!(
            window.appearance(),
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        ),
        AppearancePreference::Light => false,
        AppearancePreference::Dark => true,
    };
    DARK_PALETTE.store(dark, Ordering::Relaxed);
    match preference {
        AppearancePreference::System => ComponentTheme::sync_system_appearance(Some(window), cx),
        AppearancePreference::Light => ComponentTheme::change(ThemeMode::Light, Some(window), cx),
        AppearancePreference::Dark => ComponentTheme::change(ThemeMode::Dark, Some(window), cx),
    }
    let theme = ComponentTheme::global_mut(cx);
    theme.colors.primary = blue().into();
    theme.colors.primary_hover = color(0x1d4ed8, 0x8cb5ff).into();
    theme.colors.primary_active = color(0x1e40af, 0x5b92ee).into();
    theme.colors.primary_foreground = color(0xffffff, 0x101522).into();
    theme.colors.ring = blue().into();
    theme.colors.border = border().into();
    theme.radius = RADIUS_SMALL;
}

fn color(light: u32, dark: u32) -> Rgba {
    rgb(if DARK_PALETTE.load(Ordering::Relaxed) {
        dark
    } else {
        light
    })
}

pub fn canvas() -> Rgba {
    color(0xf6f7f9, 0x11151d)
}

pub fn surface() -> Rgba {
    color(0xffffff, 0x1b202b)
}

pub fn sidebar() -> Rgba {
    color(0xf0f2f5, 0x161b24)
}

pub fn text_primary() -> Rgba {
    color(0x182033, 0xf3f5f7)
}

pub fn text_secondary() -> Rgba {
    color(0x667085, 0xaab4c8)
}

pub fn text_muted() -> Rgba {
    color(0x748094, 0x98a4b8)
}

pub fn border() -> Rgba {
    color(0xe2e7ef, 0x2b3549)
}

pub fn border_subtle() -> Rgba {
    color(0xedf1f6, 0x222c3e)
}

pub fn blue() -> Rgba {
    color(0x2463eb, 0x76a7ff)
}

pub fn blue_soft() -> Rgba {
    color(0xeaf3ff, 0x203a5f)
}

pub fn blue_pale() -> Rgba {
    color(0xf3f8ff, 0x192c48)
}

pub fn green() -> Rgba {
    color(0x19a45b, 0x41c97c)
}

pub fn green_soft() -> Rgba {
    color(0xeaf8f0, 0x183b2b)
}

pub fn amber() -> Rgba {
    color(0xf5a524, 0xffb84d)
}

pub fn amber_soft() -> Rgba {
    color(0xfff6e6, 0x49351a)
}

pub fn red() -> Rgba {
    color(0xee4455, 0xff6b78)
}

pub fn red_soft() -> Rgba {
    color(0xffeef0, 0x48232d)
}

pub fn purple() -> Rgba {
    color(0x7c5ce7, 0xa58aff)
}

pub fn purple_soft() -> Rgba {
    color(0xf2edff, 0x30294f)
}

pub fn cyan() -> Rgba {
    color(0x18a4c7, 0x45c4e2)
}

pub fn cyan_soft() -> Rgba {
    color(0xe8f8fb, 0x173946)
}
