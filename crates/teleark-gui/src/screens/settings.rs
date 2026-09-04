use gpui::{
    AnyElement, Context, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px, rgba,
};
use gpui_component::{
    Disableable as _, Icon, IconName,
    input::{Input, InputState},
    scroll::ScrollableElement as _,
};
use teleark_i18n::{
    SupportedLocale,
    format::{format_bytes, format_integer},
};
use teleark_runtime::{
    AppearancePreference, SoftLimitPolicy, TelegramCredentialSource, default_database_path,
    default_managed_directories, diagnostics_status, encrypted_part_plaintext_limit,
};

use crate::{
    app::{
        LocalePersistence, PreferencePersistence, SettingsSection, TeleArkApp,
        TelegramApiIdPersistence, VaultActivity,
    },
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
                self.tr(match self.preference_persistence {
                    PreferencePersistence::Idle => "settings-preferences-ready",
                    PreferencePersistence::Saving => "settings-preferences-saving",
                    PreferencePersistence::Saved => "settings-preferences-saved",
                    PreferencePersistence::Failed => "settings-preferences-failed",
                }),
                match self.preference_persistence {
                    PreferencePersistence::Saving => Tone::Amber,
                    PreferencePersistence::Failed => Tone::Red,
                    PreferencePersistence::Idle | PreferencePersistence::Saved => Tone::Green,
                },
            ));

        let settings_nav = components::card()
            .w(px(layout.local_navigation_width()))
            .h_full()
            .flex_none()
            .p_2()
            .child(self.settings_nav_item(
                IconName::Settings,
                self.tr("settings-general"),
                SettingsSection::General,
                cx,
            ))
            .child(self.settings_nav_item(
                IconName::CircleUser,
                self.tr("settings-accounts"),
                SettingsSection::Accounts,
                cx,
            ))
            .child(self.settings_nav_item(
                IconName::Inbox,
                self.tr("settings-storage"),
                SettingsSection::Storage,
                cx,
            ))
            .child(self.settings_nav_item(
                IconName::ArrowDown,
                self.tr("settings-downloads"),
                SettingsSection::Downloads,
                cx,
            ))
            .child(self.settings_nav_item(
                IconName::ArrowUp,
                self.tr("settings-uploads"),
                SettingsSection::Uploads,
                cx,
            ))
            .child(self.settings_nav_item(
                IconName::Asterisk,
                self.tr("settings-key-vault"),
                SettingsSection::KeyVault,
                cx,
            ))
            .child(self.settings_nav_item(
                IconName::Search,
                self.tr("settings-indexing"),
                SettingsSection::Indexing,
                cx,
            ))
            .child(self.settings_nav_item(
                IconName::Bell,
                self.tr("settings-notifications"),
                SettingsSection::Notifications,
                cx,
            ))
            .child(self.settings_nav_item(
                IconName::Palette,
                self.tr("settings-appearance"),
                SettingsSection::Appearance,
                cx,
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

        let active_content = match self.settings_section {
            SettingsSection::General => language.into_any_element(),
            SettingsSection::Accounts => telegram_credentials.into_any_element(),
            SettingsSection::Storage => self.render_storage_settings(cx),
            SettingsSection::Downloads => self.render_download_settings(cx),
            SettingsSection::Uploads => self.render_upload_settings(cx),
            SettingsSection::KeyVault => self.render_key_vault_settings(cx),
            SettingsSection::Indexing => self.render_indexing_settings(cx),
            SettingsSection::Notifications => self.render_notification_settings(cx),
            SettingsSection::Appearance => self.render_appearance_settings(layout, cx),
        };

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
                    .child(active_content),
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

    fn settings_nav_item(
        &self,
        icon: IconName,
        label: SharedString,
        section: SettingsSection,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.settings_section == section;
        div()
            .id(("settings-section", section as u64))
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
            .cursor_pointer()
            .focusable()
            .tab_index(0)
            .hover(|item| item.bg(theme::blue_pale()))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.set_settings_section(section, cx);
            }))
            .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    this.set_settings_section(section, cx);
                }
            }))
            .child(Icon::new(icon).text_color(if selected {
                theme::blue()
            } else {
                theme::text_secondary()
            }))
            .child(div().min_w_0().truncate().child(label))
            .into_any_element()
    }

    fn render_storage_settings(&self, _cx: &mut Context<Self>) -> AnyElement {
        let database_path = default_database_path();
        let display_path = database_path
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.tr("settings-path-unavailable").to_string());
        let library_card = settings_card(
            self.tr("settings-storage-library-title"),
            self.tr("settings-storage-library-description"),
        )
        .child(path_panel(display_path))
        .when_some(database_path, |card, path| {
            card.child(
                div().mt_4().child(
                    components::button(
                        "settings-reveal-library-database",
                        self.tr("settings-storage-reveal-action"),
                        Some(IconName::FolderOpen),
                        false,
                    )
                    .on_click(move |_, _, cx| cx.reveal_path(&path)),
                ),
            )
        })
        .child(
            div()
                .mt_4()
                .p_3()
                .rounded(theme::RADIUS_SMALL)
                .bg(theme::blue_pale())
                .text_xs()
                .text_color(theme::text_secondary())
                .child(self.tr("settings-storage-sqlite-note")),
        )
        .into_any_element();
        let diagnostics = diagnostics_status();
        let diagnostics_path = diagnostics
            .as_ref()
            .map(|status| status.log_directory.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.tr("settings-diagnostics-unavailable").to_string());
        let dropped_events = diagnostics
            .as_ref()
            .map(|status| status.dropped_event_count)
            .unwrap_or_default();
        let diagnostics_directory = diagnostics.map(|status| status.log_directory);
        let diagnostics_card = settings_card(
            self.tr("settings-diagnostics-title"),
            self.tr("settings-diagnostics-description"),
        )
        .child(path_panel(diagnostics_path))
        .child(
            div()
                .mt_3()
                .text_xs()
                .text_color(if dropped_events == 0 {
                    theme::green()
                } else {
                    theme::amber()
                })
                .child(self.tr_with(
                    "settings-diagnostics-dropped-events",
                    teleark_i18n::MessageArgs::new().with(
                        "count",
                        format_integer(self.locale(), dropped_events as u64),
                    ),
                )),
        )
        .when_some(diagnostics_directory, |card, directory| {
            card.child(
                div().mt_3().child(
                    components::button(
                        "settings-open-diagnostics",
                        self.tr("settings-diagnostics-open-action"),
                        Some(IconName::FolderOpen),
                        false,
                    )
                    .on_click(move |_, _, cx| cx.open_with_system(&directory)),
                ),
            )
        })
        .child(
            div()
                .mt_4()
                .p_3()
                .rounded(theme::RADIUS_SMALL)
                .bg(theme::blue_pale())
                .text_xs()
                .text_color(theme::text_secondary())
                .child(self.tr("settings-diagnostics-privacy-note")),
        )
        .into_any_element();

        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(library_card)
            .child(diagnostics_card)
            .into_any_element()
    }

    fn render_download_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let directories = default_managed_directories(&self.preferences);
        let root = directories
            .as_ref()
            .map(|directories| directories.root.clone());
        let active_diagnostics = diagnostics_status();
        let logs_relocation_pending = directories
            .as_ref()
            .zip(active_diagnostics.as_ref())
            .is_some_and(|(directories, status)| directories.logs != status.log_directory);
        let mut card = settings_card(
            self.tr("settings-download-title"),
            self.tr("settings-download-description"),
        );
        if let Some(directories) = directories {
            card = card
                .child(labeled_path_panel(
                    self.tr("settings-managed-root-label"),
                    directories.root.to_string_lossy().into_owned(),
                ))
                .child(labeled_path_panel(
                    self.tr("settings-managed-downloads-label"),
                    directories.downloads.to_string_lossy().into_owned(),
                ))
                .child(labeled_path_panel(
                    self.tr("settings-managed-cache-label"),
                    directories.cache.to_string_lossy().into_owned(),
                ))
                .child(labeled_path_panel(
                    self.tr("settings-managed-logs-label"),
                    directories.logs.to_string_lossy().into_owned(),
                ));
        } else {
            card = card.child(path_panel(self.tr("settings-path-unavailable")));
        }
        card = card.when(logs_relocation_pending, |card| {
            card.child(
                div()
                    .mt_3()
                    .p_3()
                    .rounded(theme::RADIUS_SMALL)
                    .bg(theme::amber_soft())
                    .text_xs()
                    .text_color(theme::text_secondary())
                    .child(self.tr("settings-managed-logs-restart-note")),
            )
        });
        card.child(
            div()
                .mt_3()
                .flex()
                .flex_wrap()
                .gap_2()
                .child(
                    components::button(
                        "settings-choose-download-directory",
                        self.tr("settings-managed-root-action"),
                        Some(IconName::FolderOpen),
                        true,
                    )
                    .disabled(self.preference_persistence == PreferencePersistence::Saving)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.choose_managed_files_root(cx);
                    })),
                )
                .child(
                    components::button(
                        "settings-clear-download-directory",
                        self.tr("settings-managed-root-default-action"),
                        Some(IconName::Undo),
                        false,
                    )
                    .disabled(
                        self.preference_persistence == PreferencePersistence::Saving
                            || self.preferences.managed_files_root.is_none(),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        if this.preference_persistence != PreferencePersistence::Saving
                            && this.preferences.managed_files_root.is_some()
                        {
                            this.preferences.managed_files_root = None;
                            this.persist_preferences(cx);
                        }
                    })),
                )
                .when_some(root, |actions, root| {
                    actions.child(
                        components::button(
                            "settings-open-managed-root",
                            self.tr("settings-managed-root-open-action"),
                            Some(IconName::FolderOpen),
                            false,
                        )
                        .on_click(move |_, _, cx| cx.open_with_system(&root)),
                    )
                }),
        )
        .child(div().mt_5().flex().flex_col().gap_2().child(preference_row(
            "settings-reveal-completed-downloads",
            self.tr("settings-download-reveal-completed"),
            self.tr("settings-download-reveal-completed-description"),
            self.preferences.reveal_completed_downloads,
            cx.listener(|this, _, _, cx| {
                if this.preference_persistence != PreferencePersistence::Saving {
                    this.preferences.reveal_completed_downloads =
                        !this.preferences.reveal_completed_downloads;
                    this.persist_preferences(cx);
                }
            }),
        )))
        .into_any_element()
    }

    fn render_upload_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let part_size = format_bytes(self.locale(), encrypted_part_plaintext_limit());
        settings_card(
            self.tr("settings-upload-title"),
            self.tr("settings-upload-description"),
        )
        .child(
            div()
                .mt_4()
                .flex()
                .items_center()
                .gap_3()
                .p_4()
                .rounded(theme::RADIUS_MEDIUM)
                .bg(theme::blue_pale())
                .child(Icon::new(IconName::Asterisk).text_color(theme::blue()))
                .child(
                    div()
                        .flex_1()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .child(self.tr("settings-upload-vault-managed-title")),
                        )
                        .child(
                            div()
                                .mt_1()
                                .text_xs()
                                .text_color(theme::text_secondary())
                                .child(self.tr_with(
                                    "settings-upload-vault-managed-description",
                                    teleark_i18n::MessageArgs::new().with("size", part_size),
                                )),
                        ),
                ),
        )
        .child(
            div()
                .mt_5()
                .pt_4()
                .border_t_1()
                .border_color(theme::border())
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(self.tr("settings-transfer-soft-limit-title")),
                )
                .child(
                    div()
                        .mt_1()
                        .text_xs()
                        .text_color(theme::text_secondary())
                        .child(self.tr("settings-transfer-soft-limit-description")),
                )
                .child(
                    div()
                        .mt_3()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .child(
                            components::button(
                                "settings-soft-limit-respect",
                                self.tr("settings-transfer-soft-limit-respect"),
                                None,
                                self.preferences.transfer_soft_limit_policy
                                    == SoftLimitPolicy::Respect,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.preferences.transfer_soft_limit_policy =
                                    SoftLimitPolicy::Respect;
                                this.persist_preferences(cx);
                            })),
                        )
                        .child(
                            components::button(
                                "settings-soft-limit-adaptive",
                                self.tr("settings-transfer-soft-limit-adaptive"),
                                None,
                                self.preferences.transfer_soft_limit_policy
                                    == SoftLimitPolicy::AdaptiveOverride,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.preferences.transfer_soft_limit_policy =
                                    SoftLimitPolicy::AdaptiveOverride;
                                this.persist_preferences(cx);
                            })),
                        )
                        .child(
                            components::button(
                                "settings-soft-limit-ignore",
                                self.tr("settings-transfer-soft-limit-ignore"),
                                None,
                                self.preferences.transfer_soft_limit_policy
                                    == SoftLimitPolicy::Ignore,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.preferences.transfer_soft_limit_policy =
                                    SoftLimitPolicy::Ignore;
                                this.persist_preferences(cx);
                            })),
                        ),
                )
                .child(
                    div()
                        .mt_3()
                        .p_3()
                        .rounded(theme::RADIUS_SMALL)
                        .bg(theme::amber_soft())
                        .text_xs()
                        .text_color(theme::text_secondary())
                        .child(self.tr("settings-transfer-soft-limit-note")),
                ),
        )
        .into_any_element()
    }

    fn render_key_vault_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let working = self.vault_activity == VaultActivity::Working;
        let status_id = if !self.vault_status.configured {
            "vault-status-not-configured"
        } else if self.vault_status.locked {
            "vault-status-locked"
        } else {
            "vault-status-unlocked"
        };
        let status_tone = if !self.vault_status.configured || self.vault_status.locked {
            Tone::Amber
        } else {
            Tone::Green
        };
        let card = settings_card(
            self.tr("settings-vault-title"),
            self.tr("settings-vault-description"),
        )
        .child(
            div()
                .mt_4()
                .flex()
                .items_center()
                .gap_3()
                .child(components::badge(self.tr(status_id), status_tone))
                .when_some(self.vault_status.password_generation, |row, generation| {
                    row.child(components::badge(
                        self.tr_with(
                            "vault-password-generation",
                            teleark_i18n::MessageArgs::new().with(
                                "generation",
                                format_integer(self.locale(), u64::from(generation)),
                            ),
                        ),
                        Tone::Neutral,
                    ))
                })
                .when(working, |row| {
                    row.child(components::badge(
                        self.tr("vault-operation-working"),
                        Tone::Blue,
                    ))
                }),
        )
        .when_some(vault_activity_message(self), |card, (message, tone)| {
            card.child(
                div()
                    .mt_3()
                    .p_3()
                    .rounded(theme::RADIUS_SMALL)
                    .bg(match tone {
                        Tone::Red => theme::red_soft(),
                        Tone::Green => theme::green_soft(),
                        _ => theme::blue_pale(),
                    })
                    .text_sm()
                    .text_color(theme::text_secondary())
                    .child(message),
            )
        })
        .when(!self.vault_status.configured, |card| {
            card.child(vault_input(
                self.tr("vault-create-password-label"),
                &self.vault_password,
            ))
            .child(vault_input(
                self.tr("vault-confirm-password-label"),
                &self.vault_new_password,
            ))
            .child(
                components::button(
                    "settings-create-vault",
                    self.tr("vault-create-action"),
                    Some(IconName::Asterisk),
                    true,
                )
                .mt_3()
                .disabled(working)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.initialize_vault(window, cx);
                })),
            )
            .child(
                div()
                    .mt_5()
                    .pt_4()
                    .border_t_1()
                    .border_color(theme::border())
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.tr("vault-restore-title")),
            )
            .child(
                div()
                    .mt_1()
                    .text_xs()
                    .text_color(theme::text_secondary())
                    .child(self.tr("vault-restore-description")),
            )
            .child(vault_input(
                self.tr("vault-recovery-bundle-label"),
                &self.vault_recovery_key,
            ))
            .child(
                components::button(
                    "settings-restore-vault",
                    self.tr("vault-restore-action"),
                    Some(IconName::Redo2),
                    false,
                )
                .mt_3()
                .disabled(working)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.restore_vault_with_recovery(window, cx);
                })),
            )
        })
        .when(
            self.vault_status.configured && self.vault_status.locked,
            |card| {
                card.child(vault_input(
                    self.tr("vault-password-label"),
                    &self.vault_password,
                ))
                .child(
                    components::button(
                        "settings-unlock-vault-password",
                        self.tr("vault-unlock-password-action"),
                        Some(IconName::Eye),
                        true,
                    )
                    .mt_3()
                    .disabled(working)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.unlock_vault_with_password(window, cx);
                    })),
                )
                .child(vault_input(
                    self.tr("vault-recovery-key-label"),
                    &self.vault_recovery_key,
                ))
                .child(
                    components::button(
                        "settings-unlock-vault-recovery",
                        self.tr("vault-unlock-recovery-action"),
                        Some(IconName::Asterisk),
                        false,
                    )
                    .mt_3()
                    .disabled(working)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.unlock_vault_with_recovery(window, cx);
                    })),
                )
            },
        )
        .when(
            self.vault_status.configured && !self.vault_status.locked,
            |card| {
                card.child(vault_input(
                    self.tr("vault-new-password-label"),
                    &self.vault_password,
                ))
                .child(vault_input(
                    self.tr("vault-confirm-password-label"),
                    &self.vault_new_password,
                ))
                .child(
                    div()
                        .mt_3()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .child(
                            components::button(
                                "settings-change-vault-password",
                                self.tr("vault-change-password"),
                                Some(IconName::Asterisk),
                                false,
                            )
                            .disabled(working)
                            .on_click(cx.listener(
                                |this, _, window, cx| {
                                    this.change_vault_password(window, cx);
                                },
                            )),
                        )
                        .child(
                            components::button(
                                "settings-rotate-vault-recovery",
                                self.tr("vault-rotate-recovery-action"),
                                Some(IconName::Redo2),
                                false,
                            )
                            .disabled(working)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.rotate_vault_recovery_key(cx);
                            })),
                        )
                        .child(
                            components::button(
                                "settings-lock-vault-now",
                                self.tr("settings-vault-lock-now-action"),
                                Some(IconName::EyeOff),
                                true,
                            )
                            .disabled(working)
                            .on_click(cx.listener(|this, _, _, cx| this.lock_vault(cx))),
                        ),
                )
            },
        )
        .when_some(
            self.vault_recovery_secret
                .as_ref()
                .filter(|_| self.recovery_visible),
            |card, secret| {
                card.child(
                    div()
                        .mt_4()
                        .p_4()
                        .rounded(theme::RADIUS_MEDIUM)
                        .border_1()
                        .border_color(theme::amber())
                        .bg(theme::amber_soft())
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(self.tr("vault-recovery-save-now-title")),
                        )
                        .child(
                            div()
                                .mt_2()
                                .text_xs()
                                .text_color(theme::text_secondary())
                                .child(self.tr("vault-recovery-save-now-description")),
                        )
                        .child(
                            div()
                                .mt_3()
                                .p_3()
                                .rounded(theme::RADIUS_SMALL)
                                .bg(theme::surface())
                                .text_xs()
                                .child(secret.clone()),
                        )
                        .child(
                            div()
                                .mt_3()
                                .flex()
                                .gap_2()
                                .child(
                                    components::button(
                                        "settings-export-vault-recovery",
                                        self.tr("vault-export-recovery"),
                                        Some(IconName::ExternalLink),
                                        true,
                                    )
                                    .on_click(cx.listener(
                                        |this, _, _, cx| {
                                            this.export_vault_recovery_key(cx);
                                        },
                                    )),
                                )
                                .child(
                                    components::button(
                                        "settings-hide-vault-recovery",
                                        self.tr("vault-hide-recovery"),
                                        Some(IconName::EyeOff),
                                        false,
                                    )
                                    .on_click(cx.listener(
                                        |this, _, _, cx| {
                                            this.hide_vault_recovery_key(cx);
                                        },
                                    )),
                                ),
                        ),
                )
            },
        )
        .child(
            div()
                .mt_4()
                .p_4()
                .rounded(theme::RADIUS_MEDIUM)
                .border_1()
                .border_color(theme::border())
                .bg(theme::border_subtle())
                .opacity(0.55)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(Icon::new(IconName::Asterisk).text_color(theme::text_muted()))
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(self.tr("vault-os-credential-title")),
                        ),
                )
                .child(
                    div()
                        .mt_1()
                        .text_xs()
                        .text_color(theme::text_muted())
                        .child(self.tr("vault-os-credential-development-note")),
                ),
        )
        .child(div().mt_4().child(preference_row(
            "settings-lock-vault-when-hidden",
            self.tr("settings-vault-lock-when-hidden"),
            self.tr("settings-vault-lock-when-hidden-description"),
            self.preferences.lock_vault_when_hidden,
            cx.listener(|this, _, _, cx| {
                if this.preference_persistence != PreferencePersistence::Saving {
                    this.preferences.lock_vault_when_hidden =
                        !this.preferences.lock_vault_when_hidden;
                    this.persist_preferences(cx);
                }
            }),
        )))
        .child(
            div()
                .mt_4()
                .p_3()
                .rounded(theme::RADIUS_SMALL)
                .bg(theme::red_soft())
                .text_sm()
                .text_color(theme::red())
                .child(self.tr("vault-key-loss-warning")),
        );
        card.into_any_element()
    }

    fn render_indexing_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let options = [200_u16, 500, 1_000].into_iter().map(|batch_size| {
            selection_option(
                ("settings-index-batch", usize::from(batch_size)),
                self.tr_with(
                    "settings-index-batch-option",
                    teleark_i18n::MessageArgs::new().with(
                        "count",
                        format_integer(self.locale(), u64::from(batch_size)),
                    ),
                ),
                self.tr("settings-index-batch-option-description"),
                self.preferences.index_batch_size == batch_size,
                cx.listener(move |this, _, _, cx| {
                    if this.preference_persistence != PreferencePersistence::Saving {
                        this.preferences.index_batch_size = batch_size;
                        this.persist_preferences(cx);
                    }
                }),
            )
        });
        settings_card(
            self.tr("settings-index-title"),
            self.tr("settings-index-description"),
        )
        .child(div().mt_4().grid().grid_cols(3).gap_3().children(options))
        .into_any_element()
    }

    fn render_notification_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        settings_card(
            self.tr("settings-notification-title"),
            self.tr("settings-notification-description"),
        )
        .child(
            div()
                .mt_4()
                .flex()
                .flex_col()
                .gap_2()
                .child(preference_row(
                    "settings-notify-download-complete",
                    self.tr("settings-notify-download-completed"),
                    self.tr("settings-notify-download-completed-description"),
                    self.preferences.notify_download_completed,
                    cx.listener(|this, _, _, cx| {
                        if this.preference_persistence != PreferencePersistence::Saving {
                            this.preferences.notify_download_completed =
                                !this.preferences.notify_download_completed;
                            this.persist_preferences(cx);
                        }
                    }),
                ))
                .child(preference_row(
                    "settings-notify-download-failed",
                    self.tr("settings-notify-download-failed"),
                    self.tr("settings-notify-download-failed-description"),
                    self.preferences.notify_download_failed,
                    cx.listener(|this, _, _, cx| {
                        if this.preference_persistence != PreferencePersistence::Saving {
                            this.preferences.notify_download_failed =
                                !this.preferences.notify_download_failed;
                            this.persist_preferences(cx);
                        }
                    }),
                )),
        )
        .into_any_element()
    }

    fn render_appearance_settings(
        &self,
        layout: LayoutPolicy,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        settings_card(
            self.tr("settings-appearance-title"),
            self.tr("settings-appearance-description"),
        )
        .child(
            div()
                .mt_4()
                .grid()
                .grid_cols(if layout.is_compact() { 1 } else { 3 })
                .gap_3()
                .child(theme_option(
                    "settings-theme-system",
                    self.tr("settings-theme-system"),
                    self.tr("settings-theme-system-description"),
                    self.preferences.appearance == AppearancePreference::System,
                    cx.listener(|this, _, window, cx| {
                        this.set_appearance_preference(AppearancePreference::System, window, cx);
                    }),
                ))
                .child(theme_option(
                    "settings-theme-light",
                    self.tr("settings-theme-light"),
                    self.tr("settings-theme-light-description"),
                    self.preferences.appearance == AppearancePreference::Light,
                    cx.listener(|this, _, window, cx| {
                        this.set_appearance_preference(AppearancePreference::Light, window, cx);
                    }),
                ))
                .child(theme_option(
                    "settings-theme-dark",
                    self.tr("settings-theme-dark"),
                    self.tr("settings-theme-dark-description"),
                    self.preferences.appearance == AppearancePreference::Dark,
                    cx.listener(|this, _, window, cx| {
                        this.set_appearance_preference(AppearancePreference::Dark, window, cx);
                    }),
                )),
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

fn settings_card(title: SharedString, description: SharedString) -> gpui::Div {
    components::card()
        .p_5()
        .child(components::section_title(title))
        .child(
            div()
                .mt_1()
                .text_sm()
                .text_color(theme::text_secondary())
                .child(description),
        )
}

fn path_panel(path: impl Into<SharedString>) -> AnyElement {
    div()
        .mt_4()
        .p_3()
        .rounded(theme::RADIUS_SMALL)
        .border_1()
        .border_color(theme::border())
        .bg(theme::sidebar())
        .font_family("monospace")
        .text_xs()
        .text_color(theme::text_secondary())
        .child(path.into())
        .into_any_element()
}

fn labeled_path_panel(label: SharedString, path: impl Into<SharedString>) -> AnyElement {
    div()
        .mt_4()
        .child(
            div()
                .mb_1()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::text_secondary())
                .child(label),
        )
        .child(path_panel(path))
        .into_any_element()
}

fn theme_option(
    id: &'static str,
    title: SharedString,
    description: SharedString,
    selected: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> AnyElement {
    div()
        .id(id)
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
        .cursor_pointer()
        .focusable()
        .tab_index(0)
        .hover(|option| option.bg(theme::blue_pale()))
        .on_click(on_click)
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

fn selection_option(
    id: impl Into<gpui::ElementId>,
    title: impl Into<SharedString>,
    description: SharedString,
    selected: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> AnyElement {
    div()
        .id(id)
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
        .cursor_pointer()
        .focusable()
        .tab_index(0)
        .hover(|option| option.bg(theme::blue_pale()))
        .on_click(on_click)
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .font_weight(FontWeight::MEDIUM)
                        .child(title.into()),
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

fn preference_row(
    id: &'static str,
    title: SharedString,
    description: SharedString,
    enabled: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .py_2()
        .flex()
        .items_center()
        .gap_4()
        .cursor_pointer()
        .focusable()
        .tab_index(0)
        .on_click(on_click)
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

fn vault_input(label: SharedString, input: &Entity<InputState>) -> AnyElement {
    div()
        .mt_4()
        .child(
            div()
                .mb_2()
                .text_xs()
                .text_color(theme::text_secondary())
                .child(label),
        )
        .child(Input::new(input).mask_toggle().h(px(38.0)))
        .into_any_element()
}

fn vault_activity_message(app: &TeleArkApp) -> Option<(SharedString, Tone)> {
    match app.vault_activity {
        VaultActivity::Idle | VaultActivity::Working => None,
        VaultActivity::Succeeded => Some((app.tr("vault-operation-succeeded"), Tone::Green)),
        VaultActivity::Failed(kind) => {
            let id = match kind {
                teleark_core::ApplicationErrorKind::InvalidRequest => "vault-error-invalid-request",
                teleark_core::ApplicationErrorKind::Authorization => "vault-error-authorization",
                teleark_core::ApplicationErrorKind::SourceMissing => "vault-error-source-missing",
                teleark_core::ApplicationErrorKind::PermissionDenied => {
                    "vault-error-permission-denied"
                }
                teleark_core::ApplicationErrorKind::Network => "vault-error-network",
                teleark_core::ApplicationErrorKind::NotFound => "vault-error-not-found",
                teleark_core::ApplicationErrorKind::Conflict => "vault-error-conflict",
                teleark_core::ApplicationErrorKind::Capacity => "vault-error-capacity",
                teleark_core::ApplicationErrorKind::Cancelled => "vault-error-cancelled",
                teleark_core::ApplicationErrorKind::Persistence
                | teleark_core::ApplicationErrorKind::SourceChanged => "vault-error-persistence",
                _ => "vault-error-persistence",
            };
            Some((app.tr(id), Tone::Red))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TELEGRAM_API_PANEL_URL;

    #[test]
    fn telegram_api_panel_button_targets_the_official_https_page() {
        assert_eq!(TELEGRAM_API_PANEL_URL, "https://my.telegram.org/apps");
    }
}
