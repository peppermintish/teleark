//! Desktop shell: permanent primary destinations and a separately virtualized
//! channel list. Refreshing source data cannot move the transfer controls.
use crate::assets::Symbol;

use super::*;
use gpui_kit::component::{
    Collapsible,
    avatar::Avatar,
    button::ButtonVariants as _,
    input::Input,
    sidebar::{Sidebar, SidebarItem, SidebarMenuItem},
};

/// Sidebar virtualizes its `SidebarItem::render` calls, but eagerly retains its
/// children. Keep those children as identities, not allocated menu widgets.
#[derive(Clone)]
struct ChannelSidebarItem {
    owner: gpui_kit::WeakEntity<TeleArkApp>,
    source_index: usize,
    chat_id: i64,
}

impl Collapsible for ChannelSidebarItem {
    fn is_collapsed(&self) -> bool {
        false
    }
    fn collapsed(self, _: bool) -> Self {
        self
    }
}

impl SidebarItem for ChannelSidebarItem {
    fn render(
        self,
        id: impl Into<gpui_kit::ElementId>,
        window: &mut Window,
        cx: &mut gpui_kit::App,
    ) -> impl IntoElement {
        let item = self
            .owner
            .update(cx, |app, cx| {
                let chat = app.telegram_chats.get(self.source_index)?;
                // A source refresh can invalidate a retained item before layout.
                if chat.id != self.chat_id
                    || chat.kind != TelegramChatKind::Channel
                    || Some(chat.id) == app.storage_channel_id()
                {
                    return None;
                }
                #[cfg(test)]
                MATERIALIZED_CHANNELS.with(|count| count.set(count.get() + 1));
                Some(
                    SidebarMenuItem::new(chat.name.clone())
                        .active(app.selected_chat_id == Some(self.chat_id))
                        .on_click(cx.listener(move |app, _, _, cx| {
                            if app.telegram_chats.iter().any(|chat| {
                                chat.id == self.chat_id && chat.kind == TelegramChatKind::Channel
                            }) && Some(self.chat_id) != app.storage_channel_id()
                            {
                                app.select_channel(self.chat_id, cx);
                            }
                        })),
                )
            })
            .ok()
            .flatten();
        div()
            .debug_selector(move || format!("channel-sidebar-row-{}", self.chat_id))
            .when_some(item, |row, item| {
                row.child(item.render(id, window, cx).into_any_element())
            })
            .into_any_element()
    }
}

#[cfg(test)]
thread_local! { static MATERIALIZED_CHANNELS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

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
                .on_click(cx.listener(|this, _, window, cx| {
                    if this.vault_locked {
                        this.request_vault_unlock(UnlockIntent::Browse, cx);
                    } else {
                        this.lock_vault(window, cx);
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
        icon: impl Into<Icon>,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let title = self.tr(label);
        components::button(id, "", None, false)
            .icon(icon)
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
                Symbol::Transfer,
                self.page == Page::Transfers,
                cx,
            ))
            .child(self.primary_nav(
                "nav-library",
                "nav-library",
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
            .into_any_element()
    }

    pub(super) fn render_channels_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let channels = self.channel_sidebar_items(cx);
        Sidebar::new("channels-sidebar")
            .collapsible(false)
            .w_full()
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
            .footer(
                div()
                    .debug_selector(|| "channel-list-width-feedback".into())
                    .text_xs()
                    .text_color(theme::text_secondary())
                    .child(self.tr(match self.preference_persistence {
                        PreferencePersistence::Idle => "channel-list-resize-hint",
                        PreferencePersistence::Saving => "settings-preferences-saving",
                        PreferencePersistence::Saved => "settings-preferences-saved",
                        PreferencePersistence::Failed => "settings-preferences-failed",
                    }))
                    .when(
                        self.preference_persistence == PreferencePersistence::Failed,
                        |footer| {
                            footer.child(
                                components::button(
                                    "channel-width-save-retry",
                                    self.tr("common-retry"),
                                    None,
                                    false,
                                )
                                .debug_selector(|| "channel-width-save-retry".into())
                                .ghost()
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.persist_preferences(cx)),
                                ),
                            )
                        },
                    ),
            )
            .into_any_element()
    }

    fn channel_sidebar_items(&self, cx: &Context<Self>) -> Vec<ChannelSidebarItem> {
        let storage_id = self.storage_channel_id();
        self.telegram_chats
            .iter()
            .enumerate()
            .filter(|(_, chat)| {
                chat.kind == TelegramChatKind::Channel && Some(chat.id) != storage_id
            })
            .map(|(source_index, chat)| ChannelSidebarItem {
                owner: cx.weak_entity(),
                source_index,
                chat_id: chat.id,
            })
            .collect()
    }

    pub(super) fn render_status_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut rates = teleark_runtime::TransferRates::default();
        for item in self
            .native_transfer_view
            .items
            .iter()
            .filter(|item| item.state == teleark_runtime::ChannelDownloadState::Running)
        {
            rates.download_bytes_per_second = rates
                .download_bytes_per_second
                .saturating_add(item.current_bytes_per_second.unwrap_or(0));
        }
        for item in self
            .vault_transfer_view
            .items
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
        div()
            .debug_selector(|| "global-background-status".into())
            .h(px(28.0))
            .flex_none()
            .px_4()
            .flex()
            .items_center()
            .gap_2()
            .border_t_1()
            .border_color(theme::border())
            .bg(theme::surface())
            .text_xs()
            .text_color(theme::text_secondary())
            .child(
                components::button(
                    "status-disk-space",
                    if self.preference_persistence == PreferencePersistence::Saving {
                        self.tr("settings-preferences-saving")
                    } else if self.preference_persistence == PreferencePersistence::Failed {
                        self.tr("settings-preferences-failed")
                    } else {
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
                            .unwrap_or_else(|| self.tr("shell-disk-space-unavailable"))
                    },
                    Some(IconName::HardDrive),
                    false,
                )
                .ghost()
                .h(px(28.0))
                .tooltip(self.tr(
                    if self.preference_persistence == PreferencePersistence::Failed {
                        "common-retry"
                    } else {
                        "settings-managed-root-picker"
                    },
                ))
                .on_click(cx.listener(|this, _, _, cx| {
                    if this.preference_persistence == PreferencePersistence::Failed {
                        this.persist_preferences(cx);
                        return;
                    }
                    this.settings_section = SettingsSection::Storage;
                    this.set_page(Page::Settings, cx);
                })),
            )
            .when(self.dialogs.has_activity(), |bar| {
                bar.child(self.render_dialog_status(cx))
            })
            .when(self.channel_sync_snapshot.is_some(), |bar| {
                bar.child(
                    components::button(
                        "shell-channel-sync",
                        self.channel_sync_label(),
                        None,
                        false,
                    )
                    .ghost()
                    .tooltip(self.channel_sync_timing())
                    .max_w(px(280.0))
                    .min_w_0()
                    .flex_1()
                    .h(px(24.0))
                    .overflow_hidden()
                    .debug_selector(|| "global-sync-details".into())
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.channel_sync_details = !app.channel_sync_details;
                        cx.notify();
                    })),
                )
            })
            .when_some(self.managed_change_warning(), |bar, warning| {
                bar.child(
                    components::button("managed-watch-alert", warning.clone(), None, false)
                        .ghost()
                        .text_color(theme::amber())
                        .max_w(px(220.0))
                        .overflow_hidden()
                        .tooltip(warning)
                        .debug_selector(|| "managed-watch-alert".into())
                        .on_click(cx.listener(|app, _, _, cx| {
                            app.channel_sync_details = true;
                            cx.notify();
                        })),
                )
            })
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

