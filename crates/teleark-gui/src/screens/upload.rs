use gpui::{
    AnyElement, Context, FontWeight, IntoElement, ParentElement as _, SharedString, Styled as _,
    div, prelude::FluentBuilder as _, px, rgba,
};
use gpui_component::{Disableable as _, Icon, IconName, scroll::ScrollableElement as _};
use teleark_i18n::format::format_bytes;
use teleark_runtime::encrypted_part_plaintext_limit;

use crate::{app::TeleArkApp, components, layout::LayoutPolicy, theme};

pub fn render_upload_overlay(
    app: &TeleArkApp,
    layout: LayoutPolicy,
    cx: &mut Context<TeleArkApp>,
) -> AnyElement {
    let part_size_label = format_bytes(app.locale(), encrypted_part_plaintext_limit());
    let source_name = app
        .upload_source
        .as_ref()
        .and_then(|path| path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| app.tr("upload-no-file-selected").to_string());
    let source_path = app
        .upload_source
        .as_ref()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|| app.tr("upload-select-file-description").to_string());
    let file_preview = div()
        .w(px(layout.upload_preview_width()))
        .h_full()
        .flex_none()
        .p(px(if layout.is_compact() { 16.0 } else { 20.0 }))
        .flex()
        .flex_col()
        .items_center()
        .bg(theme::sidebar())
        .border_r_1()
        .border_color(theme::border())
        .child(
            div()
                .mt_4()
                .size(px(76.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(theme::RADIUS_LARGE)
                .bg(theme::blue_soft())
                .text_color(theme::blue())
                .text_2xl()
                .font_weight(FontWeight::BOLD)
                .child("▶"),
        )
        .child(
            div()
                .mt_4()
                .max_w_full()
                .truncate()
                .font_weight(FontWeight::SEMIBOLD)
                .child(source_name),
        )
        .child(
            div()
                .mt_2()
                .text_sm()
                .text_color(theme::text_secondary())
                .child(app.tr("transfer-value-unavailable")),
        )
        .child(
            div()
                .mt_5()
                .max_w_full()
                .text_center()
                .text_xs()
                .text_color(theme::text_muted())
                .child(source_path),
        )
        .child(
            components::button(
                "upload-change-file",
                app.tr("upload-change-file"),
                Some(IconName::FolderOpen),
                false,
            )
            .mt_5()
            .w_full()
            .on_click(cx.listener(|this, _, _, cx| this.choose_upload_file(cx))),
        )
        .child(div().flex_1())
        .child(
            div()
                .w_full()
                .p_3()
                .rounded(theme::RADIUS_MEDIUM)
                .bg(theme::blue_pale())
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme::blue())
                        .child(app.tr("upload-source-unchanged")),
                )
                .child(
                    div()
                        .mt_1()
                        .text_xs()
                        .text_color(theme::text_muted())
                        .child(app.tr("upload-source-checked")),
                ),
        );

    let form_body = div()
        .flex_1()
        .min_h_0()
        .p_5()
        .flex()
        .flex_col()
        .overflow_y_scrollbar()
        .child(form_label(app.tr("upload-target-account")))
        .child(select_field(
            app.telegram_account
                .as_ref()
                .map(|account| account.display_name.clone())
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| app.tr("transfer-value-unavailable").to_string()),
            app.tr("account-standard"),
        ))
        .child(form_label(app.tr("upload-target-channel")).mt_4())
        .child(select_field(
            app.tr("upload-target-saved-messages"),
            app.tr("channel-private"),
        ))
        .child(form_label(app.tr("upload-storage-method")).mt_5())
        .child(check_row(
            app.tr("upload-automatic-multipart"),
            app.tr("upload-automatic-multipart-description"),
            true,
        ))
        .child(
            div()
                .mt_4()
                .flex()
                .gap_5()
                .child(
                    div()
                        .flex_1()
                        .child(form_label(app.tr("upload-part-size")))
                        .child(check_row(
                            app.tr("upload-current-part-size"),
                            app.tr_with(
                                "upload-current-part-size-description",
                                teleark_i18n::MessageArgs::new()
                                    .with("size", part_size_label.clone()),
                            ),
                            true,
                        )),
                )
                .child(
                    div()
                        .w(px(190.0))
                        .p_4()
                        .rounded(theme::RADIUS_MEDIUM)
                        .bg(theme::sidebar())
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(app.tr("upload-estimate-title")),
                        )
                        .child(estimate_row(
                            app.tr("upload-estimate-parts"),
                            app.tr("transfer-value-unavailable"),
                        ))
                        .child(estimate_row(
                            app.tr("upload-estimate-part-size"),
                            part_size_label,
                        ))
                        .child(estimate_row(
                            app.tr("upload-estimate-total-size"),
                            app.tr("transfer-value-unavailable"),
                        ))
                        .child(estimate_row(
                            app.tr("upload-estimate-messages"),
                            app.tr("transfer-value-unavailable"),
                        ))
                        .child(estimate_row(
                            app.tr("upload-estimate-time"),
                            app.tr("transfer-value-unavailable"),
                        )),
                ),
        )
        .child(
            div()
                .mt_5()
                .pt_4()
                .border_t_1()
                .border_color(theme::border())
                .child(form_label(app.tr("upload-security")))
                .child(
                    div()
                        .mt_2()
                        .text_xs()
                        .text_color(theme::text_secondary())
                        .child(app.tr("upload-saved-messages-security-note")),
                )
                .child(check_row(
                    app.tr("upload-client-encryption"),
                    app.tr("upload-client-encryption-description"),
                    true,
                ))
                .child(
                    div()
                        .mt_3()
                        .h(px(38.0))
                        .px_3()
                        .flex()
                        .items_center()
                        .rounded(theme::RADIUS_SMALL)
                        .border_1()
                        .border_color(theme::border())
                        .child(
                            div()
                                .flex_1()
                                .text_sm()
                                .child(app.tr("upload-encryption-profile")),
                        )
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .child("AES-256-GCM"),
                        )
                        .child(Icon::new(IconName::ChevronDown).text_color(theme::text_muted())),
                )
                .child(check_row(
                    app.tr("upload-hide-filename"),
                    app.tr("upload-hide-filename-description"),
                    true,
                ))
                .child(check_row(
                    app.tr("upload-encrypt-metadata"),
                    app.tr("upload-encrypt-metadata-description"),
                    true,
                )),
        );

    let form_footer = div()
        .min_h(px(58.0))
        .px_5()
        .py_2()
        .flex_none()
        .flex()
        .flex_wrap()
        .items_center()
        .justify_end()
        .gap_2()
        .border_t_1()
        .border_color(theme::border())
        .child(
            components::button("upload-cancel", app.tr("action-cancel"), None, false).on_click(
                cx.listener(|this, _, _, cx| {
                    this.show_upload = false;
                    cx.notify();
                }),
            ),
        )
        .child(
            components::button(
                "upload-add-queue",
                app.tr("upload-add-to-queue"),
                Some(IconName::ArrowUp),
                true,
            )
            .disabled(
                app.upload_source.is_none()
                    || app.vault_status.locked
                    || app.vault_activity == crate::app::VaultActivity::Working,
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.enqueue_vault_upload(cx);
            })),
        );

    let form = div()
        .flex_1()
        .min_w_0()
        .h_full()
        .flex()
        .flex_col()
        .child(form_body)
        .child(form_footer);

    let dialog = div()
        .w(px(layout.upload_dialog_width()))
        .h(px(layout.upload_dialog_height()))
        .rounded(theme::RADIUS_LARGE)
        .border_1()
        .border_color(theme::border())
        .bg(theme::surface())
        .shadow_xl()
        .overflow_hidden()
        .flex()
        .flex_col()
        .child(
            div()
                .h(px(52.0))
                .px_5()
                .flex()
                .items_center()
                .justify_center()
                .border_b_1()
                .border_color(theme::border())
                .font_weight(FontWeight::SEMIBOLD)
                .child(app.tr("upload-dialog-title")),
        )
        .child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .child(file_preview)
                .child(form),
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

