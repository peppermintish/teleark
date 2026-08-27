#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod components;
mod layout;
mod library_state;
mod mock;
mod screens;
mod theme;

use app::{LocaleStartup, Page, RuntimeStartup, TeleArkApp};
use gpui::{
    App, AppContext as _, Application, Bounds, KeyBinding, TitlebarOptions, WindowBounds,
    WindowOptions, px, size,
};
use gpui_component::Root;
use gpui_component_assets::Assets;
use teleark_i18n::{Localizer, SupportedLocale};
use teleark_runtime::{DesktopLibrary, DesktopTelegram};

gpui::actions!(teleark, [DismissOverlay]);

fn main() {
    let system_locale = detect_system_locale();
    let library = DesktopLibrary::open_default();
    let telegram = DesktopTelegram::open_default();
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
            cx.bind_keys([KeyBinding::new("escape", DismissOverlay, None)]);

            let localizer = match Localizer::new(launch.locale) {
                Ok(localizer) => localizer,
                Err(error) => {
                    eprintln!("failed to initialize TeleArk localization resources: {error}");
                    cx.quit();
                    return;
                }
            };

            let requested_size = size(
                px(launch.window_width as f32),
                px(launch.window_height as f32),
            );
            let window_size = cx
                .primary_display()
                .map(|display| requested_size.min(&display.bounds().size))
                .unwrap_or(requested_size);
            let bounds = Bounds::centered(None, window_size, cx);
            let window = cx.open_window(
                WindowOptions {
                    titlebar: Some(TitlebarOptions {
                        title: None,
                        appears_transparent: true,
                        ..Default::default()
                    }),
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(900.0), px(600.0))),
                    app_id: Some("com.teleark.desktop".to_owned()),
                    ..Default::default()
                },
                move |window, cx| {
                    let view = cx.new(|cx| {
                        TeleArkApp::new(
                            window,
                            cx,
                            localizer,
                            RuntimeStartup { library, telegram },
                            launch.page,
                            launch.show_upload,
                            LocaleStartup {
                                system_locale,
                                follows_system_locale,
                            },
                        )
                    });
                    cx.new(|cx| Root::new(view, window, cx))
                },
            );

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LaunchOptions {
    page: Page,
    locale: SupportedLocale,
    show_upload: bool,
    window_width: u32,
    window_height: u32,
    locale_from_command_line: bool,
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
        }

        let locale = explicit_locale.unwrap_or(fallback_locale);
        Self {
            page,
            locale,
            show_upload,
            window_width: window_size.0,
            window_height: window_size.1,
            locale_from_command_line: explicit_locale.is_some(),
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
