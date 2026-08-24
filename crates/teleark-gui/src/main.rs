#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod components;
mod mock;
mod screens;
mod theme;

use app::{Page, TeleArkApp};
use gpui::{
    App, AppContext as _, Application, Bounds, KeyBinding, TitlebarOptions, WindowBounds,
    WindowOptions, px, size,
};
use gpui_component::Root;
use gpui_component_assets::Assets;
use teleark_i18n::{Localizer, SupportedLocale};

gpui::actions!(teleark, [DismissOverlay]);

fn main() {
    let launch = LaunchOptions::from_env();

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

            let bounds = Bounds::centered(None, size(px(1360.0), px(760.0)), cx);
            let window = cx.open_window(
                WindowOptions {
                    titlebar: Some(TitlebarOptions {
                        title: None,
                        appears_transparent: true,
                        ..Default::default()
                    }),
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(1120.0), px(720.0))),
                    app_id: Some("com.teleark.desktop".to_owned()),
                    ..Default::default()
                },
                move |window, cx| {
                    let view = cx.new(|cx| {
                        TeleArkApp::new(window, cx, localizer, launch.page, launch.show_upload)
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
}

impl LaunchOptions {
    fn from_env() -> Self {
        Self::from_args(std::env::args().skip(1), detect_system_locale())
    }

    fn from_args<I, S>(arguments: I, fallback_locale: SupportedLocale) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut page = Page::Library;
        let mut show_upload = false;
        let mut explicit_locale = None;

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
        }

        let locale = explicit_locale.unwrap_or(fallback_locale);
        Self {
            page,
            locale,
            show_upload,
        }
    }
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
            }
        );
    }

    #[test]
    fn upload_launch_uses_the_library_overlay() {
        let options = LaunchOptions::from_args(["--screen=upload"], SupportedLocale::JaJp);

        assert_eq!(options.page, Page::Library);
        assert_eq!(options.locale, SupportedLocale::JaJp);
        assert!(options.show_upload);
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
        assert!(!options.show_upload);
    }
}
