//! Desktop shell: permanent primary destinations and a separately virtualized
//! channel list. Refreshing source data cannot move the transfer controls.
use crate::assets::Symbol;

use super::*;
use gpui_kit::{
    base::Button as NavButton,
    component::{
        avatar::Avatar,
        button::ButtonVariants as _,
        collapsible::Collapsible,
        input::Input,
        sidebar::{Sidebar, SidebarMenuItem},
    },
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
        NavButton::new(id)
            .accessibility_label(title.clone())
            .selected(selected)
            .w_full()
            .h(px(38.0))
            .px_3()
            .flex()
            .items_center()
            .gap_3()
            .rounded(px(8.0))
            .text_size(px(13.0))
            .text_color(if selected {
                theme::blue()
            } else {
                theme::text_primary()
            })
            .when(selected, |button| button.bg(theme::blue_soft()))
            .on_click(cx.listener(move |this, _, _, cx| match id {
                "nav-storage" => {
                    this.page = Page::Storage;
                    this.select_storage(StorageView::Files, cx);
                }
                "nav-transfers-all" => {
                    this.nav_selection = id;
                    this.set_page(Page::Transfers, cx);
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
            .child(
                Icon::new(icon)
                    .size(px(18.0))
                    .text_color(if id == "nav-storage" {
                        theme::blue()
                    } else {
                        theme::text_secondary()
                    }),
            )
            .child(div().flex_1().text_left().child(title))
            .when(id == "nav-storage", |button| {
                button.child(
                    Icon::new(Symbol::Lock)
                        .size(px(12.0))
                        .text_color(theme::blue()),
                )
            })
            .when(id == "nav-transfers-all", |button| {
                let active = self
                    .transfer_rows()
                    .iter()
                    .filter(|row| {
                        !row.batch_child
                            && matches!(
                                row.state,
                                crate::mock::TransferState::Uploading
                                    | crate::mock::TransferState::Downloading
                                    | crate::mock::TransferState::Waiting
                                    | crate::mock::TransferState::Paused
                            )
                    })
                    .count();
                button.when(active > 0, |button| {
                    button.child(
                        div()
                            .text_xs()
                            .text_color(theme::text_secondary())
                            .child(format_integer(self.locale(), active as u64)),
                    )
                })
            })
            .into_any_element()
    }

    pub(super) fn render_sidebar(
        &self,
        layout: LayoutPolicy,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let header = div()
            .w_full()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .h(px(52.0))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(components::app_mark(30.0))
                    .child(
                        div()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .text_size(px(17.0))
                            .child("TeleArk"),
                    ),
            )
            .child(self.primary_nav(
                "nav-transfers-all",
                "transfer-title",
                IconName::ArrowDown,
                self.page == Page::Transfers,
                cx,
            ))
            .child(self.primary_nav(
                "nav-storage",
                "storage-nav-title",
                IconName::FolderOpen,
                self.page == Page::Storage,
                cx,
            ))
            .child(
                div()
                    .mt_4()
                    .h(px(26.0))
                    .px_2()
                    .flex()
                    .items_center()
                    .child(
                        NavButton::new("channels-disclosure")
                            .accessibility_label(self.tr("nav-channels"))
                            .flex_1()
                            .flex()
                            .items_center()
                            .gap_1()
                            .text_size(px(11.0))
                            .text_color(theme::text_muted())
                            .child(
                                Icon::new(if self.sidebar_channels_expanded {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                })
                                .size(px(12.0)),
                            )
                            .child(self.tr("nav-channels"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.sidebar_channels_expanded = !this.sidebar_channels_expanded;
                                cx.notify();
                            })),
                    )
                    .child(
                        components::icon_button(
                            "channels-refresh",
                            IconName::Redo2,
                            self.tr("shell-refresh-channels"),
                        )
                        .ghost()
                        .size(px(26.0))
                        .on_click(cx.listener(|this, _, _, cx| this.load_telegram_dialogs(cx))),
                    ),
            );
        let channels: Vec<_> = if self.sidebar_channels_expanded {
            self.telegram_chats
                .iter()
                .filter(|chat| {
                    chat.kind == TelegramChatKind::Channel
                        && Some(chat.id) != self.storage_channel_id()
                })
                .map(|chat| {
                    let chat_id = chat.id;
                    SidebarMenuItem::new(chat.name.clone())
                        .icon(Icon::new(Symbol::Hash).size(px(16.0)))
                        .active(
                            self.page == Page::Channel && self.selected_chat_id == Some(chat_id),
                        )
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.select_channel(chat_id, cx)),
                        )
                })
                .collect()
        } else {
            Vec::new()
        };
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
        let footer = div()
            .w_full()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                Collapsible::new()
                    .open(self.sidebar_tools_expanded)
                    .child(
                        NavButton::new("utilities-disclosure")
                            .accessibility_label(self.tr("shell-utilities"))
                            .w_full()
                            .h(px(30.0))
                            .px_3()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_xs()
                            .text_color(theme::text_secondary())
                            .child(
                                Icon::new(if self.sidebar_tools_expanded {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                })
                                .size(px(12.0)),
                            )
                            .child(self.tr("shell-utilities"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.sidebar_tools_expanded = !this.sidebar_tools_expanded;
                                cx.notify();
                            })),
                    )
                    .content(self.primary_nav(
                        "nav-library",
                        "shell-local-library",
                        IconName::Folder,
                        self.page == Page::Library,
                        cx,
                    )),
            )
            .child(self.primary_nav(
                "nav-settings",
                "settings-title",
                IconName::Settings,
                self.page == Page::Settings,
                cx,
            ))
            .child(
                NavButton::new("sidebar-account")
                    .accessibility_label(self.tr("shell-account"))
                    .h(px(48.0))
                    .w_full()
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(self.account_avatar_element(27.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_left()
                            .truncate()
                            .text_size(px(12.0))
                            .child(
                                self.telegram_account
                                    .as_ref()
                                    .map(|a| SharedString::from(a.display_name.clone()))
                                    .unwrap_or_else(|| self.tr("telegram-header-login-action")),
                            ),
                    )
                    .child(
                        Icon::new(IconName::ChevronRight)
                            .size(px(12.0))
                            .text_color(theme::text_muted()),
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.set_page(Page::Account, cx))),
            )
            .child(
                div()
                    .mx_3()
                    .pt_3()
                    .pb_2()
                    .border_t_1()
                    .border_color(theme::border())
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_size(px(10.0))
                    .text_color(theme::text_muted())
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .child(Icon::new(IconName::ArrowDown).size(px(11.0)))
                            .child(format_speed(self.locale(), rates.download_bytes_per_second)),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .child(Icon::new(IconName::ArrowUp).size(px(11.0)))
                            .child(format_speed(self.locale(), rates.upload_bytes_per_second)),
                    ),
            )
            .when_some(self.overall_storage_metrics.as_ref(), |footer, metrics| {
                footer.child(
                    div()
                        .px_3()
                        .pb_2()
                        .text_size(px(10.0))
                        .text_color(theme::text_muted())
                        .child(
                            self.tr_with(
                                "shell-disk-summary",
                                MessageArgs::new()
                                    .with(
                                        "free",
                                        format_bytes(self.locale(), metrics.available_bytes),
                                    )
                                    .with(
                                        "used",
                                        format_bytes(self.locale(), metrics.app_used_bytes),
                                    ),
                            ),
                        ),
                )
            });
        Sidebar::new("teleark-sidebar")
            .collapsible(false)
            .w(px(layout.sidebar_width()))
            .bg(theme::sidebar())
            .header(header)
            .children(channels)
            .footer(footer)
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
