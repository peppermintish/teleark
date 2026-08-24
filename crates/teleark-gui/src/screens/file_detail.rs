use gpui::{
    AnyElement, Context, FontWeight, IntoElement, ParentElement as _, SharedString, Styled as _,
    Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{Icon, IconName, scroll::ScrollableElement as _};
use teleark_i18n::format::format_bytes;

use crate::{
    app::{Page, TeleArkApp},
    components::{self, Tone},
    mock::{FileState, library_files},
    theme,
};

impl TeleArkApp {
    pub(crate) fn render_file_detail(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let files = library_files();
        let file = files
            .get(self.selected_file.min(files.len().saturating_sub(1)))
            .or_else(|| files.first());
        let Some(file) = file else {
            return div().into_any_element();
        };
        let parts_verified = matches!(file.state, FileState::Remote | FileState::Downloaded);
        let state_tone = if parts_verified {
            Tone::Green
        } else {
            Tone::Blue
        };
        let not_applicable = self.tr("common-not-applicable");
        let encryption = if file.encrypted {
            SharedString::from("AES-256-GCM")
        } else {
            self.tr("file-detail-not-encrypted")
        };
        let package_value = if file.encrypted {
            SharedString::from("7F2A9C38…B914")
        } else {
            not_applicable.clone()
        };

        let toolbar = div()
            .h(px(58.0))
            .px_5()
            .flex()
            .items_center()
            .gap_3()
            .child(
                components::icon_button(
                    "file-detail-back",
                    IconName::ArrowLeft,
                    self.tr("action-back"),
                )
                .on_click(cx.listener(|this, _, _, cx| this.set_page(Page::Library, cx))),
            )
            .child(components::section_title(self.tr("file-detail-title")))
            .child(div().flex_1())
            .child(
                components::button(
                    "file-detail-download",
                    self.tr("action-download"),
                    Some(IconName::ArrowDown),
                    true,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.nav_selection = "nav-transfers-all";
                    this.selected_file = 0;
                    this.set_page(Page::Transfers, cx);
                })),
            )
            .child(components::button(
                "file-detail-open",
                self.tr("action-open-file"),
                Some(IconName::FolderOpen),
                false,
            ))
            .child(components::icon_button(
                "file-detail-more",
                IconName::Ellipsis,
                self.tr("action-more"),
            ));

        let hero = components::card()
            .mx_5()
            .p_5()
            .flex()
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
                    .text_color(theme::blue())
                    .text_2xl()
                    .font_weight(FontWeight::BOLD)
                    .child(file.kind.glyph()),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .child(
                        div()
                            .truncate()
                            .text_xl()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(file.name),
                    )
                    .child(
                        div()
                            .mt_2()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(components::badge(
                                self.tr(file.state.message_id()),
                                state_tone,
                            ))
                            .when(file.encrypted, |badges| {
                                badges.child(components::badge(
                                    self.tr("file-state-encrypted"),
                                    Tone::Blue,
                                ))
                            }),
                    ),
            )
            .child(hero_metric(self.tr("table-size"), file.size))
            .child(hero_metric(self.tr("table-parts"), file.parts.to_string()))
            .child(hero_metric(self.tr("file-detail-hash"), "BLAKE3"));

        let properties = components::card()
            .w(px(330.0))
            .h_full()
            .flex_none()
            .overflow_hidden()
            .child(card_header(self.tr("file-detail-overview")))
            .child(div().p_5().flex().flex_col().gap_4().children([
                property_row(self.tr("table-name"), file.name),
                property_row(self.tr("table-size"), file.size),
                property_row(self.tr("table-type"), self.tr(file.kind.message_id())),
                property_row(self.tr("table-source"), file.source),
                property_row(self.tr("file-detail-uploaded-at"), file.modified),
                property_row(
                    self.tr("file-detail-part-size"),
                    if file.encrypted {
                        SharedString::from("≤ 1900 MiB")
                    } else {
                        not_applicable.clone()
                    },
                ),
                property_row(self.tr("table-parts"), file.parts.to_string()),
                property_row(self.tr("file-detail-encryption"), encryption),
                property_row(
                    self.tr("file-detail-frame-size"),
                    if file.encrypted {
                        SharedString::from("8 MiB")
                    } else {
                        not_applicable.clone()
                    },
                ),
                property_row(
                    self.tr("file-detail-manifest"),
                    if file.encrypted {
                        SharedString::from("TeleArk v1")
                    } else {
                        not_applicable.clone()
                    },
                ),
                property_row(self.tr("file-detail-package-id"), package_value),
                property_row(self.tr("file-detail-hash"), "a7f3c4e2…98f1b2a9"),
            ]))
            .child(div().flex_1())
            .child(
                div()
                    .p_4()
                    .border_t_1()
                    .border_color(theme::border())
                    .child(
                        components::button(
                            "file-location",
                            self.tr("action-open-location"),
                            Some(IconName::FolderOpen),
                            false,
                        )
                        .w_full(),
                    ),
            );

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
            .child(parts_header(self))
            .child(div().flex_1().min_h_0().overflow_y_scrollbar().children(
                part_indices(file.parts).into_iter().map(|index| {
                    if index == u16::MAX {
                        part_ellipsis()
                    } else {
                        part_row(
                            self,
                            index,
                            file.parts,
                            file.size_bytes,
                            file.state,
                            parts_verified,
                        )
                    }
                }),
            ))
            .child(
                div()
                    .h(px(46.0))
                    .px_4()
                    .flex()
                    .items_center()
                    .gap_3()
                    .border_t_1()
                    .border_color(theme::border())
                    .child(
                        Icon::new(if parts_verified {
                            IconName::CircleCheck
                        } else {
                            IconName::LoaderCircle
                        })
                        .text_color(if parts_verified {
                            theme::green()
                        } else {
                            theme::blue()
                        }),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(if parts_verified {
                                theme::green()
                            } else {
                                theme::blue()
                            })
                            .child(if parts_verified {
                                self.tr("file-detail-all-parts-verified")
                            } else {
                                self.tr("file-detail-parts-pending")
                            }),
                    )
                    .child(div().flex_1())
                    .child(div().text_xs().text_color(theme::text_muted()).child(
                        if !file.encrypted {
                            self.tr("common-not-applicable")
                        } else if parts_verified {
                            self.tr("file-detail-manifest-synced")
                        } else {
                            self.tr("file-detail-manifest-pending")
                        },
                    )),
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
                    .p_5()
                    .flex()
                    .gap_4()
                    .child(properties)
                    .child(parts),
            )
            .into_any_element()
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

