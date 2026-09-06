//! A distinct home for authenticated TeleArk files and their underlying source.
use crate::assets::Symbol;
use crate::{
    app::{Page, StorageView, TeleArkApp, UnlockIntent},
    components,
    layout::LayoutPolicy,
    theme,
};
use gpui_kit::component::{
    Disableable as _, Icon, IconName,
    button::ButtonVariants as _,
    scroll::ScrollableElement as _,
    tab::{Tab, TabBar},
};
use gpui_kit::{
    AnyElement, Context, IntoElement, ParentElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px,
};
use teleark_runtime::StorageChannelStatus;

impl TeleArkApp {
    pub(crate) fn render_storage_workspace(
        &self,
        _window: &mut Window,
        layout: LayoutPolicy,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let legacy = self.page == Page::LegacyRecovery;
        let ready = legacy || matches!(self.storage_status, StorageChannelStatus::Ready(_));
        let header = div()
            .h(px(76.0))
            .flex_none()
            .flex()
            .items_center()
            .gap_3()
            .child(components::app_mark(38.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_size(px(23.0))
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(self.tr(if legacy {
                                "storage-legacy-title"
                            } else {
                                "storage-nav-title"
                            })),
                    )
                    .child(
                        div()
                            .mt_1()
                            .text_xs()
                            .text_color(theme::text_secondary())
                            .child(self.tr(if legacy {
                                "storage-legacy-description"
                            } else {
                                "storage-private-label"
                            })),
                    ),
            )
            .child(
                components::icon_button(
                    "storage-help",
                    Symbol::Help,
                    self.tr("storage-guide-title"),
                )
                .ghost()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.show_storage_guide = !this.show_storage_guide;
                    cx.notify();
                })),
            )
            .when(ready && !legacy, |bar| {
                bar.child(
                    components::button(
                        "storage-upload",
                        self.tr("storage-channel-upload-action"),
                        Some(IconName::Plus),
                        true,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.request_vault_unlock(UnlockIntent::Upload, cx)
                    })),
                )
            });
        let body = if ready {
            let content = if self.storage_view == StorageView::Files && self.vault_locked {
                self.storage_locked_state(cx)
            } else {
                self.render_telegram_channels(layout, cx)
            };
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    TabBar::new("storage-modes")
                        .segmented()
                        .selected_index(usize::from(self.storage_view == StorageView::RawFiles))
                        .child(Tab::new().label(self.tr("storage-channel-teleark-files")))
                        .child(Tab::new().label(self.tr("storage-channel-telegram-files")))
                        .on_click(cx.listener(|this, index: &usize, _, cx| {
                            this.select_storage(
                                if *index == 0 {
                                    StorageView::Files
                                } else {
                                    StorageView::RawFiles
                                },
                                cx,
                            )
                        })),
                )
                .when(self.show_storage_guide, |body| {
                    body.child(self.storage_guide(true, cx))
                })
                .child(div().flex_1().min_h_0().child(content))
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_h_0()
                .overflow_y_scrollbar()
                .child(
                    div()
                        .max_w(px(720.0))
                        .mx_auto()
                        .py_6()
                        .child(
                            components::card()
                                .p_6()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_3()
                                        .child(components::app_mark(52.0))
                                        .child(
                                            div()
                                                .flex_1()
                                                .text_size(px(23.0))
                                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                                .child(self.tr("storage-setup-title")),
                                        ),
                                )
                                .child(
                                    div()
                                        .mt_4()
                                        .text_sm()
                                        .text_color(theme::text_secondary())
                                        .child(self.tr("storage-setup-description")),
                                )
                                .when(
                                    matches!(self.storage_status, StorageChannelStatus::Missing),
                                    |card| {
                                        card.child(
                                            components::button(
                                                "storage-create",
                                                self.tr("storage-create-action"),
                                                Some(IconName::Plus),
                                                true,
                                            )
                                            .mt_5()
                                            .disabled(
                                                self.storage_loading
                                                    || self.telegram_account.is_none(),
                                            )
                                            .on_click(
                                                cx.listener(|this, _, _, cx| {
                                                    this.create_storage_channel(cx)
                                                }),
                                            ),
                                        )
                                    },
                                )
                                .when(self.storage_loading, |card| {
                                    card.child(
                                        div()
                                            .mt_3()
                                            .text_sm()
                                            .text_color(theme::blue())
                                            .child(self.tr("storage-loading")),
                                    )
                                })
                                .when_some(self.storage_error, |card, kind| {
                                    card.child(
                                        div()
                                            .mt_3()
                                            .text_sm()
                                            .text_color(theme::red())
                                            .child(self.tr(match kind {
                                            teleark_core::ApplicationErrorKind::Network => {
                                                "telegram-error-network"
                                            }
                                            teleark_core::ApplicationErrorKind::Authorization => {
                                                "telegram-error-authorization"
                                            }
                                            _ => "storage-setup-error",
                                        })),
                                    )
                                })
                                .when(
                                    matches!(
                                        self.storage_status,
                                        StorageChannelStatus::Unavailable { .. }
                                    ),
                                    |card| {
                                        card.child(
                                            div()
                                                .mt_3()
                                                .text_sm()
                                                .text_color(theme::amber())
                                                .child(self.tr("storage-unavailable")),
                                        )
                                    },
                                )
                                .children(
                                    match &self.storage_status {
                                        StorageChannelStatus::Choose(channels)
                                        | StorageChannelStatus::Unavailable {
                                            candidates: channels,
                                            ..
                                        } => channels.as_slice(),
                                        _ => &[],
                                    }
                                    .iter()
                                    .map(|channel| {
                                        let id = channel.id;
                                        components::button(
                                            ("storage-candidate", id.unsigned_abs()),
                                            channel.name.clone(),
                                            None,
                                            false,
                                        )
                                        .icon(Symbol::Lock)
                                        .mt_3()
                                        .w_full()
                                        .disabled(self.storage_loading)
                                        .on_click(
                                            cx.listener(move |this, _, _, cx| {
                                                this.choose_storage_channel(id, cx)
                                            }),
                                        )
                                    }),
                                )
                                .child(
                                    components::button(
                                        "storage-discover",
                                        self.tr("storage-discover-action"),
                                        Some(IconName::Redo2),
                                        false,
                                    )
                                    .mt_4()
                                    .ghost()
                                    .disabled(self.storage_loading)
                                    .on_click(cx.listener(
                                        |this, _, _, cx| this.refresh_storage_channel(cx),
                                    )),
                                ),
                        )
                        .child(div().mt_4().child(self.storage_guide(false, cx))),
                )
                .into_any_element()
        };
        div()
            .flex_1()
            .h_full()
            .min_w_0()
            .flex()
            .flex_col()
            .px(px(layout.content_padding()))
            .pb(px(layout.content_padding()))
            .child(header)
            .child(body)
            .into_any_element()
    }

    fn storage_locked_state(&self, cx: &mut Context<Self>) -> AnyElement {
        components::card()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .p_6()
            .child(
                div()
                    .size(px(64.0))
                    .rounded(px(18.0))
                    .bg(theme::blue_soft())
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        Icon::new(Symbol::Lock)
                            .size(px(29.0))
                            .text_color(theme::blue()),
                    ),
            )
            .child(
                div()
                    .mt_2()
                    .text_size(px(21.0))
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .child(self.tr("storage-locked-title")),
            )
            .child(
                div()
                    .max_w(px(420.0))
                    .text_center()
                    .text_sm()
                    .text_color(theme::text_secondary())
                    .child(self.tr("storage-channel-managed-vault-locked")),
            )
            .child(
                components::button("storage-unlock", self.tr("vault-unlock-action"), None, true)
                    .icon(Symbol::Lock)
                    .mt_3()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.request_vault_unlock(UnlockIntent::Browse, cx)
                    })),
            )
            .into_any_element()
    }

    fn storage_guide(&self, compact: bool, cx: &mut Context<Self>) -> AnyElement {
        let steps = [
            (
                Icon::new(Symbol::Lock),
                "storage-guide-private-title",
                "storage-guide-private-body",
            ),
            (
                Icon::new(Symbol::Layers),
                "storage-guide-files-title",
                "storage-guide-files-body",
            ),
            (
                Icon::new(IconName::Eye),
                "storage-guide-raw-title",
                "storage-guide-raw-body",
            ),
            (
                Icon::new(IconName::Asterisk),
                "storage-guide-key-title",
                "storage-guide-key-body",
            ),
        ];
        components::card()
            .p_4()
            .bg(theme::blue_pale())
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(self.tr("storage-guide-title")),
                    )
                    .when(compact, |row| {
                        row.child(
                            components::button(
                                "storage-guide-dismiss",
                                self.tr("storage-guide-done"),
                                None,
                                false,
                            )
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_storage_guide = false;
                                cx.notify();
                            })),
                        )
                    }),
            )
            .child(
                div()
                    .mt_3()
                    .grid()
                    .grid_cols(if compact { 2 } else { 1 })
                    .gap_4()
                    .children(steps.into_iter().map(|(icon, title, body)| {
                        div()
                            .flex()
                            .items_start()
                            .gap_3()
                            .child(Icon::new(icon).size(px(17.0)).text_color(theme::blue()))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                            .child(self.tr(title)),
                                    )
                                    .child(
                                        div()
                                            .mt_1()
                                            .text_xs()
                                            .text_color(theme::text_secondary())
                                            .child(self.tr(body)),
                                    ),
                            )
                    })),
            )
            .into_any_element()
    }
}

