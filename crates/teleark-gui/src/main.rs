#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod assets;
mod components;
mod layout;
mod library_state;
mod menus;
mod mock;
mod screens;
mod startup;
mod theme;

use app::Page;
use assets::Assets;
use gpui_kit::component::Root;
use gpui_kit::{
    App, AppContext as _, Bounds, KeyBinding, Pixels, Size, TitlebarOptions, WindowBounds,
    WindowOptions, point, px, size,
};
use teleark_i18n::{Localizer, SupportedLocale};

gpui_kit::actions!(
    teleark,
    [
        DismissOverlay,
        ShowAbout,
        ShowSettings,
        ShowTransfers,
        ShowStorage,
        UploadFile,
        FocusSearch,
        RefreshPage,
        MinimizeWindow,
        Quit,
        ToggleFullscreen,
        ZoomWindow
    ]
);

// All app networking belongs to Runtime/Telegram. Remote UI assets must use
// that owner; framework HTTP is denied in both direct and proxy modes.
fn application_http_client() -> std::sync::Arc<dyn gpui_kit::http_client::HttpClient> {
    std::sync::Arc::new(gpui_kit::http_client::BlockedHttpClient::new())
}

fn main() {
    let system_locale = detect_system_locale();
    let visual_preview = std::env::args().any(|argument| argument == "--preview-ui");
    let launch = LaunchOptions::from_env(system_locale);

    gpui_kit::application()
        .with_http_client(application_http_client())
        .with_assets(Assets)
        .run(move |cx: &mut App| {
            gpui_kit::init(cx);
            cx.bind_keys([
                KeyBinding::new("escape", DismissOverlay, None),
                KeyBinding::new("cmd-,", ShowSettings, None),
                KeyBinding::new("cmd-1", ShowTransfers, None),
                KeyBinding::new("cmd-2", ShowStorage, None),
                KeyBinding::new("cmd-u", UploadFile, None),
                KeyBinding::new("cmd-f", FocusSearch, None),
                KeyBinding::new("cmd-r", RefreshPage, None),
                KeyBinding::new("cmd-m", MinimizeWindow, None),
                KeyBinding::new("cmd-q", Quit, None),
                KeyBinding::new("ctrl-cmd-f", ToggleFullscreen, None),
            ]);

            let localizer = match Localizer::new(launch.locale) {
                Ok(localizer) => localizer,
                Err(error) => {
                    eprintln!("failed to initialize TeleArk localization resources: {error}");
                    cx.quit();
                    return;
                }
            };
            cx.on_action(quit);
            cx.on_action(minimize_window);
            cx.on_action(toggle_fullscreen);
            cx.on_action(zoom_window);
            cx.set_menus(menus::application_menus(&localizer));
            cx.on_window_closed(|cx, _| {
                if should_quit_after_window_close(cx.windows().len()) {
                    cx.quit();
                }
            })
            .detach();
            cx.activate(true);

            let requested_size = size(
                px(launch.window_width as f32),
                px(launch.window_height as f32),
            );
            let bounds = cx
                .primary_display()
                .map(|display| fit_window_bounds(requested_size, display.visible_bounds()))
                .unwrap_or_else(|| Bounds::centered(None, requested_size, cx));
            let window = cx.open_window(main_window_options(bounds), move |window, cx| {
                let view = cx.new(|cx| {
                    startup::StartupView::new(
                        localizer,
                        launch,
                        system_locale,
                        visual_preview,
                        window,
                        cx,
                    )
                });
                cx.new(|cx| Root::new(view, window, cx))
            });

            match window {
                Ok(window) => {
                    let _ = window.update(cx, |_, _, cx| cx.activate(true));
                }
                Err(error) => {
                    eprintln!("failed to open the TeleArk window: {error}");
                    cx.quit();
                }
            }
        });
}

fn quit(_: &Quit, cx: &mut App) {
    cx.quit();
}

fn should_quit_after_window_close(open_window_count: usize) -> bool {
    open_window_count == 0
}

fn minimize_window(_: &MinimizeWindow, cx: &mut App) {
    if let Some(handle) = cx.active_window() {
        let _ = handle.update(cx, |_, window, _| window.minimize_window());
    }
}

fn toggle_fullscreen(_: &ToggleFullscreen, cx: &mut App) {
    if let Some(handle) = cx.active_window() {
        let _ = handle.update(cx, |_, window, _| window.toggle_fullscreen());
    }
}

