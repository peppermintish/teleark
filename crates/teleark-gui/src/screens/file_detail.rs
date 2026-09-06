use gpui_kit::component::{Icon, IconName, scroll::ScrollableElement as _};
use gpui_kit::{
    AnyElement, Context, FontWeight, IntoElement, ParentElement as _, SharedString, Styled as _,
    Window, div, prelude::FluentBuilder as _, px,
};
use teleark_core::{EncryptionState, RemoteState, VerificationState};
use teleark_i18n::{
    MessageArgs,
    format::{format_bytes, format_integer, format_unix_millis},
};

use crate::{
    app::{Page, TeleArkApp},
    components::{self, Tone},
    layout::LayoutPolicy,
    library_state::{LibraryContent, LibraryRowView},
    screens::library::{application_error_message_id, file_kind_message_id, file_status},
    theme,
};

impl TeleArkApp {
    pub(crate) fn render_file_detail(
        &self,
        _window: &mut Window,
        layout: LayoutPolicy,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(file) = self.library_content.selected(self.selected_file).cloned() else {
            return self.render_missing_file_detail(layout, cx);
        };
        self.render_real_file_detail(file, layout, cx)
    }

    fn render_missing_file_detail(
        &self,
        layout: LayoutPolicy,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (icon, title, description) = match &self.library_content {
            LibraryContent::Loading => (
                IconName::LoaderCircle,
                "library-loading-title",
                "library-loading-description",
            ),
            LibraryContent::Failed(kind) => (
                IconName::TriangleAlert,
                "library-error-title",
                application_error_message_id(*kind),
            ),
            LibraryContent::Empty(_) | LibraryContent::Ready(_) => (
                IconName::FolderOpen,
                "file-detail-empty-title",
                "file-detail-empty-description",
            ),
        };
        let padding = layout.content_padding();

        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .bg(theme::canvas())
            .child(
                div()
                    .min_h(px(58.0))
                    .px(px(padding))
                    .py_2()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        components::icon_button(
                            "file-detail-back",
                            IconName::ArrowLeft,
                            self.tr("action-back"),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.set_page(Page::Library, cx);
                        })),
                    )
                    .child(components::section_title(self.tr("file-detail-title"))),
            )
            .child(
                components::card()
                    .m(px(padding))
                    .flex_1()
                    .min_h(px(220.0))
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .p_6()
                    .text_center()
                    .child(Icon::new(icon).text_color(theme::blue()))
                    .child(
                        div()
                            .mt_4()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(self.tr(title)),
                    )
                    .child(
                        div()
                            .mt_2()
                            .max_w(px(440.0))
                            .text_sm()
                            .text_color(theme::text_secondary())
                            .child(self.tr(description)),
                    ),
            )
            .into_any_element()
    }

    fn render_real_file_detail(
        &self,
        file: LibraryRowView,
        layout: LayoutPolicy,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let padding = layout.content_padding();
        let (state_id, state_tone) = file_status(file.remote_state, file.verification_state);
        let not_applicable = self.tr("common-not-applicable");
        let source = source_label(self, &file);
        let modified = file.modified_at_unix_ms.map_or_else(
            || not_applicable.clone(),
            |timestamp| SharedString::from(format_unix_millis(self.locale(), timestamp)),
        );
        let package = file.package_id.map_or_else(
            || not_applicable.clone(),
            |id| SharedString::from(id.to_string()),
        );
        let encryption = self.tr(encryption_message_id(file.encryption_state));
        let verified = file.verification_state == VerificationState::Verified;
        let local_path_display = file.local_source_path.as_ref().map_or_else(
            || not_applicable.clone(),
            |path| SharedString::from(path.to_string_lossy().into_owned()),
        );

        let toolbar = components::page_toolbar(padding)
            .child(
                components::icon_button(
                    "file-detail-back",
                    IconName::ArrowLeft,
                    self.tr("action-back"),
                )
                .on_click(cx.listener(|this, _, _, cx| this.set_page(Page::Library, cx))),
            )
            .child(components::section_title(self.tr("file-detail-title")));

        let hero = components::card()
            .mx(px(padding))
            .p(px(padding))
            .flex()
            .flex_wrap()
            .items_center()
            .gap_5()
            .child(
                div()
                    .size(px(64.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(theme::RADIUS_LARGE)
                    .bg(theme::blue_soft())
                    .child(Icon::new(IconName::File).text_color(theme::blue())),
            )
            .child(
                div()
                    .min_w(px(180.0))
                    .flex_1()
                    .child(
                        div()
                            .truncate()
                            .text_xl()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(file.name.clone()),
                    )
                    .child(
                        div()
                            .mt_2()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap_2()
                            .child(components::badge(self.tr(state_id), state_tone))
                            .when(
                                file.encryption_state != EncryptionState::Unencrypted,
                                |badges| {
                                    badges.child(components::badge(
                                        self.tr(encryption_message_id(file.encryption_state)),
                                        Tone::Blue,
                                    ))
                                },
                            ),
                    ),
            )
            .when_some(file.local_source_path.clone(), |hero, path| {
                let open_path = path.clone();
                hero.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .child(
                            components::button(
                                "file-detail-open",
                                self.tr("action-open-file"),
                                Some(IconName::File),
                                true,
                            )
                            .on_click(cx.listener(
                                move |_, _, _, cx| {
                                    cx.open_with_system(&open_path);
                                },
                            )),
                        )
                        .child(
                            components::button(
                                "file-detail-reveal",
                                self.tr("action-open-location"),
                                Some(IconName::FolderOpen),
                                false,
                            )
                            .on_click(cx.listener(
                                move |_, _, _, cx| {
                                    cx.reveal_path(&path);
                                },
                            )),
                        ),
                )
            })
            .child(hero_metric(
                self.tr("table-size"),
                format_bytes(self.locale(), file.size_bytes),
            ))
            .child(hero_metric(
                self.tr("table-parts"),
                format_integer(self.locale(), u64::from(file.part_count)),
            ));

        let properties = components::card()
            .w(px(layout.properties_width()))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(card_header(self.tr("file-detail-overview")))
            .child(div().flex_1().min_h_0().overflow_y_scrollbar().child(
                div().p_5().flex().flex_col().gap_4().children([
                    property_row(self.tr("table-name"), file.name.clone()),
                    property_row(
                        self.tr("table-size"),
                        format_bytes(self.locale(), file.size_bytes),
                    ),
                    property_row(
                        self.tr("table-type"),
                        self.tr(file_kind_message_id(file.kind)),
                    ),
                    property_row(self.tr("table-source"), source),
                    property_row(self.tr("file-detail-local-path"), local_path_display),
                    property_row(self.tr("file-detail-modified-at"), modified),
                    property_row(
                        self.tr("table-parts"),
                        format_integer(self.locale(), u64::from(file.part_count)),
                    ),
                    property_row(self.tr("file-detail-encryption"), encryption),
                    property_row(self.tr("file-detail-package-id"), package),
                    property_row(self.tr("file-detail-hash"), not_applicable.clone()),
                ]),
            ));

        let part_count = file.part_count.max(1);
        let parts = components::card()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(48.0))
                    .px_4()
                    .flex()
                    .items_end()
                    .gap_6()
                    .border_b_1()
                    .border_color(theme::border())
                    .child(tab(self.tr("file-detail-tab-parts"), true))
                    .child(tab(self.tr("file-detail-tab-details"), false))
                    .child(tab(self.tr("file-detail-tab-activity"), false)),
            )
            .child(parts_header(self, layout))
            .child(div().flex_1().min_h_0().overflow_y_scrollbar().children(
                part_indices(part_count).into_iter().map(|index| {
                    if index == u32::MAX {
                        part_ellipsis()
                    } else {
                        part_row(self, index, part_count, &file, layout)
                    }
                }),
            ))
            .child(
                div()
                    .min_h(px(46.0))
                    .px_4()
                    .py_2()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_3()
                    .border_t_1()
                    .border_color(theme::border())
                    .child(
                        Icon::new(if verified {
                            IconName::CircleCheck
                        } else {
                            IconName::Info
                        })
                        .text_color(if verified {
                            theme::green()
                        } else {
                            theme::text_secondary()
                        }),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(if verified {
                                theme::green()
                            } else {
                                theme::text_secondary()
                            })
                            .child(if verified {
                                self.tr("file-detail-all-parts-verified")
                            } else {
                                self.tr("file-detail-verification-unavailable")
                            }),
                    ),
            );

        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .bg(theme::canvas())
            .child(toolbar)
            .child(hero)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .p(px(padding))
                    .flex()
                    .gap_4()
                    .child(properties)
                    .child(parts),
            )
            .into_any_element()
    }
}