fn form_label(label: impl Into<SharedString>) -> gpui::Div {
    div()
        .text_sm()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme::text_primary())
        .child(label.into())
}

fn select_field(value: impl Into<SharedString>, hint: SharedString) -> AnyElement {
    div()
        .mt_2()
        .h(px(38.0))
        .px_3()
        .flex()
        .items_center()
        .rounded(theme::RADIUS_SMALL)
        .border_1()
        .border_color(theme::border())
        .child(
            div()
                .size(px(22.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(theme::RADIUS_SMALL)
                .bg(theme::blue_soft())
                .child(Icon::new(IconName::CircleUser).text_color(theme::blue())),
        )
        .child(
            div()
                .ml_2()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_sm()
                .child(value.into()),
        )
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_xs()
                .text_color(theme::text_muted())
                .child(hint),
        )
        .child(Icon::new(IconName::ChevronDown).text_color(theme::text_muted()))
        .into_any_element()
}

fn check_row(title: SharedString, description: SharedString, checked: bool) -> AnyElement {
    div()
        .mt_3()
        .flex()
        .items_start()
        .gap_2()
        .child(
            div()
                .mt(px(1.0))
                .size(px(16.0))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(3.0))
                .border_1()
                .border_color(if checked {
                    theme::blue()
                } else {
                    theme::border()
                })
                .bg(if checked {
                    theme::blue()
                } else {
                    theme::surface()
                })
                .text_color(theme::surface())
                .text_xs()
                .when(checked, |box_| box_.child("✓")),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(div().text_sm().child(title))
                .child(
                    div()
                        .mt_1()
                        .text_xs()
                        .text_color(theme::text_muted())
                        .child(description),
                ),
        )
        .into_any_element()
}

fn estimate_row(label: SharedString, value: impl Into<SharedString>) -> AnyElement {
    div()
        .mt_3()
        .flex()
        .text_xs()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(theme::text_muted())
                .child(label),
        )
        .child(
            div()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::text_secondary())
                .child(value.into()),
        )
        .into_any_element()
}
