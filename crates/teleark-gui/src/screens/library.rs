use gpui::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px,
};
use gpui_component::{Icon, IconName, scroll::ScrollableElement as _};

use crate::{
    app::{Page, TeleArkApp},
    components::{self, Tone},
    mock::{FileKind, FileRow, FileState, library_files},
    theme,
};

impl TeleArkApp {
    pub(crate) fn render_library(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let toolbar = div()
            .h(px(58.0))
            .px_5()
            .flex()
            .items_center()
            .gap_3()
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_baseline()
                    .gap_3()
                    .child(components::section_title(self.tr("library-title")))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::text_muted())
                            .child(self.tr("library-result-count")),
                    ),
            )
            .child(
                components::button(
                    "library-upload",
                    self.tr("action-upload"),
                    Some(IconName::ArrowUp),
                    true,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.show_upload = true;
                    cx.notify();
                })),
            )
            .child(components::button(
                "library-new-collection",
                self.tr("collection-new"),
                Some(IconName::Folder),
                false,
            ))
            .child(components::button(
                "library-filter",
                self.tr("action-filter"),
                Some(IconName::Settings2),
                false,
            ))
            .child(components::icon_button(
                "library-view",
                IconName::LayoutDashboard,
                self.tr("action-view-options"),
            ));

        let facets = div()
            .h(px(42.0))
            .px_5()
            .flex()
            .items_center()
            .gap_2()
            .border_t_1()
            .border_color(theme::border_subtle())
            .children([
                components::badge(self.tr("filter-all-channels"), Tone::Blue),
                components::badge(self.tr("filter-video"), Tone::Neutral),
                components::badge(self.tr("filter-large-files"), Tone::Neutral),
                components::badge("2025–2026", Tone::Neutral),
                components::badge("mkv", Tone::Neutral),
            ]);

        let header = div()
            .h(px(36.0))
            .px_4()
            .flex()
            .items_center()
            .bg(theme::sidebar())
            .border_b_1()
            .border_color(theme::border())
            .child(table_header(self.tr("table-name"), None))
            .child(table_header(self.tr("table-size"), Some(90.0)))
            .child(table_header(self.tr("table-type"), Some(110.0)))
            .child(table_header(self.tr("table-source"), Some(140.0)))
            .child(table_header(self.tr("table-modified"), Some(154.0)))
            .child(table_header(self.tr("table-status"), Some(108.0)))
            .child(table_header(self.tr("table-encrypted"), Some(72.0)))
            .child(table_header(self.tr("table-parts"), Some(54.0)));

        let query = self.search_input.read(cx).value().to_lowercase();
        let visible_files: Vec<_> = library_files()
            .into_iter()
            .enumerate()
            .filter(|(index, file)| library_matches_nav(self.nav_selection, *index, file))
            .filter(|(_, file)| {
                query.is_empty()
                    || file.name.to_lowercase().contains(&query)
                    || file.source.to_lowercase().contains(&query)
            })
            .collect();
        let rows = visible_files
            .into_iter()
            .map(|(index, file)| self.render_library_row(index, file, cx));

        let footer = div()
            .h(px(46.0))
            .px_4()
            .flex()
            .items_center()
            .gap_5()
            .border_t_1()
            .border_color(theme::border())
            .text_xs()
            .text_color(theme::text_secondary())
            .child(self.tr("library-total-files"))
            .child(
                div()
                    .w(px(250.0))
                    .child(components::progress(43.0, Tone::Blue)),
            )
            .child("2.34 TB")
            .child(div().flex_1())
            .child(self.tr("storage-telegram"))
            .child("18.76 TB");

        let table = components::card()
            .mx_5()
            .mb_5()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(header)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .children(rows),
            )
            .child(footer);

        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .bg(theme::canvas())
            .child(toolbar)
            .child(facets)
            .child(table)
            .into_any_element()
    }

    fn render_library_row(
        &self,
        index: usize,
        file: FileRow,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.selected_file == index;
        let name = div()
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .gap_3()
            .child(file_icon(file.kind))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme::text_primary())
                    .child(file.name),
            );

        let state_tone = match file.state {
            FileState::Remote | FileState::Downloaded => Tone::Green,
            FileState::Uploading | FileState::Verifying => Tone::Blue,
        };

        div()
            .id(("library-row", index))
            .h(theme::ROW_HEIGHT)
            .px_4()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(theme::border_subtle())
            .text_sm()
            .cursor_pointer()
            .focusable()
            .tab_index(0)
            .when(selected, |row| row.bg(theme::blue_pale()))
            .hover(|row| row.bg(theme::blue_pale()))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.selected_file = index;
                this.set_page(Page::FileDetail, cx);
            }))
            .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    this.selected_file = index;
                    this.set_page(Page::FileDetail, cx);
                }
            }))
            .child(name)
            .child(table_value(file.size, 90.0))
            .child(table_value(self.tr(file.kind.message_id()), 110.0))
            .child(table_value(file.source, 140.0))
            .child(table_value(file.modified, 154.0))
            .child(div().w(px(108.0)).child(components::badge(
                self.tr(file.state.message_id()),
                state_tone,
            )))
            .child(
                div().w(px(72.0)).flex().items_center().child(
                    Icon::new(if file.encrypted {
                        IconName::Asterisk
                    } else {
                        IconName::Dash
                    })
                    .text_color(if file.encrypted {
                        theme::text_secondary()
                    } else {
                        theme::text_muted()
                    }),
                ),
            )
            .child(
                div()
                    .w(px(54.0))
                    .text_color(theme::text_secondary())
                    .child(file.parts.to_string()),
            )
            .into_any_element()
    }
}