fn zoom_window(_: &ZoomWindow, cx: &mut App) {
    if let Some(handle) = cx.active_window() {
        let _ = handle.update(cx, |_, window, _| window.zoom_window());
    }
}

fn main_window_options(bounds: Bounds<Pixels>) -> WindowOptions {
    WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some("TeleArk".into()),
            // Keep the native macOS titlebar so the traffic lights and the
            // top-edge full-screen reveal remain owned by AppKit.
            appears_transparent: false,
            ..Default::default()
        }),
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(900.0), px(600.0)).min(&bounds.size)),
        app_id: Some("com.teleark.desktop".to_owned()),
        ..Default::default()
    }
}

// GPUI takes a content size, then AppKit adds the native titlebar. Keep a
// conservative decoration allowance separate from the OS-reported work area.
const NATIVE_TITLEBAR_RESERVE: f32 = 36.0;

fn fit_window_bounds(requested: Size<Pixels>, visible: Bounds<Pixels>) -> Bounds<Pixels> {
    let margin = px(16.0);
    let titlebar = px(NATIVE_TITLEBAR_RESERVE).min(visible.size.height);
    let available = size(
        (visible.size.width - margin * 2.0).max(px(1.0)),
        (visible.size.height - titlebar - margin * 2.0).max(px(1.0)),
    );
    let window_size = requested.min(&available);
    // Center the complete native frame, including its titlebar. Never expand
    // back to the nominal minimum when that would overlap the Dock/taskbar.
    Bounds::new(
        point(
            visible.origin.x + (visible.size.width - window_size.width) / 2.0,
            visible.origin.y + (visible.size.height - window_size.height - titlebar) / 2.0,
        ),
        window_size,
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LaunchOptions {
    page: Page,
    locale: SupportedLocale,
    show_upload: bool,
    window_width: u32,
    window_height: u32,
    locale_from_command_line: bool,
    skip_telegram_api_id_prompt: bool,
}

impl LaunchOptions {
    fn from_env(fallback_locale: SupportedLocale) -> Self {
        Self::from_args(std::env::args().skip(1), fallback_locale)
    }

    fn from_args<I, S>(arguments: I, fallback_locale: SupportedLocale) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut page = Page::Account;
        let mut show_upload = false;
        let mut explicit_locale = None;
        let mut window_size = (1120, 680);
        let mut skip_telegram_api_id_prompt = false;

        for argument in arguments {
            let argument = argument.as_ref();
            if let Some(value) = argument.strip_prefix("--screen=") {
                match value {
                    "account" => page = Page::Account,
                    "storage" => page = Page::Storage,
                    "library" => page = Page::Library,
                    "transfers" => page = Page::Transfers,
                    "file" => page = Page::FileDetail,
                    "channel" => page = Page::Channel,
                    "settings" => page = Page::Settings,
                    "upload" => {
                        page = Page::Storage;
                        show_upload = true;
                    }
                    _ => {}
                }
            }

            if let Some(locale) = argument.strip_prefix("--locale=").and_then(parse_locale) {
                explicit_locale = Some(locale);
            }

            if let Some(size) = argument
                .strip_prefix("--window-size=")
                .and_then(parse_window_size)
            {
                window_size = size;
            }

            if argument == "--skip-telegram-api-id-prompt" {
                skip_telegram_api_id_prompt = true;
            }
        }

        let locale = explicit_locale.unwrap_or(fallback_locale);
        Self {
            page,
            locale,
            show_upload,
            window_width: window_size.0,
            window_height: window_size.1,
            locale_from_command_line: explicit_locale.is_some(),
            skip_telegram_api_id_prompt,
        }
    }
}

fn parse_window_size(value: &str) -> Option<(u32, u32)> {
    let (width, height) = value.split_once('x').or_else(|| value.split_once('X'))?;
    let width = width.parse::<u32>().ok()?;
    let height = height.parse::<u32>().ok()?;
    Some((width.max(900), height.max(600)))
}

fn parse_locale(value: &str) -> Option<SupportedLocale> {
    let language = value
        .split(['.', '@'])
        .next()
        .unwrap_or(value)
        .split(['-', '_'])
        .next()
        .unwrap_or(value)
        .to_ascii_lowercase();
    let negotiated = SupportedLocale::negotiate([value]);
    match (language.as_str(), negotiated) {
        ("en", SupportedLocale::EnUs) => Some(SupportedLocale::EnUs),
        ("zh", SupportedLocale::ZhCn) => Some(SupportedLocale::ZhCn),
        ("ja", SupportedLocale::JaJp) => Some(SupportedLocale::JaJp),
        _ => None,
    }
}

fn detect_system_locale() -> SupportedLocale {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .filter_map(|name| std::env::var(name).ok())
        .find_map(|value| parse_locale(value.split('.').next().unwrap_or(&value)))
        .unwrap_or(SupportedLocale::EnUs)
}

fn apply_persisted_locale(launch: &mut LaunchOptions, persisted: Option<&str>) -> bool {
    if launch.locale_from_command_line {
        return false;
    }
    if let Some(locale) = persisted.and_then(parse_locale) {
        launch.locale = locale;
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_aliases_follow_the_supported_product_locales() {
        assert_eq!(parse_locale("en_AU"), Some(SupportedLocale::EnUs));
        assert_eq!(parse_locale("zh-Hans"), Some(SupportedLocale::ZhCn));
        assert_eq!(parse_locale("zh-Hans-CN"), Some(SupportedLocale::ZhCn));
        assert_eq!(parse_locale("zh-Hant-TW"), None);
        assert_eq!(parse_locale("ja_JP"), Some(SupportedLocale::JaJp));
        assert_eq!(parse_locale("fr-FR"), None);
    }

    #[test]
    fn launch_arguments_select_a_screen_and_locale() {
        let options = LaunchOptions::from_args(
            ["--screen=transfers", "--locale=zh-CN"],
            SupportedLocale::EnUs,
        );

        assert_eq!(
            options,
            LaunchOptions {
                page: Page::Transfers,
                locale: SupportedLocale::ZhCn,
                show_upload: false,
                window_width: 1120,
                window_height: 680,
                locale_from_command_line: true,
                skip_telegram_api_id_prompt: false,
            }
        );
    }

    #[test]
    fn upload_launch_uses_the_storage_overlay() {
        let options = LaunchOptions::from_args(["--screen=upload"], SupportedLocale::JaJp);

        assert_eq!(options.page, Page::Storage);
        assert_eq!(options.locale, SupportedLocale::JaJp);
        assert!(options.show_upload);
        assert!(!options.locale_from_command_line);
        assert_eq!((options.window_width, options.window_height), (1120, 680));
    }

    #[test]
    fn visual_test_launch_can_take_the_same_session_only_skip_path_as_the_dialog() {
        let options =
            LaunchOptions::from_args(["--skip-telegram-api-id-prompt"], SupportedLocale::EnUs);

        assert!(options.skip_telegram_api_id_prompt);
    }

    #[test]
    fn removed_vault_and_unknown_routes_preserve_the_default_choice() {
        let options = LaunchOptions::from_args(
            [
                "--screen=vault",
                "--screen=unknown",
                "--locale=ja",
                "--locale=invalid",
            ],
            SupportedLocale::EnUs,
        );

        assert_eq!(options.page, Page::Account);
        assert_eq!(options.locale, SupportedLocale::JaJp);
        assert!(options.locale_from_command_line);
        assert!(!options.show_upload);
    }

    #[test]
    fn window_size_argument_supports_the_visual_test_matrix() {
        let compact = LaunchOptions::from_args(["--window-size=960x640"], SupportedLocale::EnUs);
        let spacious = LaunchOptions::from_args(["--window-size=1920X1080"], SupportedLocale::EnUs);

        assert_eq!((compact.window_width, compact.window_height), (960, 640));
        assert_eq!(
            (spacious.window_width, spacious.window_height),
            (1920, 1080)
        );
    }

    #[test]
    fn initial_native_frame_stays_in_the_os_work_area() {
        // Normal/tall bottom Dock, left/right Dock and an offset work area.
        for (x, y, width, height) in [
            (0.0, 25.0, 1440.0, 795.0),
            (0.0, 38.0, 1440.0, 682.0),
            (126.0, 25.0, 1314.0, 875.0),
            (0.0, 25.0, 1314.0, 875.0),
            (-1280.0, 104.0, 1280.0, 680.0),
        ] {
            let visible = Bounds::new(point(px(x), px(y)), size(px(width), px(height)));
            for requested in [size(px(1120.0), px(680.0)), size(px(1920.0), px(1080.0))] {
                let bounds = fit_window_bounds(requested, visible);
                assert!(bounds.left() >= visible.left() + px(16.0));
                assert!(bounds.top() >= visible.top() + px(16.0));
                assert!(bounds.right() <= visible.right() - px(16.0));
                assert!(
                    bounds.bottom() + px(NATIVE_TITLEBAR_RESERVE) <= visible.bottom() - px(16.0)
                );
                assert!(bounds.size.width <= requested.width);
                assert!(bounds.size.height <= requested.height);
            }
            let compact = fit_window_bounds(size(px(900.0), px(600.0)), visible);
            assert_eq!(compact.size, size(px(900.0), px(600.0)));
        }
    }

    #[test]
    fn small_work_area_takes_precedence_over_nominal_window_minimum() {
        let visible = Bounds::new(point(px(100.0), px(40.0)), size(px(880.0), px(600.0)));
        let bounds = fit_window_bounds(size(px(1120.0), px(680.0)), visible);
        let options = main_window_options(bounds);
        assert_eq!(bounds.size, size(px(848.0), px(532.0)));
        assert_eq!(options.window_min_size, Some(bounds.size));
        assert!(bounds.bottom() + px(NATIVE_TITLEBAR_RESERVE) < visible.bottom());
        assert!(bounds.right() < visible.right());
    }

    #[test]
    fn default_window_is_compact_and_centers_its_native_frame() {
        let launch = LaunchOptions::from_args(std::iter::empty::<&str>(), SupportedLocale::EnUs);
        assert_eq!((launch.window_width, launch.window_height), (1120, 680));
        let visible = Bounds::new(point(px(0.0), px(25.0)), size(px(1920.0), px(975.0)));
        let bounds = fit_window_bounds(
            size(
                px(launch.window_width as f32),
                px(launch.window_height as f32),
            ),
            visible,
        );
        assert_eq!(bounds.size, size(px(1120.0), px(680.0)));
        assert_eq!(bounds.center().x, visible.center().x);
        assert_eq!(
            bounds.center().y + px(NATIVE_TITLEBAR_RESERVE / 2.0),
            visible.center().y
        );
    }

    #[test]
    fn main_window_keeps_native_fullscreen_controls_available() {
        let bounds = Bounds::new(point(px(16.0), px(28.0)), size(px(900.0), px(600.0)));
        let options = main_window_options(bounds);
        let titlebar = options.titlebar.expect("native titlebar configuration");

        assert_eq!(
            titlebar.title.map(|title| title.to_string()),
            Some("TeleArk".to_owned())
        );
        assert!(!titlebar.appears_transparent);
        assert!(options.is_resizable);
        assert!(options.is_minimizable);
        assert_eq!(options.window_bounds, Some(WindowBounds::Windowed(bounds)));
    }

    #[test]
    fn closing_the_last_window_terminates_the_application_event_loop() {
        assert!(should_quit_after_window_close(0));
        assert!(!should_quit_after_window_close(1));
        assert!(!should_quit_after_window_close(2));
    }

    #[test]
    fn requested_window_size_is_clamped_to_the_supported_minimum() {
        let options = LaunchOptions::from_args(
            ["--window-size=320x200", "--window-size=invalid"],
            SupportedLocale::EnUs,
        );

        assert_eq!((options.window_width, options.window_height), (900, 600));
    }

    #[test]
    fn persisted_locale_applies_only_without_a_command_line_override() {
        let mut persisted =
            LaunchOptions::from_args(std::iter::empty::<&str>(), SupportedLocale::EnUs);
        assert!(!apply_persisted_locale(&mut persisted, Some("ja-JP")));
        assert_eq!(persisted.locale, SupportedLocale::JaJp);

        let mut explicit = LaunchOptions::from_args(["--locale=zh-CN"], SupportedLocale::EnUs);
        assert!(!apply_persisted_locale(&mut explicit, Some("ja-JP")));
        assert_eq!(explicit.locale, SupportedLocale::ZhCn);

        let mut system =
            LaunchOptions::from_args(std::iter::empty::<&str>(), SupportedLocale::JaJp);
        assert!(apply_persisted_locale(&mut system, None));
        assert_eq!(system.locale, SupportedLocale::JaJp);
    }
}

#[cfg(test)]
mod network_policy_tests;

#[cfg(test)]
mod list_density_tests;
