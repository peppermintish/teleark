use gpui::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px, rgba,
};
use gpui_component::{
    Disableable as _, Icon, IconName, input::Input, scroll::ScrollableElement as _,
};
use teleark_i18n::SupportedLocale;
use teleark_runtime::TelegramCredentialSource;

use crate::{
    app::{LocalePersistence, TeleArkApp, TelegramApiIdPersistence},
    components::{self, Tone},
    layout::LayoutPolicy,
    theme,
};

const TELEGRAM_API_PANEL_URL: &str = "https://my.telegram.org/apps";

impl TeleArkApp {
    pub(crate) fn render_settings(
        &self,
        _window: &mut Window,
        layout: LayoutPolicy,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let padding = layout.content_padding();
        let toolbar = div()
            .h(px(58.0))
            .px(px(padding))
            .flex()
            .items_center()
            .child(components::section_title(self.tr("settings-title")))
            .child(div().flex_1())
            .child(components::badge(
                self.tr("settings-preview-controls"),
                Tone::Amber,
            ));

        let settings_nav = components::card()
            .w(px(layout.local_navigation_width()))
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
                    .child(concat!("TeleArk ", env!("CARGO_PKG_VERSION"))),
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
                    .grid_cols(if layout.is_compact() { 2 } else { 4 })
                    .gap_3()
                    .child(self.system_locale_card(cx))
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
            )
            .child(div().mt_3().child(components::badge(
                self.tr(match self.locale_persistence {
                    LocalePersistence::Idle => "settings-language-persistence-ready",
                    LocalePersistence::Saving => "settings-language-persistence-saving",
                    LocalePersistence::Saved => "settings-language-persistence-saved",
                    LocalePersistence::Failed => "settings-language-persistence-failed",
                }),
                match self.locale_persistence {
                    LocalePersistence::Failed => Tone::Red,
                    LocalePersistence::Saving => Tone::Amber,
                    LocalePersistence::Idle | LocalePersistence::Saved => Tone::Green,
                },
            )));

