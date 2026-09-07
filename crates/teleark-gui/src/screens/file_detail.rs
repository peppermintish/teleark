use gpui_kit::component::{Disableable as _, Icon, IconName, scroll::ScrollableElement as _};
use gpui_kit::{
    AnyElement, Context, FontWeight, IntoElement, ParentElement as _, SharedString, Styled as _,
    Window, div, prelude::FluentBuilder as _, px,
};
use teleark_core::EncryptionState;
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
            .when(file.local_source_path.is_none(), |hero| {
                let source = file.download_source(self.telegram_account.as_ref().map(|a| a.id));
                let download_file = file.clone();
                hero.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            components::button(
                                "library-download",
                                self.tr("action-download"),
                                Some(IconName::ArrowDown),
                                true,
                            )
                            .disabled(
                                source.is_err()
                                    || self.library_action_busy
                                    || self.transfers.is_none(),
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.download_library_file(download_file.clone(), cx)
                                },
                            )),
                        )
                        .when_some(source.err(), |body, reason| {
                            body.child(div().max_w(px(240.0)).text_sm().child(self.tr(reason)))
                        })
                        .when_some(self.library_action_error, |body, error| {
                            body.child(
                                div()
                                    .max_w(px(240.0))
                                    .text_sm()
                                    .text_color(theme::red())
                                    .child(self.tr(application_error_message_id(error))),
                            )
                        }),
                )
            })
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

        let provenance =
            components::card()
                .flex_1()
                .min_w_0()
                .h_full()
                .overflow_hidden()
                .flex()
                .flex_col()
                .child(card_header(self.tr("file-detail-source-record")))
                .child(
                    div().flex_1().min_h_0().overflow_y_scrollbar().child(
                        div()
                            .p_5()
                            .flex()
                            .flex_col()
                            .gap_4()
                            .child(div().text_sm().text_color(theme::text_secondary()).child(
                                self.tr(
                                    if file.local_source_path.is_some()
                                        && file.source_account_id.is_some()
                                    {
                                        "file-detail-downloaded-source-note"
                                    } else if file.source_account_id.is_some() {
                                        "file-detail-indexed-source-note"
                                    } else {
                                        "file-detail-local-source-note"
                                    },
                                ),
                            ))
                            .when(file.source_account_id.is_some(), |body| {
                                body.children([
                                    source_id_row(
                                        self.tr("file-detail-source-account-id"),
                                        file.source_account_id
                                            .map(|id| id.to_string())
                                            .unwrap_or_else(|| not_applicable.to_string()),
                                    ),
                                    source_id_row(
                                        self.tr("file-detail-source-chat-id"),
                                        file.source_chat_id
                                            .map(|id| id.to_string())
                                            .unwrap_or_else(|| not_applicable.to_string()),
                                    ),
                                    source_id_row(
                                        self.tr("file-detail-remote-id"),
                                        file.source_message_id
                                            .map(|id| id.to_string())
                                            .unwrap_or_else(|| not_applicable.to_string()),
                                    ),
                                ])
                            })
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme::text_secondary())
                                    .child(self.tr("file-detail-verification-unavailable")),
                            ),
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
                    .child(provenance),
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

fn source_id_row(label: impl Into<SharedString>, value: impl Into<SharedString>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .text_sm()
        .child(div().text_color(theme::text_muted()).child(label.into()))
        .child(div().text_color(theme::text_primary()).child(value.into()))
        .into_any_element()
}