fn source_label(app: &TeleArkApp, file: &LibraryRowView) -> SharedString {
    file.source_name.clone().map_or_else(
        || {
            file.source_chat_id.map_or_else(
                || app.tr("library-source-local"),
                |chat_id| {
                    app.tr_with(
                        "library-source-telegram-chat",
                        MessageArgs::new().with("chat_id", chat_id),
                    )
                },
            )
        },
        SharedString::from,
    )
}

fn encryption_message_id(state: EncryptionState) -> &'static str {
    match state {
        EncryptionState::Unencrypted => "file-detail-not-encrypted",
        EncryptionState::Encrypted => "file-state-encrypted",
        EncryptionState::Locked => "file-state-locked",
        _ => "common-not-applicable",
    }
}

fn hero_metric(label: impl Into<SharedString>, value: impl Into<SharedString>) -> AnyElement {
    div()
        .min_w(px(110.0))
        .pl_5()
        .border_l_1()
        .border_color(theme::border())
        .child(
            div()
                .text_xs()
                .text_color(theme::text_muted())
                .child(label.into()),
        )
        .child(
            div()
                .mt_2()
                .font_weight(FontWeight::SEMIBOLD)
                .child(value.into()),
        )
        .into_any_element()
}

fn card_header(title: impl Into<SharedString>) -> AnyElement {
    div()
        .h(px(48.0))
        .px_4()
        .flex()
        .items_center()
        .border_b_1()
        .border_color(theme::border())
        .font_weight(FontWeight::SEMIBOLD)
        .child(title.into())
        .into_any_element()
}