        let telegram_credentials = components::card()
            .p_5()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(components::section_title(
                        self.tr("settings-telegram-credentials-title"),
                    ))
                    .child(div().flex_1())
                    .child(components::badge(
                        self.tr(self.telegram_api_id_status_message_id()),
                        self.telegram_api_id_status_tone(),
                    )),
            )
            .child(
                div()
                    .mt_1()
                    .text_sm()
                    .text_color(theme::text_secondary())
                    .child(self.tr("settings-telegram-credentials-description")),
            )
            .child(
                div()
                    .mt_4()
                    .grid()
                    .grid_cols(if layout.is_compact() { 1 } else { 2 })
                    .items_end()
                    .gap_3()
                    .child(
                        div()
                            .w_full()
                            .child(
                                div()
                                    .mb_2()
                                    .text_xs()
                                    .text_color(theme::text_secondary())
                                    .child(self.tr("telegram-api-id-label")),
                            )
                            .child(Input::new(&self.telegram_api_id).h(px(38.0))),
                    )
                    .child(
                        div()
                            .w_full()
                            .child(
                                div()
                                    .mb_2()
                                    .text_xs()
                                    .text_color(theme::text_secondary())
                                    .child(self.tr("telegram-api-hash-label")),
                            )
                            .child(Input::new(&self.telegram_api_hash).h(px(38.0))),
                    ),
            )
            .child(
                div()
                    .mt_3()
                    .flex()
                    .when(layout.is_compact(), |row| row.flex_col())
                    .items_center()
                    .gap_3()
                    .child(
                        components::button(
                            "settings-save-telegram-credentials",
                            self.tr("settings-telegram-credentials-save-action"),
                            Some(IconName::Check),
                            true,
                        )
                        .disabled(matches!(
                            self.telegram_api_id_persistence,
                            TelegramApiIdPersistence::Saving
                        ))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.save_telegram_credentials(window, cx);
                        })),
                    )
                    .child(
                        components::button(
                            "settings-clear-telegram-credentials",
                            self.tr("settings-telegram-credentials-clear-action"),
                            Some(IconName::Delete),
                            false,
                        )
                        .disabled(
                            self.telegram_credential_source != Some(TelegramCredentialSource::User)
                                || matches!(
                                    self.telegram_api_id_persistence,
                                    TelegramApiIdPersistence::Saving
                                ),
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.clear_telegram_credentials(window, cx);
                        })),
                    ),
            )
            .child(
                div()
                    .mt_4()
                    .p_3()
                    .rounded(theme::RADIUS_SMALL)
                    .bg(theme::amber_soft())
                    .flex()
                    .items_start()
                    .gap_2()
                    .text_xs()
                    .text_color(theme::text_secondary())
                    .child(Icon::new(IconName::Info).text_color(theme::amber()))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .whitespace_normal()
                            .child(div().w_full().min_w_0().whitespace_normal().child(self.tr(
                                if self.telegram_credential_source
                                    == Some(TelegramCredentialSource::Distribution)
                                {
                                    "settings-telegram-credentials-distribution-note"
                                } else {
                                    "settings-telegram-credentials-storage-note"
                                },
                            )))
                            .child(
                                div().mt_2().child(
                                    components::button(
                                        "settings-open-telegram-api-panel",
                                        self.tr("settings-telegram-api-panel-action"),
                                        Some(IconName::ExternalLink),
                                        false,
                                    )
                                    .on_click(|_, _, cx| {
                                        cx.open_url(TELEGRAM_API_PANEL_URL);
                                    }),
                                ),
                            ),
                    ),
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
                    .child(telegram_credentials)
                    .child(language.mt_4())
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
                    .px(px(padding))
                    .pb(px(padding))
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
        let selected = !self.follows_system_locale && self.locale() == locale;
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
                    .min_w_0()
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

    fn system_locale_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let selected = self.follows_system_locale;
        div()
            .id("settings-locale-system")
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
            .on_click(cx.listener(|this, _, window, cx| {
                this.use_system_locale(window, cx);
            }))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    this.use_system_locale(window, cx);
                }
            }))
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
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .child("OS"),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child(self.tr("settings-language-system-default")),
                    )
                    .child(
                        div()
                            .mt_1()
                            .text_xs()
                            .text_color(theme::text_muted())
                            .child(self.system_locale.as_str()),
                    ),
            )
            .when(selected, |card| {
                card.child(Icon::new(IconName::CircleCheck).text_color(theme::blue()))
            })
            .into_any_element()
    }

    fn telegram_api_id_status_message_id(&self) -> &'static str {
        match self.telegram_api_id_persistence {
            TelegramApiIdPersistence::Saving => "settings-telegram-api-id-saving",
            TelegramApiIdPersistence::Saved => "settings-telegram-api-id-saved",
            TelegramApiIdPersistence::Removed
                if self.telegram_credential_source
                    == Some(TelegramCredentialSource::Distribution) =>
            {
                "settings-telegram-credentials-removed-using-distribution"
            }
            TelegramApiIdPersistence::Removed => "settings-telegram-credentials-removed",
            TelegramApiIdPersistence::Failed(
                teleark_core::ApplicationErrorKind::InvalidRequest,
            ) => "settings-telegram-api-id-invalid",
            TelegramApiIdPersistence::Failed(_) => "settings-telegram-api-id-failed",
            TelegramApiIdPersistence::Idle
                if self.telegram_credential_source
                    == Some(TelegramCredentialSource::Distribution) =>
            {
                "settings-telegram-api-id-distribution"
            }
            TelegramApiIdPersistence::Idle if self.configured_telegram_api_id.is_some() => {
                "settings-telegram-api-id-configured"
            }
            TelegramApiIdPersistence::Idle => "settings-telegram-api-id-missing",
        }
    }

    fn telegram_api_id_status_tone(&self) -> Tone {
        match self.telegram_api_id_persistence {
            TelegramApiIdPersistence::Saving => Tone::Amber,
            TelegramApiIdPersistence::Failed(_) => Tone::Red,
            TelegramApiIdPersistence::Saved => Tone::Green,
            TelegramApiIdPersistence::Removed
                if self.telegram_credential_source
                    == Some(TelegramCredentialSource::Distribution) =>
            {
                Tone::Green
            }
            TelegramApiIdPersistence::Removed => Tone::Amber,
            TelegramApiIdPersistence::Idle if self.configured_telegram_api_id.is_some() => {
                Tone::Green
            }
            TelegramApiIdPersistence::Idle => Tone::Amber,
        }
    }
}

