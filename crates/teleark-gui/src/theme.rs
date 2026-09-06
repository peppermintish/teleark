//! TeleArk's visual design tokens.
//!
//! Screen modules deliberately consume this palette instead of scattering
//! one-off color values. Geometry constants live here for the same reason.

use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::component::{Theme as ComponentTheme, ThemeMode};
use gpui_kit::{Context, Pixels, Rgba, Window, WindowAppearance, px, rgb};
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
    theme.colors.primary_foreground = rgb(0xffffff).into();
    theme.colors.ring = blue().into();
    theme.colors.border = border().into();
    theme.colors.input = color(0xc7cbd3, 0x66666e).into();
    theme.colors.background = canvas().into();
    theme.colors.foreground = text_primary().into();
    theme.colors.sidebar = sidebar().into();
    theme.colors.button_primary = theme.colors.primary;
    theme.colors.button_primary_foreground = theme.colors.primary_foreground;
    theme.colors.button_primary_hover = theme.colors.primary_hover;
    theme.colors.button_primary_active = theme.colors.primary_active;
    theme.colors.button = surface().into();
    theme.colors.button_foreground = text_primary().into();
    theme.colors.button_hover = canvas().into();
    theme.colors.button_active = border().into();
    theme.colors.muted_foreground = text_secondary().into();
    theme.colors.popover = surface().into();
    theme.colors.popover_foreground = text_primary().into();
    theme.colors.progress_bar = blue().into();
    theme.colors.selection = blue_soft().into();
    theme.colors.sidebar_accent = blue_soft().into();
    theme.colors.sidebar_foreground = text_secondary().into();
    theme.colors.sidebar_accent_foreground = blue().into();
    theme.colors.table = surface().into();
    theme.colors.table_head = sidebar().into();
    theme.colors.table_head_foreground = text_secondary().into();
    theme.colors.table_active = blue_soft().into();
    theme.colors.table_hover = blue_pale().into();
    theme.colors.table_active_border = blue_soft().into();
    theme.colors.table_row_border = border_subtle().into();
    theme.colors.tab_bar_segmented = sidebar().into();
    theme.colors.tab_active = surface().into();
    theme.colors.tab_foreground = text_secondary().into();
    theme.colors.tab_active_foreground = text_primary().into();
    theme.colors.switch = blue().into();
    theme.tokens = gpui_kit::component::ThemeTokens::from(theme.colors);
    theme.radius = RADIUS_SMALL;
    ComponentTheme::sync_base(cx);
}

fn color(light: u32, dark: u32) -> Rgba {
    rgb(if DARK_PALETTE.load(Ordering::Relaxed) {
        dark
    } else {
        light
    })
}

pub fn canvas() -> Rgba {
    color(0xf8f9fb, 0x1c1c1e)
}

pub fn surface() -> Rgba {
    color(0xffffff, 0x262628)
}

pub fn sidebar() -> Rgba {
    color(0xf0f1f4, 0x222224)
}

pub fn text_primary() -> Rgba {
    color(0x202124, 0xf5f5f7)
}

pub fn text_secondary() -> Rgba {
    color(0x6e727b, 0xb3b3bc)
}

pub fn text_muted() -> Rgba {
    color(0x7e838c, 0x9898a2)
}

pub fn border() -> Rgba {
    color(0xe3e5ea, 0x3b3b40)
}

pub fn border_subtle() -> Rgba {
    color(0xf0f1f4, 0x303034)
}

pub fn blue() -> Rgba {
    color(0x007aff, 0x0a84ff)
}

pub fn blue_soft() -> Rgba {
    color(0xe1edff, 0x17375a)
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
