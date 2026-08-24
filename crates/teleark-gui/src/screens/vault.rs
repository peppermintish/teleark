use gpui::{
    AnyElement, Context, FontWeight, IntoElement, ParentElement as _, SharedString, Styled as _,
    Window, div, px,
};
use gpui_component::{Icon, IconName, scroll::ScrollableElement as _};

use crate::{
    app::TeleArkApp,
    components::{self, Tone},
    theme,
};

impl TeleArkApp {
    pub(crate) fn render_vault(&self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let toolbar = div()
            .h(px(58.0))
            .px_5()
            .flex()
            .items_center()
            .child(components::section_title(self.tr("vault-title")))
            .child(div().flex_1())
            .child(components::badge(
                if self.vault_locked {
                    self.tr("vault-status-locked")
                } else {
                    self.tr("vault-status-unlocked")
                },
                if self.vault_locked {
                    Tone::Amber
                } else {
                    Tone::Green
                },
            ));

        let settings_nav = components::card()
            .w(px(205.0))
            .h_full()
            .flex_none()
            .p_2()
            .children([
                settings_item(IconName::Settings, self.tr("settings-general"), false),
                settings_item(IconName::CircleUser, self.tr("settings-accounts"), false),
                settings_item(IconName::Inbox, self.tr("settings-storage"), false),
                settings_item(IconName::ArrowDown, self.tr("settings-downloads"), false),
                settings_item(IconName::ArrowUp, self.tr("settings-uploads"), false),
                settings_item(IconName::Asterisk, self.tr("settings-key-vault"), true),
                settings_item(IconName::Search, self.tr("settings-indexing"), false),
                settings_item(IconName::Bell, self.tr("settings-notifications"), false),
                settings_item(IconName::Palette, self.tr("settings-appearance"), false),
            ]);

        let master_key = components::card()
            .p_5()
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap_4()
                    .child(
                        div()
                            .size(px(54.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(theme::RADIUS_MEDIUM)
                            .bg(if self.vault_locked {
                                theme::amber_soft()
                            } else {
                                theme::blue_soft()
                            })
                            .child(Icon::new(IconName::Asterisk).text_color(
                                if self.vault_locked {
                                    theme::amber()
                                } else {
                                    theme::blue()
                                },
                            )),
                    )
                    .child(
                        div()
                            .flex_1()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .child(components::section_title(self.tr("vault-master-key")))
                                    .child(components::badge(
                                        if self.vault_locked {
                                            self.tr("vault-status-locked")
                                        } else {
                                            self.tr("vault-status-unlocked")
                                        },
                                        if self.vault_locked {
                                            Tone::Amber
                                        } else {
                                            Tone::Green
                                        },
                                    )),
                            )
                            .child(
                                div()
                                    .mt_2()
                                    .grid()
                                    .grid_cols(3)
                                    .gap_5()
                                    .child(key_metric(self.tr("vault-created"), "2026-08-20 14:32"))
                                    .child(key_metric(self.tr("vault-kdf"), "Argon2id"))
                                    .child(key_metric(self.tr("vault-cipher"), "AES-256-GCM")),
                            ),
                    ),
            )
            .child(
                div()
                    .mt_5()
                    .pt_4()
                    .border_t_1()
                    .border_color(theme::border())
                    .flex()
                    .gap_2()
                    .child(components::button(
                        "vault-change-password",
                        self.tr("vault-change-password"),
                        Some(IconName::Asterisk),
                        false,
                    ))
                    .child(
                        components::button(
                            "vault-toggle-lock",
                            if self.vault_locked {
                                self.tr("vault-unlock")
                            } else {
                                self.tr("vault-lock")
                            },
                            Some(if self.vault_locked {
                                IconName::Eye
                            } else {
                                IconName::EyeOff
                            }),
                            false,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.vault_locked = !this.vault_locked;
                            if this.vault_locked {
                                this.recovery_visible = false;
                            }
                            cx.notify();
                        })),
                    ),
            );

        let recovery_key = components::card()
            .mt_4()
            .p_5()
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .child(components::section_title(self.tr("vault-recovery-title")))
                            .child(
                                div()
                                    .mt_2()
                                    .text_sm()
                                    .text_color(theme::text_secondary())
                                    .child(if self.recovery_visible {
                                        "DEMO-RK-NOT-A-REAL-KEY"
                                    } else {
                                        "DEMO-RK-••••-••••-••••"
                                    }),
                            ),
                    )
                    .child(components::badge(
                        self.tr("vault-recovery-backed-up"),
                        Tone::Green,
                    )),
            )
            .child(
                div()
                    .mt_5()
                    .pt_4()
                    .border_t_1()
                    .border_color(theme::border())
                    .flex()
                    .gap_2()
                    .child(
                        components::button(
                            "vault-reveal-recovery",
                            if self.vault_locked {
                                self.tr("vault-unlock-to-view")
                            } else if self.recovery_visible {
                                self.tr("vault-hide-recovery")
                            } else {
                                self.tr("vault-show-recovery")
                            },
                            Some(if self.recovery_visible {
                                IconName::EyeOff
                            } else {
                                IconName::Eye
                            }),
                            false,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            if !this.vault_locked {
                                this.recovery_visible = !this.recovery_visible;
                                cx.notify();
                            }
                        })),
                    )
                    .child(components::button(
                        "vault-export-recovery",
                        self.tr("vault-export-recovery"),
                        Some(IconName::ExternalLink),
                        false,
                    )),
            );

        let profile = components::card()
            .mt_4()
            .p_5()
            .child(components::section_title(self.tr("vault-profile-title")))
            .child(
                div()
                    .mt_4()
                    .grid()
                    .grid_cols(2)
                    .gap_3()
                    .child(option_row(
                        self.tr("vault-option-hidden-filenames"),
                        self.tr("vault-option-hidden-filenames-description"),
                        true,
                    ))
                    .child(option_row(
                        self.tr("vault-option-encrypted-metadata"),
                        self.tr("vault-option-encrypted-metadata-description"),
                        true,
                    ))
                    .child(option_row(
                        self.tr("vault-option-compatible-parts"),
                        self.tr("vault-option-compatible-parts-description"),
                        true,
                    ))
                    .child(option_row(
                        self.tr("vault-option-keychain"),
                        self.tr("vault-option-keychain-description"),
                        false,
                    )),
            );

        let warning = div()
            .mt_4()
            .p_4()
            .flex()
            .items_start()
            .gap_3()
            .rounded(theme::RADIUS_MEDIUM)
            .bg(theme::red_soft())
            .text_color(theme::red())
            .text_sm()
            .child(Icon::new(IconName::TriangleAlert).text_color(theme::red()))
            .child(self.tr("vault-key-loss-warning"));

        let content = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scrollbar()
            .child(
                div()
                    .max_w(px(920.0))
                    .mx_auto()
                    .pb_5()
                    .child(master_key)
                    .child(recovery_key)
                    .child(profile)
                    .child(warning),
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
}

fn settings_item(icon: IconName, label: SharedString, selected: bool) -> AnyElement {
    div()
        .h(px(36.0))
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

fn key_metric(label: SharedString, value: &'static str) -> AnyElement {
    div()
        .child(div().text_xs().text_color(theme::text_muted()).child(label))
        .child(
            div()
                .mt_1()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .child(value),
        )
        .into_any_element()
}

fn option_row(title: SharedString, description: SharedString, enabled: bool) -> AnyElement {
    div()
        .p_4()
        .flex()
        .items_center()
        .gap_3()
        .rounded(theme::RADIUS_MEDIUM)
        .border_1()
        .border_color(theme::border())
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
                .w(px(34.0))
                .h(px(19.0))
                .p(px(2.0))
                .flex()
                .justify_end()
                .rounded_full()
                .bg(if enabled {
                    theme::blue()
                } else {
                    theme::border()
                })
                .child(div().size(px(15.0)).rounded_full().bg(theme::surface())),
        )
        .into_any_element()
}