fn parts_header(app: &TeleArkApp) -> AnyElement {
    div()
        .h(px(36.0))
        .px_4()
        .grid()
        .grid_cols(5)
        .items_center()
        .bg(theme::sidebar())
        .border_b_1()
        .border_color(theme::border())
        .text_xs()
        .text_color(theme::text_muted())
        .children([
            app.tr("file-detail-part-index"),
            app.tr("table-size"),
            app.tr("table-status"),
            app.tr("file-detail-remote-id"),
            app.tr("file-detail-upload-time"),
        ])
        .into_any_element()
}

fn part_indices(total: u16) -> Vec<u16> {
    if total <= 9 {
        return (1..=total).collect();
    }
    let mut indices: Vec<_> = (1..=6).collect();
    indices.push(u16::MAX);
    indices.extend(total.saturating_sub(2)..=total);
    indices
}

fn part_row(
    app: &TeleArkApp,
    index: u16,
    total: u16,
    total_size_bytes: u64,
    file_state: FileState,
    verified: bool,
) -> AnyElement {
    let size = format_bytes(
        app.locale(),
        logical_part_size(total_size_bytes, total, index),
    );
    div()
        .h(px(40.0))
        .px_4()
        .grid()
        .grid_cols(5)
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
                .items_center()
                .gap_2()
                .text_color(if verified {
                    theme::green()
                } else {
                    theme::blue()
                })
                .child(
                    Icon::new(if verified {
                        IconName::Check
                    } else {
                        IconName::LoaderCircle
                    })
                    .text_color(if verified {
                        theme::green()
                    } else {
                        theme::blue()
                    }),
                )
                .child(if verified {
                    app.tr("file-state-verified")
                } else {
                    app.tr(file_state.message_id())
                }),
        )
        .child(if verified {
            format!("1849{:02}", 20 + index)
        } else {
            "—".to_owned()
        })
        .child(if verified {
            format!("2026-08-23 10:{:02}", 21 + index.min(38))
        } else {
            "—".to_owned()
        })
        .into_any_element()
}

fn logical_part_size(total_size_bytes: u64, total_parts: u16, index: u16) -> u64 {
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
        for file in library_files() {
            let total: u64 = (1..=file.parts)
                .map(|index| logical_part_size(file.size_bytes, file.parts, index))
                .sum();

            assert_eq!(total, file.size_bytes, "{}", file.name);
        }
    }

    #[test]
    fn invalid_part_indices_have_no_displayed_bytes() {
        assert_eq!(logical_part_size(10, 0, 1), 0);
        assert_eq!(logical_part_size(10, 2, 0), 0);
        assert_eq!(logical_part_size(10, 2, 3), 0);
    }
}