fn property_row(label: impl Into<SharedString>, value: impl Into<SharedString>) -> AnyElement {
    div()
        .flex()
        .text_sm()
        .child(
            div()
                .w(px(116.0))
                .flex_none()
                .text_color(theme::text_muted())
                .child(label.into()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(theme::text_secondary())
                .child(value.into()),
        )
        .into_any_element()
}

fn tab(label: impl Into<SharedString>, selected: bool) -> AnyElement {
    div()
        .h_full()
        .pb_3()
        .flex()
        .items_end()
        .text_sm()
        .text_color(if selected {
            theme::blue()
        } else {
            theme::text_secondary()
        })
        .when(selected, |tab| tab.border_b_2().border_color(theme::blue()))
        .child(label.into())
        .into_any_element()
}

fn parts_header(app: &TeleArkApp, layout: LayoutPolicy) -> AnyElement {
    div()
        .h(px(36.0))
        .px_4()
        .grid()
        .grid_cols(layout.file_detail_part_columns())
        .items_center()
        .bg(theme::sidebar())
        .border_b_1()
        .border_color(theme::border())
        .text_xs()
        .text_color(theme::text_muted())
        .child(app.tr("file-detail-part-index"))
        .child(app.tr("table-size"))
        .child(app.tr("table-status"))
        .when(!layout.is_compact(), |header| {
            header
                .child(app.tr("file-detail-remote-id"))
                .child(app.tr("file-detail-upload-time"))
        })
        .into_any_element()
}

fn part_indices(total: u32) -> Vec<u32> {
    if total <= 9 {
        return (1..=total).collect();
    }
    let mut indices: Vec<_> = (1..=6).collect();
    indices.push(u32::MAX);
    indices.extend(total.saturating_sub(2)..=total);
    indices
}

fn part_row(
    app: &TeleArkApp,
    index: u32,
    total: u32,
    file: &LibraryRowView,
    layout: LayoutPolicy,
) -> AnyElement {
    let size = format_bytes(
        app.locale(),
        logical_part_size(file.size_bytes, total, index),
    );
    let (state_id, tone) = file_status(file.remote_state, file.verification_state);
    div()
        .h(px(40.0))
        .px_4()
        .grid()
        .grid_cols(layout.file_detail_part_columns())
        .items_center()
        .border_b_1()
        .border_color(theme::border_subtle())
        .text_xs()
        .text_color(theme::text_secondary())
        .child(format!("{index:03}"))
        .child(size)
        .child(
            div()
                .flex()
                .min_w_0()
                .items_center()
                .gap_2()
                .text_color(tone.foreground())
                .child(
                    Icon::new(status_icon(file))
                        .flex_none()
                        .text_color(tone.foreground()),
                )
                .child(div().min_w_0().truncate().child(app.tr(state_id))),
        )
        .when(!layout.is_compact(), |row| {
            row.child(app.tr("common-not-applicable"))
                .child(app.tr("common-not-applicable"))
        })
        .into_any_element()
}

fn status_icon(file: &LibraryRowView) -> IconName {
    if file.verification_state == VerificationState::Verified {
        IconName::Check
    } else if matches!(
        file.remote_state,
        RemoteState::Uploading | RemoteState::Uploaded
    ) {
        IconName::LoaderCircle
    } else {
        IconName::Info
    }
}

fn logical_part_size(total_size_bytes: u64, total_parts: u32, index: u32) -> u64 {
    if total_parts == 0 || index == 0 || index > total_parts {
        return 0;
    }
    let total_parts = u64::from(total_parts);
    let base_size = total_size_bytes / total_parts;
    let remainder = total_size_bytes % total_parts;
    base_size + u64::from(u64::from(index) <= remainder)
}

fn part_ellipsis() -> AnyElement {
    div()
        .h(px(38.0))
        .flex()
        .items_center()
        .justify_center()
        .border_b_1()
        .border_color(theme::border_subtle())
        .text_color(theme::text_muted())
        .child("•••")
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn displayed_part_sizes_preserve_each_logical_file_size() {
        for (size, parts) in [(73_600_000_000, 39), (42, 1), (7, 3)] {
            let total: u64 = (1..=parts)
                .map(|index| logical_part_size(size, parts, index))
                .sum();

            assert_eq!(total, size);
        }
    }

    #[test]
    fn invalid_part_indices_have_no_displayed_bytes() {
        assert_eq!(logical_part_size(10, 0, 1), 0);
        assert_eq!(logical_part_size(10, 2, 0), 0);
        assert_eq!(logical_part_size(10, 2, 3), 0);
    }
}