fn file_icon(kind: FileKind) -> AnyElement {
    let (foreground, background) = match kind {
        FileKind::Video => (theme::blue(), theme::blue_soft()),
        FileKind::DiskImage => (theme::cyan(), theme::cyan_soft()),
        FileKind::Document => (theme::red(), theme::red_soft()),
        FileKind::Archive => (theme::amber(), theme::amber_soft()),
        FileKind::Image => (theme::purple(), theme::purple_soft()),
        FileKind::Audio => (theme::green(), theme::green_soft()),
    };

    div()
        .size(px(26.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme::RADIUS_SMALL)
        .bg(background)
        .text_color(foreground)
        .text_xs()
        .font_weight(FontWeight::BOLD)
        .child(kind.glyph())
        .into_any_element()
}

fn table_header(label: impl Into<SharedString>, width: Option<f32>) -> AnyElement {
    div()
        .when_some(width, |cell, width| cell.w(px(width)))
        .when(width.is_none(), |cell| cell.flex_1())
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme::text_secondary())
        .child(label.into())
        .into_any_element()
}

fn table_value(value: impl Into<SharedString>, width: f32) -> AnyElement {
    div()
        .w(px(width))
        .min_w_0()
        .truncate()
        .text_color(theme::text_secondary())
        .child(value.into())
        .into_any_element()
}

fn library_matches_nav(selection: &str, index: usize, file: &FileRow) -> bool {
    match selection {
        "nav-recent" => index < 3,
        "nav-videos" => file.kind == FileKind::Video,
        "nav-docs" => file.kind == FileKind::Document,
        "nav-archives" => file.kind == FileKind::Archive,
        "channel-saved" => file.source == "Saved Messages",
        "channel-design" => file.source == "Design Assets",
        "channel-software" => file.source == "Software",
        "collection-mac" => file.source == "Work Backup",
        "collection-course" => file.source == "Course Materials",
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidebar_facets_filter_the_mock_library_without_reindexing_rows() {
        let files = library_files();
        assert!(library_matches_nav("nav-videos", 0, &files[0]));
        assert!(!library_matches_nav("nav-videos", 1, &files[1]));
        assert!(library_matches_nav("nav-recent", 2, &files[2]));
        assert!(!library_matches_nav("nav-recent", 3, &files[3]));
        assert!(library_matches_nav("collection-course", 7, &files[7]));
    }
}
