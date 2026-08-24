use gpui::{
    AnyElement, Context, FontWeight, IntoElement, ParentElement as _, SharedString, Styled as _,
    div, prelude::FluentBuilder as _, px, rgba,
};
use gpui_component::{Icon, IconName, scroll::ScrollableElement as _};

use crate::{
    app::{Page, TeleArkApp},
    components, theme,
};

pub fn render_upload_overlay(app: &TeleArkApp, cx: &mut Context<TeleArkApp>) -> AnyElement {
    let file_preview = div()
        .w(px(210.0))
        .h_full()
        .flex_none()
        .p_5()
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
                .child("Movie_Archive.mkv"),
        )
        .child(
            div()
                .mt_2()
                .text_sm()
                .text_color(theme::text_secondary())
                .child("73.6 GB"),
        )
        .child(
            div()
                .mt_5()
                .max_w_full()
                .text_center()
                .text_xs()
                .text_color(theme::text_muted())
                .child("/Users/maxmeng/Movies/Movie_Archive.mkv"),
        )
        .child(
            components::button(
                "upload-change-file",
                app.tr("upload-change-file"),
                Some(IconName::FolderOpen),
                false,
            )
            .mt_5()
            .w_full(),
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
        .child(select_field("maxmeng", app.tr("account-standard")))
        .child(form_label(app.tr("upload-target-channel")).mt_4())
        .child(select_field("My Storage", app.tr("channel-private")))
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
                        .child(radio_row(
                            "1900 MiB",
                            app.tr("upload-compatibility-mode"),
                            true,
                        ))
                        .child(radio_row(
                            "1024 MiB",
                            app.tr("upload-conservative-mode"),
                            false,
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
                        .child(estimate_row(app.tr("upload-estimate-parts"), "39"))
                        .child(estimate_row(
                            app.tr("upload-estimate-part-size"),
                            "1900 MiB",
                        ))
                        .child(estimate_row(
                            app.tr("upload-estimate-total-size"),
                            "73.6 GB",
                        ))
                        .child(estimate_row(app.tr("upload-estimate-messages"), "40"))
                        .child(estimate_row(app.tr("upload-estimate-time"), "≈ 48m")),
                ),
        )
        .child(
            div()
                .mt_5()
                .pt_4()
                .border_t_1()
                .border_color(theme::border())
                .child(form_label(app.tr("upload-security")))
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
        .h(px(58.0))
        .px_5()
        .flex_none()
        .flex()
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
            .on_click(cx.listener(|this, _, _, cx| {
                this.show_upload = false;
                this.upload_queued = true;
                this.nav_selection = "nav-transfers-all";
                this.selected_file = 0;
                this.set_page(Page::Transfers, cx);
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
        .w(px(820.0))
        .h(px(660.0))
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

fn select_field(value: &'static str, hint: SharedString) -> AnyElement {
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
        .child(div().ml_2().flex_1().text_sm().child(value))
        .child(div().text_xs().text_color(theme::text_muted()).child(hint))
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
            div().flex_1().child(div().text_sm().child(title)).child(
                div()
                    .mt_1()
                    .text_xs()
                    .text_color(theme::text_muted())
                    .child(description),
            ),
        )
        .into_any_element()
}

fn radio_row(title: &'static str, hint: SharedString, selected: bool) -> AnyElement {
    div()
        .mt_3()
        .flex()
        .items_center()
        .gap_2()
        .child(
            div()
                .size(px(15.0))
                .p(px(3.0))
                .rounded_full()
                .border_1()
                .border_color(if selected {
                    theme::blue()
                } else {
                    theme::border()
                })
                .when(selected, |radio| {
                    radio.child(div().size_full().rounded_full().bg(theme::blue()))
                }),
        )
        .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(title))
        .child(div().text_xs().text_color(theme::text_muted()).child(hint))
        .into_any_element()
}

fn estimate_row(label: SharedString, value: &'static str) -> AnyElement {
    div()
        .mt_3()
        .flex()
        .text_xs()
        .child(div().flex_1().text_color(theme::text_muted()).child(label))
        .child(
            div()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::text_secondary())
                .child(value),
        )
        .into_any_element()
}