pub fn render_telegram_api_id_prompt(app: &TeleArkApp, cx: &mut Context<TeleArkApp>) -> AnyElement {
    let failed_message = match app.telegram_api_id_persistence {
        TelegramApiIdPersistence::Failed(teleark_core::ApplicationErrorKind::InvalidRequest) => {
            Some(app.tr("settings-telegram-api-id-invalid"))
        }
        TelegramApiIdPersistence::Failed(_) => Some(app.tr("settings-telegram-api-id-failed")),
        TelegramApiIdPersistence::Idle
        | TelegramApiIdPersistence::Saving
        | TelegramApiIdPersistence::Saved
        | TelegramApiIdPersistence::Removed => None,
    };
    let dialog = components::card()
        .w(px(560.0))
        .max_w_full()
        .p_6()
        .child(
            div()
                .size(px(44.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(theme::RADIUS_MEDIUM)
                .bg(theme::blue_soft())
                .child(Icon::new(IconName::Settings2).text_color(theme::blue())),
        )
        .child(
            div()
                .mt_4()
                .text_xl()
                .font_weight(FontWeight::SEMIBOLD)
                .child(app.tr("telegram-credentials-prompt-title")),
        )
        .child(
            div()
                .mt_2()
                .text_sm()
                .text_color(theme::text_secondary())
                .child(app.tr("telegram-credentials-prompt-description")),
        )
        .child(
            div()
                .mt_4()
                .p_3()
                .rounded(theme::RADIUS_SMALL)
                .bg(theme::blue_pale())
                .flex()
                .items_start()
                .gap_2()
                .text_xs()
                .text_color(theme::text_secondary())
                .child(Icon::new(IconName::Info).text_color(theme::blue()))
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .whitespace_normal()
                        .child(
                            div()
                                .w_full()
                                .min_w_0()
                                .whitespace_normal()
                                .child(app.tr("telegram-credentials-official-panel-note")),
                        )
                        .child(
                            div().mt_2().child(
                                components::button(
                                    "telegram-prompt-open-api-panel",
                                    app.tr("settings-telegram-api-panel-action"),
                                    Some(IconName::ExternalLink),
                                    false,
                                )
                                .on_click(|_, _, cx| {
                                    cx.open_url(TELEGRAM_API_PANEL_URL);
                                }),
                            ),
                        ),
                ),
        )
        .child(
            div()
                .mt_5()
                .grid()
                .grid_cols(2)
                .gap_3()
                .child(
                    div()
                        .child(
                            div()
                                .mb_2()
                                .text_xs()
                                .text_color(theme::text_secondary())
                                .child(app.tr("telegram-api-id-label")),
                        )
                        .child(Input::new(&app.telegram_api_id).h(px(38.0))),
                )
                .child(
                    div()
                        .child(
                            div()
                                .mb_2()
                                .text_xs()
                                .text_color(theme::text_secondary())
                                .child(app.tr("telegram-api-hash-label")),
                        )
                        .child(Input::new(&app.telegram_api_hash).h(px(38.0))),
                ),
        )
        .when_some(failed_message, |dialog, message| {
            dialog.child(
                div()
                    .mt_3()
                    .p_3()
                    .rounded(theme::RADIUS_SMALL)
                    .bg(theme::red_soft())
                    .text_sm()
                    .text_color(theme::red())
                    .child(message),
            )
        })
        .child(
            div()
                .mt_5()
                .flex()
                .justify_end()
                .gap_3()
                .child(
                    components::button(
                        "telegram-api-id-skip",
                        app.tr("telegram-api-id-skip-action"),
                        None,
                        false,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.skip_telegram_api_id_prompt(cx);
                    })),
                )
                .child(
                    components::button(
                        "telegram-api-id-save",
                        app.tr("telegram-credentials-save-action"),
                        Some(IconName::Check),
                        true,
                    )
                    .disabled(matches!(
                        app.telegram_api_id_persistence,
                        TelegramApiIdPersistence::Saving
                    ))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.save_telegram_credentials(window, cx);
                    })),
                ),
        );

    div()
        .absolute()
        .inset_0()
        .bg(rgba(0x18203366))
        .p_4()
        .flex()
        .items_center()
        .justify_center()
        .child(dialog)
        .into_any_element()
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
        .child(div().min_w_0().truncate().child(label))
        .into_any_element()
}

fn theme_option(title: SharedString, description: SharedString, selected: bool) -> AnyElement {
    div()
        .flex_1()
        .min_w_0()
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
                .min_w_0()
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
                .flex_none()
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

#[cfg(test)]
mod tests {
    use super::TELEGRAM_API_PANEL_URL;

    #[test]
    fn telegram_api_panel_button_targets_the_official_https_page() {
        assert_eq!(TELEGRAM_API_PANEL_URL, "https://my.telegram.org/apps");
    }
}
