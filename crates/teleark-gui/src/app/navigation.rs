//! Desktop shell: permanent primary destinations and a separately virtualized
//! channel list. Refreshing source data cannot move the transfer controls.
use crate::assets::Symbol;

use super::*;
use gpui_kit::component::{
    avatar::Avatar,
    button::ButtonVariants as _,
    input::Input,
    sidebar::{Sidebar, SidebarMenuItem},
};

impl TeleArkApp {
    pub(crate) fn account_avatar_element(&self, size: f32) -> Avatar {
        Avatar::new()
            .name(
                self.telegram_account
                    .as_ref()
                    .map(|account| account.display_name.clone())
                    .unwrap_or_default(),
            )
            .when_some(self.account_avatar.clone(), |avatar, image| {
                avatar.src(image)
            })
            .size(px(size))
            .text_size(px((size * 0.32).max(11.0)))
    }

    pub(super) fn render_header(
        &self,
        window: &mut Window,
        layout: LayoutPolicy,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .h(theme::HEADER_HEIGHT)
            .flex_none()
            .px(px(layout.content_padding()))
            .flex()
            .items_center()
            .gap_3()
            .bg(theme::surface())
            .border_b_1()
            .border_color(theme::border_subtle())
            .when(self.visual_preview, |bar| {
                bar.child(components::badge(self.tr("shell-preview"), Tone::Neutral))
            })
            .child(div().flex_1())
            .when(
                matches!(
                    self.page,
                    Page::Channel
                        | Page::Storage
                        | Page::LegacyRecovery
                        | Page::Transfers
                        | Page::Library
                ),
                |bar| {
                    bar.child(
                        div()
                            .w(px(if layout.is_compact() { 250.0 } else { 320.0 }))
                            .child(
                                Input::new(&self.search_input)
                                    .prefix(Icon::new(IconName::Search).size(px(15.0)))
                                    .cleanable(true)
                                    .h(px(32.0)),
                            ),
                    )
                },
            )
            .child(
                components::icon_button(
                    "shell-vault",
                    if self.vault_locked {
                        Symbol::Lock
                    } else {
                        Symbol::Unlock
                    },
                    self.tr(if self.vault_locked {
                        "vault-unlock-action"
                    } else {
                        "vault-lock-action"
                    }),
                )
                .ghost()
                .on_click(cx.listener(|this, _, _, cx| {
                    if this.vault_locked {
                        this.request_vault_unlock(UnlockIntent::Browse, cx);
                    } else {
                        this.lock_vault(cx);
                    }
                })),
            )
            .when(window.is_fullscreen(), |bar| {
                bar.child(
                    components::icon_button(
                        "shell-exit-fullscreen",
                        IconName::Minimize,
                        self.tr("window-exit-fullscreen-action"),
                    )
                    .ghost()
                    .on_click(|_, window, _| window.toggle_fullscreen()),
                )
            })
            .into_any_element()
    }

    fn primary_nav(
        &self,
        id: &'static str,
        label: &'static str,
        icon: IconName,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let title = self.tr(label);
        components::button(id, "", Some(icon), false)
            .ghost()
            .tooltip(title.clone())
            .accessibility_label(title.clone())
            .w_full()
            .h(px(42.0))
            .when(!self.preferences.sidebar_collapsed, |button| {
                button.label(title).justify_start()
            })
            .when(selected, |button| {
                button.bg(theme::blue_soft()).text_color(theme::blue())
            })
            .on_click(cx.listener(move |this, _, _, cx| match id {
                "nav-storage" => {
                    this.page = Page::Storage;
                    this.select_storage(StorageView::Files, cx);
                }
                "nav-transfers-all" => {
                    this.nav_selection = id;
                    this.set_page(Page::Transfers, cx);
                }
                "nav-channels" => {
                    let eligible = |chat: &&TelegramChatSummary| {
                        chat.kind == TelegramChatKind::Channel
                            && Some(chat.id) != this.storage_channel_id()
                    };
                    let target = this
                        .telegram_chats
                        .iter()
                        .filter(eligible)
                        .find(|chat| Some(chat.id) == this.last_channel_id)
                        .or_else(|| this.telegram_chats.iter().find(eligible))
                        .map(|chat| chat.id);
                    if let Some(chat_id) = target {
                        this.select_channel(chat_id, cx);
                    } else {
                        this.set_page(Page::Channel, cx);
                    }
                }
                "nav-settings" => {
                    this.nav_selection = id;
                    this.set_page(Page::Settings, cx);
                }
                "nav-library" => {
                    this.nav_selection = "nav-all";
                    this.set_page(Page::Library, cx);
                }
                _ => {
                    this.nav_selection = "nav-account";
                    this.set_page(Page::Account, cx);
                }
            }))
            .into_any_element()
    }

