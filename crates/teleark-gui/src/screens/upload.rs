//! Focused upload composer; runtime owns encryption, splitting, and publication.
use crate::{
    app::{TeleArkApp, UnlockIntent, VaultActivity},
    assets::Symbol,
    components,
    layout::LayoutPolicy,
    theme,
};
use gpui_kit::component::{Disableable as _, Icon, IconName, button::ButtonVariants as _};
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, div, prelude::FluentBuilder as _, px,
};

pub fn render_upload_overlay(
    app: &TeleArkApp,
    layout: LayoutPolicy,
    cx: &mut Context<TeleArkApp>,
) -> AnyElement {
    let file_name = app
        .upload_source
        .as_ref()
        .and_then(|path| path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| app.tr("upload-no-file-selected").to_string());
    let channel_name = match &app.storage_status {
        teleark_runtime::StorageChannelStatus::Ready(channel) => channel.name.clone(),
        _ => app.tr("storage-nav-title").to_string(),
    };
    let popup = components::card()
        .relative()
        .w(px(520.0))
        .max_h(px(layout.upload_dialog_height()))
        .shadow_xl()
        .id("upload-popup")
        .overflow_y_scroll()
        .h_auto()
        .p_6()
        .child(
            div()
                .flex()
                .items_center()
                .gap_3()
                .child(components::app_mark(36.0))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(21.0))
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                        .child(app.tr("upload-dialog-title")),
                )
                .child(
                    components::icon_button(
                        "upload-close",
                        IconName::Close,
                        app.tr("action-cancel"),
                    )
                    .ghost()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_upload = false;
                        cx.notify();
                    })),
                ),
        )
        .child(
            div()
                .mt_5()
                .p_5()
                .rounded(theme::RADIUS_LARGE)
                .border_1()
                .border_color(theme::border())
                .bg(theme::canvas())
                .flex()
                .flex_col()
                .items_center()
                .gap_3()
                .child(
                    Icon::new(IconName::File)
                        .size(px(42.0))
                        .text_color(theme::blue()),
                )
                .child(
                    div()
                        .w_full()
                        .truncate()
                        .text_center()
                        .text_sm()
                        .font_weight(gpui_kit::FontWeight::MEDIUM)
                        .child(file_name),
                )
                .when_some(app.upload_source.as_ref(), |area, path| {
                    area.child(
                        div()
                            .w_full()
                            .truncate()
                            .text_center()
                            .text_xs()
                            .text_color(theme::text_muted())
                            .child(path.to_string_lossy().into_owned()),
                    )
                })
                .child(
                    components::button(
                        "upload-choose",
                        app.tr(if app.upload_source.is_some() {
                            "upload-change-file"
                        } else {
                            "upload-choose-file"
                        }),
                        Some(IconName::FolderOpen),
                        false,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.choose_upload_file(cx))),
                ),
        )
        .child(
            div()
                .mt_5()
                .flex()
                .items_center()
                .gap_3()
                .child(
                    Icon::new(Symbol::Lock)
                        .size(px(18.0))
                        .text_color(theme::blue()),
                )
                .child(
                    div()
                        .flex_1()
                        .child(div().text_sm().child(channel_name))
                        .child(
                            div()
                                .mt_1()
                                .text_xs()
                                .text_color(theme::text_muted())
                                .child(
                                    app.telegram_account
                                        .as_ref()
                                        .map(|account| account.display_name.clone())
                                        .unwrap_or_default(),
                                ),
                        ),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme::text_secondary())
                        .child(app.tr("channel-private")),
                ),
        )
        .child(
            div()
                .mt_4()
                .text_sm()
                .text_color(theme::text_secondary())
                .child(app.tr("upload-simple-description")),
        )
        .child(
            components::button(
                "upload-options",
                app.tr("settings-advanced"),
                Some(if app.upload_advanced_expanded {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                }),
                false,
            )
            .ghost()
            .mt_4()
            .on_click(cx.listener(|this, _, _, cx| {
                this.upload_advanced_expanded = !this.upload_advanced_expanded;
                cx.notify();
            })),
        )
        .when(app.upload_advanced_expanded, |popup| {
            popup.child(
                div()
                    .mt_3()
                    .p_4()
                    .rounded(theme::RADIUS_SMALL)
                    .bg(theme::canvas())
                    .text_xs()
                    .text_color(theme::text_secondary())
                    .child(app.tr_with(
                        "upload-current-part-size-description",
                        teleark_i18n::MessageArgs::new().with(
                            "size",
                            teleark_i18n::format::format_bytes(
                                app.locale(),
                                teleark_runtime::encrypted_part_plaintext_limit(),
                            ),
                        ),
                    ))
                    .child(
                        div()
                            .mt_2()
                            .child(app.tr("upload-hide-filename-description")),
                    )
                    .child(
                        div()
                            .mt_2()
                            .child(app.tr("upload-encrypt-metadata-description")),
                    )
                    .child(div().mt_2().child(app.tr("upload-source-checked"))),
            )
        })
        .when_some(
            super::settings::vault_activity_message(app),
            |popup, (message, tone)| {
                popup.child(
                    div()
                        .mt_3()
                        .text_sm()
                        .text_color(tone.foreground())
                        .child(message),
                )
            },
        )
        .child(
            div()
                .mt_5()
                .pt_4()
                .border_t_1()
                .border_color(theme::border())
                .flex()
                .items_center()
                .justify_end()
                .gap_2()
                .child(
                    components::button("upload-cancel", app.tr("action-cancel"), None, false)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_upload = false;
                            cx.notify();
                        })),
                )
                .child(
                    components::button(
                        "upload-add-queue",
                        app.tr(if app.vault_locked {
                            "vault-unlock-action"
                        } else {
                            "upload-add-to-queue"
                        }),
                        Some(IconName::ArrowUp),
                        true,
                    )
                    .disabled(
                        app.upload_source.is_none()
                            || app.vault_activity == VaultActivity::Working
                            || app.storage_channel_id().is_none(),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        if this.vault_locked {
                            this.show_upload = false;
                            this.request_vault_unlock(UnlockIntent::Upload, cx);
                        } else {
                            this.enqueue_vault_upload(cx);
                        }
                    })),
                ),
        );
    let cancel = cx.listener(|this, _, _, cx| {
        this.show_upload = false;
        cx.notify();
    });
    gpui_kit::base::Dialog::new(cx)
        .focus_handle(app.modal_focus.clone())
        .flex()
        .items_center()
        .justify_center()
        .backdrop(div().absolute().inset_0().bg(gpui_kit::rgba(0x10182060)))
        .popup(popup)
        .close_on_backdrop_press(false)
        .on_cancel(move |event, window, cx| {
            cancel(event, window, cx);
            false
        })
        .on_ok(|_, _, _| false)
        .into_any_element()
}
