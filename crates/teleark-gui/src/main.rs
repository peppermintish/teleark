#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod components;
mod layout;
mod library_state;
mod menus;
mod mock;
mod screens;
mod theme;

use app::{AppStartup, LocaleStartup, Page, RuntimeStartup, TeleArkApp};
use gpui::{
    App, AppContext as _, Application, Bounds, KeyBinding, Pixels, Size, TitlebarOptions,
    WindowBounds, WindowOptions, point, px, size,
};
use gpui_component::Root;
use gpui_component_assets::Assets;
use teleark_core::{ApplicationError, ApplicationErrorKind};
use teleark_i18n::{Localizer, SupportedLocale};
use teleark_runtime::{DesktopLibrary, DesktopTelegram, DesktopTransfers, initialize_diagnostics};

gpui::actions!(
    teleark,
    [
        DismissOverlay,
        MinimizeWindow,
        Quit,
        ToggleFullscreen,
        ZoomWindow
    ]
);

fn main() {
    let system_locale = detect_system_locale();
    let library = DesktopLibrary::open_default();
    let _diagnostics = library
        .as_ref()
        .map_err(|error| ApplicationError::new(error.kind()))
        .and_then(|library| library.managed_directories())
        .and_then(|directories| initialize_diagnostics(&directories.logs));
    tracing::info!(
        event = "application.starting",
        version = env!("CARGO_PKG_VERSION"),
        "TeleArk application starting"
    );
    let telegram = DesktopTelegram::open_default();
    let transfers = match (telegram.as_ref(), library.as_ref()) {
        (Ok(telegram), Ok(library)) => DesktopTransfers::new(telegram.clone(), library.clone()),
        (Err(error), _) | (_, Err(error)) => Err(ApplicationError::new(match error.kind() {
            ApplicationErrorKind::Persistence => ApplicationErrorKind::Persistence,
            _ => ApplicationErrorKind::Network,
        })),
    };
    let mut launch = LaunchOptions::from_env(system_locale);
    let persisted_locale = library
        .as_ref()
        .ok()
        .and_then(|library| library.locale_override().ok())
        .flatten();
    let follows_system_locale = apply_persisted_locale(&mut launch, persisted_locale.as_deref());

    Application::new()
        .with_assets(Assets)
        .run(move |cx: &mut App| {
            gpui_component::init(cx);
            cx.bind_keys([
                KeyBinding::new("escape", DismissOverlay, None),
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
            cx.on_window_closed(|cx| {
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
                .map(|display| fit_window_bounds(requested_size, display.bounds()))
                .unwrap_or_else(|| Bounds::centered(None, requested_size, cx));
            let window = cx.open_window(main_window_options(bounds), move |window, cx| {
                let view = cx.new(|cx| {
                    TeleArkApp::new(
                        window,
                        cx,
                        localizer,
                        RuntimeStartup {
                            library,
                            telegram,
                            transfers,
                        },
                        AppStartup {
                            page: launch.page,
                            show_upload: launch.show_upload,
                            skip_telegram_api_id_prompt: launch.skip_telegram_api_id_prompt,
                            locale: LocaleStartup {
                                system_locale,
                                follows_system_locale,
                            },
                        },
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
        window_min_size: Some(size(px(900.0), px(600.0))),
        app_id: Some("com.teleark.desktop".to_owned()),
        ..Default::default()
    }
}

fn fit_window_bounds(requested: Size<Pixels>, display: Bounds<Pixels>) -> Bounds<Pixels> {
    let horizontal_inset = px(16.0);
    let top_inset = px(28.0);
    let bottom_inset = px(80.0);
    let safe_width = (display.size.width - horizontal_inset * 2.0)
        .max(px(900.0))
        .min(display.size.width);
    let safe_height = (display.size.height - top_inset - bottom_inset)
        .max(px(600.0))
        .min(display.size.height);
    let safe = Bounds::new(
        point(
            display.origin.x + (display.size.width - safe_width) / 2.0,
            display.origin.y + top_inset.min((display.size.height - safe_height).max(px(0.0))),
        ),
        size(safe_width, safe_height),
    );
    let window_size = requested.min(&safe.size);
    let center = safe.center();
    let offset = window_size / 2.0;
    Bounds::new(
        point(center.x - offset.width, center.y - offset.height),
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
        let mut page = Page::Library;
        let mut show_upload = false;
        let mut explicit_locale = None;
        let mut window_size = (1360, 760);
        let mut skip_telegram_api_id_prompt = false;

        for argument in arguments {
            let argument = argument.as_ref();
            if let Some(value) = argument.strip_prefix("--screen=") {
                match value {
                    "library" => page = Page::Library,
                    "transfers" => page = Page::Transfers,
                    "file" => page = Page::FileDetail,
                    "vault" => page = Page::Vault,
                    "channel" => page = Page::Channel,
                    "settings" => page = Page::Settings,
                    "upload" => {
                        page = Page::Library;
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
                window_width: 1360,
                window_height: 760,
                locale_from_command_line: true,
                skip_telegram_api_id_prompt: false,
            }
        );
    }

    #[test]
    fn upload_launch_uses_the_library_overlay() {
        let options = LaunchOptions::from_args(["--screen=upload"], SupportedLocale::JaJp);

        assert_eq!(options.page, Page::Library);
        assert_eq!(options.locale, SupportedLocale::JaJp);
        assert!(options.show_upload);
        assert!(!options.locale_from_command_line);
        assert_eq!((options.window_width, options.window_height), (1360, 760));
    }

    #[test]
    fn visual_test_launch_can_take_the_same_session_only_skip_path_as_the_dialog() {
        let options =
            LaunchOptions::from_args(["--skip-telegram-api-id-prompt"], SupportedLocale::EnUs);

        assert!(options.skip_telegram_api_id_prompt);
    }

    #[test]
    fn unknown_values_preserve_the_last_valid_choice() {
        let options = LaunchOptions::from_args(
            [
                "--screen=vault",
                "--screen=unknown",
                "--locale=ja",
                "--locale=invalid",
            ],
            SupportedLocale::EnUs,
        );

        assert_eq!(options.page, Page::Vault);
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
    fn oversized_windows_stay_inside_desktop_safe_insets() {
        let display = Bounds::new(point(px(0.0), px(0.0)), size(px(1_440.0), px(900.0)));
        let bounds = fit_window_bounds(size(px(1_680.0), px(960.0)), display);
        assert_eq!(bounds.origin, point(px(16.0), px(28.0)));
        assert_eq!(bounds.size, size(px(1_408.0), px(792.0)));

        let compact = fit_window_bounds(size(px(900.0), px(600.0)), display);
        assert_eq!(compact.size, size(px(900.0), px(600.0)));
        assert!(compact.origin.y >= px(28.0));
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