    pub(super) fn render_sidebar(
        &self,
        layout: LayoutPolicy,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let collapsed = self.preferences.sidebar_collapsed;
        div()
            .w(px(layout.sidebar_width()))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .px_2()
            .py_3()
            .gap_2()
            .bg(theme::sidebar())
            .border_r_1()
            .border_color(theme::border())
            .child(
                div()
                    .h(px(50.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap_3()
                    .child(components::app_mark(36.0))
                    .when(!collapsed, |brand| {
                        brand.child(
                            div()
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .text_size(px(17.0))
                                .child("TeleArk"),
                        )
                    }),
            )
            .child(self.primary_nav(
                "nav-storage",
                "storage-nav-title",
                IconName::FolderOpen,
                self.page == Page::Storage,
                cx,
            ))
            .child(self.primary_nav(
                "nav-channels",
                "nav-channels",
                IconName::Globe,
                self.page == Page::Channel,
                cx,
            ))
            .child(self.primary_nav(
                "nav-transfers-all",
                "transfer-title",
                IconName::ArrowDown,
                self.page == Page::Transfers,
                cx,
            ))
            .child(self.primary_nav(
                "nav-library",
                "shell-local-library",
                IconName::Folder,
                self.page == Page::Library || self.page == Page::FileDetail,
                cx,
            ))
            .child(div().flex_1())
            .child(self.primary_nav(
                "nav-settings",
                "settings-title",
                IconName::Settings,
                self.page == Page::Settings,
                cx,
            ))
            .child(self.primary_nav(
                "nav-account",
                "shell-account",
                IconName::User,
                self.page == Page::Account,
                cx,
            ))
            .child(div().h(px(1.0)).my_1().bg(theme::border()))
            .child(
                components::button(
                    "sidebar-toggle",
                    "",
                    Some(if collapsed {
                        IconName::PanelLeftOpen
                    } else {
                        IconName::PanelLeftClose
                    }),
                    false,
                )
                .ghost()
                .w_full()
                .h(px(36.0))
                .accessibility_label(self.tr(if collapsed {
                    "shell-expand-navigation"
                } else {
                    "shell-collapse-navigation"
                }))
                .tooltip(self.tr(if collapsed {
                    "shell-expand-navigation"
                } else {
                    "shell-collapse-navigation"
                }))
                .when(!collapsed, |button| {
                    button
                        .label(self.tr("shell-collapse-navigation"))
                        .justify_start()
                })
                .on_click(cx.listener(|this, _, _, cx| {
                    this.preferences.sidebar_collapsed = !this.preferences.sidebar_collapsed;
                    this.persist_preferences(cx);
                    cx.notify();
                })),
            )
            .into_any_element()
    }

    pub(super) fn render_channels_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let channels = self
            .telegram_chats
            .iter()
            .filter(|chat| {
                chat.kind == TelegramChatKind::Channel && Some(chat.id) != self.storage_channel_id()
            })
            .map(|chat| {
                let chat_id = chat.id;
                SidebarMenuItem::new(chat.name.clone())
                    .icon(Icon::new(Symbol::Hash).size(px(16.0)))
                    .active(self.selected_chat_id == Some(chat_id))
                    .on_click(cx.listener(move |this, _, _, cx| this.select_channel(chat_id, cx)))
            })
            .collect::<Vec<_>>();
        Sidebar::new("channels-sidebar")
            .collapsible(false)
            .w(px(208.0))
            .header(
                div()
                    .h(px(42.0))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_sm().child(self.tr("nav-channels")))
                    .child(
                        components::icon_button(
                            "channels-refresh",
                            IconName::Redo2,
                            self.tr("shell-refresh-channels"),
                        )
                        .ghost()
                        .on_click(cx.listener(|this, _, _, cx| this.load_telegram_dialogs(cx))),
                    ),
            )
            .children(channels)
            .into_any_element()
    }

    pub(super) fn render_status_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut rates = self
            .transfers
            .as_ref()
            .and_then(|transfers| transfers.current_rates().ok())
            .unwrap_or_default();
        if let Some(vault) = &self.vault {
            for item in vault
                .transfers()
                .iter()
                .filter(|item| item.state == teleark_runtime::VaultTransferState::Running)
            {
                match item.direction {
                    teleark_runtime::VaultTransferDirection::Upload => {
                        rates.upload_bytes_per_second = rates
                            .upload_bytes_per_second
                            .saturating_add(item.telemetry.goodput_bytes_per_second)
                    }
                    teleark_runtime::VaultTransferDirection::Download => {
                        rates.download_bytes_per_second = rates
                            .download_bytes_per_second
                            .saturating_add(item.telemetry.goodput_bytes_per_second)
                    }
                }
            }
        }
        div()
            .h(px(32.0))
            .flex_none()
            .px_4()
            .flex()
            .items_center()
            .gap_4()
            .border_t_1()
            .border_color(theme::border())
            .bg(theme::surface())
            .text_xs()
            .text_color(theme::text_secondary())
            .child(
                components::button(
                    "status-disk-space",
                    self.volume_space
                        .map(|space| {
                            self.tr_with(
                                "shell-free-disk-space",
                                MessageArgs::new().with(
                                    "free",
                                    format_bytes(self.locale(), space.available_bytes),
                                ),
                            )
                        })
                        .unwrap_or_else(|| self.tr("shell-disk-space-unavailable")),
                    Some(IconName::HardDrive),
                    false,
                )
                .ghost()
                .h(px(28.0))
                .tooltip(self.tr("settings-managed-root-picker"))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.settings_section = SettingsSection::Storage;
                    this.set_page(Page::Settings, cx);
                })),
            )
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(Icon::new(IconName::ArrowDown).size(px(12.0)))
                    .child(format_speed(self.locale(), rates.download_bytes_per_second)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(Icon::new(IconName::ArrowUp).size(px(12.0)))
                    .child(format_speed(self.locale(), rates.upload_bytes_per_second)),
            )
            .into_any_element()
    }

    pub(super) fn render_page(
        &self,
        window: &mut Window,
        layout: LayoutPolicy,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match self.page {
            Page::Account => self.render_account(window, layout, cx),
            Page::Storage | Page::LegacyRecovery => {
                self.render_storage_workspace(window, layout, cx)
            }
            Page::Library => self.render_library(window, layout, cx),
            Page::Transfers => self.render_transfers(window, layout, cx),
            Page::FileDetail => self.render_file_detail(window, layout, cx),
            Page::Channel => self.render_channel(window, layout, cx),
            Page::Settings => self.render_settings(window, layout, cx),
        }
    }
}