#[cfg(test)]
mod tests {
    use super::*;

    fn populate(app: &mut TeleArkApp) {
        app.telegram_chats = (1..=10_000)
            .map(|id| TelegramChatSummary {
                id,
                name: format!("Channel · 频道 · チャンネル {id} with a long archive title"),
                username: None,
                kind: TelegramChatKind::Channel,
                sync_pts: None,
            })
            .collect();
    }

    #[gpui_kit::test]
    fn channel_sidebar_only_constructs_visible_menus_and_keeps_selection(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Channel);
        cx.simulate_resize(gpui_kit::size(px(900.0), px(600.0)));
        app.update(cx, |app, cx| {
            populate(app);
            assert_eq!(app.channel_sidebar_items(cx).len(), 9_999); // private storage excluded
            MATERIALIZED_CHANNELS.with(|count| count.set(0));
            cx.notify();
        });
        cx.run_until_parked();
        let count = MATERIALIZED_CHANNELS.with(|count| count.get());
        assert!(count > 0 && count < 200, "constructed {count} menus");
        let row = cx
            .debug_bounds("channel-sidebar-row-1")
            .expect("first channel");
        cx.simulate_click(row.center(), gpui_kit::Modifiers::default());
        cx.run_until_parked();
        app.read_with(cx, |app, _| assert_eq!(app.selected_chat_id, Some(1)));
        let panel = cx.debug_bounds("channel-list-panel").expect("sidebar");
        cx.simulate_event(gpui_kit::ScrollWheelEvent {
            position: panel.center(),
            delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.0), px(-500.0))),
            ..Default::default()
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("channel-sidebar-row-1").is_none());
        assert!(cx.debug_bounds("channel-list-width-feedback").is_some());
        app.read_with(cx, |app, _| assert_eq!(app.selected_chat_id, Some(1)));
        let stale = app.update(cx, |app, cx| {
            let stale = app.channel_sidebar_items(cx).remove(0);
            app.telegram_chats.swap(0, 1);
            stale
        });
        MATERIALIZED_CHANNELS.with(|count| count.set(0));
        cx.update(|window, cx| {
            let _ = stale.render("stale-source", window, cx);
        });
        assert_eq!(MATERIALIZED_CHANNELS.with(|count| count.get()), 0);
    }

    #[gpui_kit::test]
    #[ignore = "manual comparative benchmark; no wall-time assertion"]
    fn perf_channel_sidebar_construction(cx: &mut gpui_kit::TestAppContext) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Channel);
        app.update(cx, |app, cx| {
            populate(app);
            let started = std::time::Instant::now();
            for _ in 0..100 {
                let items: Vec<_> = app
                    .telegram_chats
                    .iter()
                    .filter(|chat| Some(chat.id) != app.storage_channel_id())
                    .map(|chat| {
                        let chat_id = chat.id;
                        SidebarMenuItem::new(chat.name.clone())
                            .active(app.selected_chat_id == Some(chat_id))
                            .on_click(
                                cx.listener(move |app, _, _, cx| app.select_channel(chat_id, cx)),
                            )
                    })
                    .collect();
                std::hint::black_box(items);
            }
            let old = started.elapsed();
            let started = std::time::Instant::now();
            for _ in 0..100 {
                std::hint::black_box(app.channel_sidebar_items(cx));
            }
            eprintln!(
                "channel_sidebar channels=10000 renders=100 old_us={} new_us={}",
                old.as_micros(),
                started.elapsed().as_micros()
            );
        });
    }
}
