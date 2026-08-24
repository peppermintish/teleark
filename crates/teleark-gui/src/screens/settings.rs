use gpui::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px,
};
use gpui_component::{Icon, IconName, scroll::ScrollableElement as _};
use teleark_i18n::SupportedLocale;

use crate::{
    app::TeleArkApp,
    components::{self, Tone},
    theme,
};

impl TeleArkApp {
    pub(crate) fn render_settings(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let toolbar = div()
            .h(px(58.0))
            .px_5()
            .flex()
            .items_center()
            .child(components::section_title(self.tr("settings-title")))
            .child(div().flex_1())
            .child(components::badge(
                self.tr("settings-session-only"),
                Tone::Amber,
            ));

        let settings_nav = components::card()
            .w(px(220.0))
            .h_full()
            .flex_none()
            .p_2()
            .child(settings_nav_item(
                IconName::Settings,
                self.tr("settings-general"),
                true,
            ))
            .child(settings_nav_item(
                IconName::CircleUser,
                self.tr("settings-accounts"),
                false,
            ))
            .child(settings_nav_item(
                IconName::Inbox,
                self.tr("settings-storage"),
                false,
            ))
            .child(settings_nav_item(
                IconName::ArrowDown,
                self.tr("settings-downloads"),
                false,
            ))
            .child(settings_nav_item(
                IconName::ArrowUp,
                self.tr("settings-uploads"),
                false,
            ))
            .child(settings_nav_item(
                IconName::Asterisk,
                self.tr("settings-key-vault"),
                false,
            ))
            .child(settings_nav_item(
                IconName::Search,
                self.tr("settings-indexing"),
                false,
            ))
            .child(settings_nav_item(
                IconName::Bell,
                self.tr("settings-notifications"),
                false,
            ))
            .child(settings_nav_item(
                IconName::Palette,
                self.tr("settings-appearance"),
                false,
            ))
            .child(div().flex_1())
            .child(
                div()
                    .p_3()
                    .text_xs()
                    .text_color(theme::text_muted())
                    .child("TeleArk 0.1.0"),
            );

        let language = components::card()
            .p_5()
            .child(components::section_title(
                self.tr("settings-language-title"),
            ))
            .child(
                div()
                    .mt_1()
                    .text_sm()
                    .text_color(theme::text_secondary())
                    .child(self.tr("settings-language-description")),
            )
            .child(
                div()
                    .mt_5()
                    .grid()
                    .grid_cols(3)
                    .gap_3()
                    .child(self.locale_card(
                        "settings-locale-en",
                        SupportedLocale::EnUs,
                        self.tr("settings-language-english"),
                        "en-US",
                        cx,
                    ))
                    .child(self.locale_card(
                        "settings-locale-zh",
                        SupportedLocale::ZhCn,
                        self.tr("settings-language-chinese"),
                        "zh-CN",
                        cx,
                    ))
                    .child(self.locale_card(
                        "settings-locale-ja",
                        SupportedLocale::JaJp,
                        self.tr("settings-language-japanese"),
                        "ja-JP",
                        cx,
                    )),
            )
            .child(
                div()
                    .mt_4()
                    .p_3()
                    .rounded(theme::RADIUS_SMALL)
                    .bg(theme::blue_pale())
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_xs()
                    .text_color(theme::text_secondary())
                    .child(Icon::new(IconName::Info).text_color(theme::blue()))
                    .child(self.tr("settings-language-runtime-note")),
            );

        let appearance = components::card()
            .mt_4()
            .p_5()
            .child(components::section_title(self.tr("settings-appearance")))
            .child(
                div()
                    .mt_4()
                    .flex()
                    .gap_3()
                    .child(theme_option(
                        self.tr("settings-theme-system"),
                        self.tr("settings-theme-system-description"),
                        true,
                    ))
                    .child(theme_option(
                        self.tr("settings-theme-light"),
                        self.tr("settings-theme-light-description"),
                        false,
                    ))
                    .child(theme_option(
                        self.tr("settings-theme-dark"),
                        self.tr("settings-theme-dark-description"),
                        false,
                    )),
            );

        let behavior = components::card()
            .mt_4()
            .p_5()
            .child(components::section_title(
                self.tr("settings-behavior-title"),
            ))
            .child(
                div()
                    .mt_4()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(preference_row(
                        self.tr("settings-start-at-login"),
                        self.tr("settings-start-at-login-description"),
                        false,
                    ))
                    .child(preference_row(
                        self.tr("settings-restore-window"),
                        self.tr("settings-restore-window-description"),
                        true,
                    ))
                    .child(preference_row(
                        self.tr("settings-show-menu-bar"),
                        self.tr("settings-show-menu-bar-description"),
                        true,
                    )),
            );

        let content = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scrollbar()
            .child(
                div()
                    .max_w(px(880.0))
                    .mx_auto()
                    .pb_5()
                    .child(language)
                    .child(appearance)
                    .child(behavior),
            );

        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .bg(theme::canvas())
            .child(toolbar)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .px_5()
                    .pb_5()
                    .flex()
                    .gap_4()
                    .child(settings_nav)
                    .child(content),
            )
            .into_any_element()
    }

    fn locale_card(
        &self,
        id: &'static str,
        locale: SupportedLocale,
        label: SharedString,
        code: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.locale() == locale;
        div()
            .id(id)
            .p_4()
            .flex()
            .items_center()
            .gap_3()
            .rounded(theme::RADIUS_MEDIUM)
            .border_1()
            .border_color(if selected {
                theme::blue()
            } else {
                theme::border()
            })
            .bg(if selected {
                theme::blue_pale()
            } else {
                theme::surface()
            })
            .cursor_pointer()
            .focusable()
            .tab_index(0)
            .hover(|card| card.bg(theme::blue_pale()))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.set_locale(locale, window, cx);
            }))
            .on_key_down(
                cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        this.set_locale(locale, window, cx);
                    }
                }),
            )
            .child(
                div()
                    .size(px(34.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(if selected {
                        theme::blue()
                    } else {
                        theme::border_subtle()
                    })
                    .text_color(if selected {
                        theme::surface()
                    } else {
                        theme::text_secondary()
                    })
                    .font_weight(FontWeight::BOLD)
                    .child(match locale {
                        SupportedLocale::EnUs => "A",
                        SupportedLocale::ZhCn => "中",
                        SupportedLocale::JaJp => "あ",
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(label))
                    .child(
                        div()
                            .mt_1()
                            .text_xs()
                            .text_color(theme::text_muted())
                            .child(code),
                    ),
            )
            .when(selected, |card| {
                card.child(Icon::new(IconName::CircleCheck).text_color(theme::blue()))
            })
            .into_any_element()
    }
}

fn settings_nav_item(icon: IconName, label: SharedString, selected: bool) -> AnyElement {
    div()
        .h(px(38.0))
        .px_3()
        .flex()
        .items_center()
        .gap_3()
        .rounded(theme::RADIUS_SMALL)
        .bg(if selected {
            theme::blue_soft()
        } else {
            theme::surface()
        })
        .text_color(if selected {
            theme::blue()
        } else {
            theme::text_secondary()
        })
        .text_sm()
        .child(Icon::new(icon).text_color(if selected {
            theme::blue()
        } else {
            theme::text_secondary()
        }))
        .child(label)
        .into_any_element()
}

fn theme_option(title: SharedString, description: SharedString, selected: bool) -> AnyElement {
    div()
        .flex_1()
        .p_4()
        .rounded(theme::RADIUS_MEDIUM)
        .border_1()
        .border_color(if selected {
            theme::blue()
        } else {
            theme::border()
        })
        .bg(if selected {
            theme::blue_pale()
        } else {
            theme::surface()
        })
        .child(
            div()
                .h(px(70.0))
                .rounded(theme::RADIUS_SMALL)
                .border_1()
                .border_color(theme::border())
                .bg(theme::canvas())
                .p_2()
                .child(div().h(px(10.0)).rounded(px(3.0)).bg(theme::border()))
                .child(
                    div()
                        .mt_2()
                        .flex()
                        .gap_2()
                        .child(
                            div()
                                .w(px(34.0))
                                .h(px(38.0))
                                .rounded(px(3.0))
                                .bg(theme::border_subtle()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .h(px(38.0))
                                .rounded(px(3.0))
                                .bg(theme::surface()),
                        ),
                ),
        )
        .child(
            div()
                .mt_3()
                .flex()
                .items_center()
                .child(
                    div()
                        .flex_1()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .child(title),
                )
                .when(selected, |row| {
                    row.child(Icon::new(IconName::CircleCheck).text_color(theme::blue()))
                }),
        )
        .child(
            div()
                .mt_1()
                .text_xs()
                .text_color(theme::text_muted())
                .child(description),
        )
        .into_any_element()
}

fn preference_row(title: SharedString, description: SharedString, enabled: bool) -> AnyElement {
    div()
        .py_2()
        .flex()
        .items_center()
        .gap_4()
        .child(
            div()
                .flex_1()
                .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(title))
                .child(
                    div()
                        .mt_1()
                        .text_xs()
                        .text_color(theme::text_muted())
                        .child(description),
                ),
        )
        .child(
            div()
                .w(px(36.0))
                .h(px(20.0))
                .p(px(2.0))
                .flex()
                .when(enabled, |toggle| toggle.justify_end())
                .rounded_full()
                .bg(if enabled {
                    theme::blue()
                } else {
                    theme::border()
                })
                .child(div().size(px(16.0)).rounded_full().bg(theme::surface())),
        )
        .into_any_element()
}