impl TeleArkApp {
    pub(crate) fn render_storage_managed(
        &self,
        layout: LayoutPolicy,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let query = self.search_input.read(cx).value().to_lowercase();
        let files: Vec<_> = self
            .managed_vault_files
            .iter()
            .filter(|file| query.is_empty() || file.logical_name.to_lowercase().contains(&query))
            .cloned()
            .collect();
        let selected = files
            .iter()
            .find(|file| Some(file.manifest_message_id) == self.selected_telegram_message_id)
            .cloned();
        let count = files.len();
        let rows = std::sync::Arc::new(files);
        let list = gpui_kit::uniform_list(
            "managed-files",
            count,
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                range
                    .filter_map(|index| rows.get(index))
                    .map(|file| {
                        let message_id = file.manifest_message_id;
                        let package_id = file.package_numeric_id;
                        gpui_kit::base::Button::new(("managed-file", package_id))
                            .accessibility_label(file.logical_name.clone())
                            .w_full()
                            .h(px(60.0))
                            .px_4()
                            .flex()
                            .items_center()
                            .gap_3()
                            .border_b_1()
                            .border_color(theme::border_subtle())
                            .when(
                                this.selected_telegram_message_id == Some(message_id),
                                |row| row.bg(theme::blue_pale()),
                            )
                            .child(
                                Icon::new(IconName::File)
                                    .size(px(22.0))
                                    .text_color(theme::blue()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_left()
                                    .child(
                                        div()
                                            .truncate()
                                            .text_size(px(13.0))
                                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                                            .child(file.logical_name.clone()),
                                    )
                                    .child(
                                        div()
                                            .mt_1()
                                            .text_xs()
                                            .text_color(theme::text_muted())
                                            .child(teleark_i18n::format::format_bytes(
                                                this.locale(),
                                                file.size_bytes,
                                            )),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme::green())
                                    .child(this.tr("storage-channel-manifest-authenticated")),
                            )
                            .child(
                                components::icon_button(
                                    ("managed-download", package_id),
                                    IconName::ArrowDown,
                                    this.tr("storage-channel-download-restored-action"),
                                )
                                .ghost()
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.download_managed_vault_file(package_id, cx);
                                    },
                                )),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.selected_telegram_message_id = Some(message_id);
                                this.show_channel_detail = true;
                                cx.notify();
                            }))
                            .into_any_element()
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .w_full()
        .flex_1()
        .min_h_0();
        components::card()
            .h_full()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .relative()
            .overflow_hidden()
            .child(
                div()
                    .h(px(44.0))
                    .px_4()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .child(self.tr("storage-channel-managed-title")),
                    )
                    .child(
                        components::icon_button(
                            "managed-refresh",
                            IconName::Redo2,
                            self.tr("telegram-files-refresh-action"),
                        )
                        .ghost()
                        .disabled(self.managed_scan_loading)
                        .on_click(cx.listener(|this, _, _, cx| this.scan_managed_vault_files(cx))),
                    ),
            )
            .when(count > 0, |body| body.child(list))
            .when(count == 0, |body| {
                body.child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap_3()
                        .p_5()
                        .child(
                            Icon::new(IconName::FolderOpen)
                                .size(px(36.0))
                                .text_color(theme::blue()),
                        )
                        .child(
                            div()
                                .max_w(px(420.0))
                                .text_center()
                                .text_sm()
                                .text_color(theme::text_secondary())
                                .child(self.tr(if self.managed_scan_loading {
                                    "storage-loading"
                                } else {
                                    "storage-channel-managed-empty"
                                })),
                        ),
                )
            })
            .when_some(
                super::settings::vault_activity_message(self),
                |body, (message, tone)| {
                    body.child(
                        div()
                            .px_4()
                            .py_2()
                            .text_xs()
                            .text_color(tone.foreground())
                            .child(message),
                    )
                },
            )
            .child(
                div()
                    .px_4()
                    .py_2()
                    .border_t_1()
                    .border_color(theme::border())
                    .text_xs()
                    .text_color(theme::text_muted())
                    .child(
                        self.tr_with(
                            "storage-scan-summary",
                            teleark_i18n::MessageArgs::new()
                                .with(
                                    "count",
                                    teleark_i18n::format::format_integer(
                                        self.locale(),
                                        count as u64,
                                    ),
                                )
                                .with(
                                    "rejected",
                                    teleark_i18n::format::format_integer(
                                        self.locale(),
                                        self.managed_vault_rejected as u64,
                                    ),
                                ),
                        ),
                    ),
            )
            .when_some(
                selected.filter(|_| self.show_channel_detail),
                |body, selected| {
                    body.child(
                        components::card()
                            .absolute()
                            .right_0()
                            .top_0()
                            .bottom_0()
                            .w(px(330.0))
                            .shadow_lg()
                            .flex()
                            .flex_col()
                            .overflow_hidden()
                            .child(
                                div().px_3().py_2().flex().justify_end().child(
                                    components::icon_button(
                                        "managed-close-detail",
                                        IconName::Close,
                                        self.tr("action-close-details"),
                                    )
                                    .ghost()
                                    .on_click(cx.listener(
                                        |this, _, _, cx| {
                                            this.show_channel_detail = false;
                                            cx.notify();
                                        },
                                    )),
                                ),
                            )
                            .child(self.render_managed_package_detail(Some(&selected), layout, cx)),
                    )
                },
            )
            .into_any_element()
    }
}
