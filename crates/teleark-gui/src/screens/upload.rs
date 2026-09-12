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
    let folder_rejected = matches!(
        app.vault_activity,
        VaultActivity::Failed(teleark_core::ApplicationErrorKind::UploadFolderUnsupported)
    );
    let source_summary = app.tr_with(
        "upload-selection-summary",
        teleark_i18n::MessageArgs::new()
            .with(
                "count",
                teleark_i18n::format::format_integer(app.locale(), app.upload_sources.len() as u64),
            )
            .with(
                "size",
                teleark_i18n::format::format_bytes(
                    app.locale(),
                    app.upload_sources
                        .iter()
                        .map(|source| source.size_bytes)
                        .sum(),
                ),
            ),
    );
    let channel_name = app.storage_status.usable_channel().map_or_else(
        || app.tr("storage-nav-title").to_string(),
        |channel| channel.name.clone(),
    );
    let popup = components::card()
        .relative()
        .w(px(520.0))
        .h(px((if app.upload_sources.is_empty() {
            512.0_f32
        } else {
            640.0_f32
        })
        .min(layout.upload_dialog_height())))
        .shadow_xl()
        .id("upload-popup")
        .overflow_hidden()
        .occlude()
        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
        .flex()
        .flex_col()
        .child(
            div()
                .flex_none()
                .px_6()
                .py_5()
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
        .when(folder_rejected, |popup| {
            popup.child(
                div()
                    .flex_none()
                    .mx_6()
                    .mb_2()
                    .px_4()
                    .py_3()
                    .rounded(theme::RADIUS_MEDIUM)
                    .bg(components::Tone::Red.background())
                    .text_sm()
                    .text_color(components::Tone::Red.foreground())
                    .debug_selector(|| "upload-folder-error".into())
                    .child(app.tr("upload-error-folder")),
            )
        })
        .child(components::inspector_body(
            "upload-body",
            &app.upload_body_scroll,
            div()
                .px_6()
                .pb_4()
                .child(
                    div()
                        .mt_5()
                        .p_4()
                        .rounded(theme::RADIUS_MEDIUM)
                        .border_1()
                        .border_color(theme::border())
                        .bg(theme::canvas())
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_3()
                                .child(
                                    Icon::new(IconName::File)
                                        .size(px(26.0))
                                        .text_color(theme::blue()),
                                )
                                .child(div().flex_1().text_sm().child(
                                    if app.upload_sources.is_empty() {
                                        app.tr("upload-no-file-selected")
                                    } else {
                                        source_summary
                                    },
                                ))
                                .child(
                                    components::button(
                                        "upload-choose",
                                        app.tr(if app.upload_sources.is_empty() {
                                            "upload-choose-file"
                                        } else {
                                            "upload-change-file"
                                        }),
                                        Some(IconName::FolderOpen),
                                        false,
                                    )
                                    .disabled(app.upload_preparing)
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.choose_upload_file(cx)),
                                    ),
                                ),
                        )
                        .child(
                            div()
                                .mt_2()
                                .text_xs()
                                .text_color(theme::text_muted())
                                .child(app.tr("upload-batch-limit")),
                        )
                        .when(!app.upload_sources.is_empty(), |area| {
                            area.child(
                                div()
                                    .id("upload-selected-files")
                                    .max_h(px(168.0))
                                    .overflow_y_scroll()
                                    .mt_3()
                                    .children(app.upload_sources.iter().enumerate().map(
                                        |(index, source)| {
                                            let tooltip =
                                                source.path.to_string_lossy().into_owned();
                                            components::list_row()
                                        .id(("upload-source-row", index))
                                        .debug_selector(move || format!("upload-source-row-{index}"))
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .border_t_1()
                                        .border_color(theme::border())
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .flex()
                                                .items_center()
                                                .gap_2()
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .text_size(theme::LIST_TEXT_SIZE)
                                                        .truncate()
                                                        .debug_selector(move || format!("upload-source-name-{index}"))
                                                        .child(source.file_name.clone()),
                                                )
                                                .child(
                                                    div()
                                                        .flex_none()
                                                        .text_size(theme::LIST_SECONDARY_TEXT_SIZE)
                                                        .text_color(theme::text_muted())
                                                        .child(teleark_i18n::format::format_bytes(
                                                            app.locale(),
                                                            source.size_bytes,
                                                        )),
                                                ),
                                        )
                                        .child(
                                            components::list_icon_button(
                                                ("upload-remove-source", index),
                                                IconName::Close,
                                                app.tr("upload-remove-file"),
                                            )
                                            .ghost()
                                            .tooltip(tooltip)
                                            .debug_selector(move || format!("upload-source-action-{index}"))
                                            .on_click(
                                                cx.listener(move |this, _, _, cx| {
                                                    if index < this.upload_sources.len() {
                                                        this.upload_sources.remove(index);
                                                    }
                                                    cx.notify();
                                                }),
                                            ),
                                        )
                                        },
                                    )),
                            )
                        }),
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
                    super::settings::vault_activity_message(app).filter(|_| !folder_rejected),
                    |popup, (message, tone)| {
                        popup.child(
                            div()
                                .mt_3()
                                .text_sm()
                                .text_color(tone.foreground())
                                .child(message),
                        )
                    },
                ),
        ))
        .when(app.upload_in_flight, |popup| {
            popup.child(
                div()
                    .px_6()
                    .py_2()
                    .text_xs()
                    .child(app.tr("upload-batch-still-running")),
            )
        })
        .child(
            div()
                .flex_none()
                .px_6()
                .py_4()
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
                        app.tr(if app.vault_locked || app.vault_status.active_key_locked {
                            "vault-unlock-action"
                        } else {
                            "upload-add-to-queue"
                        }),
                        Some(IconName::ArrowUp),
                        true,
                    )
                    .debug_selector(|| "upload-add-queue".into())
                    .disabled(
                        app.visual_preview
                            || app.upload_in_flight
                            || app.upload_sources.is_empty()
                            || app.upload_preparing
                            || app.vault_activity == VaultActivity::Working
                            || app.storage_channel_id().is_none(),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        if this.vault_locked || this.vault_status.active_key_locked {
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
