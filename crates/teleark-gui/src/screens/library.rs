use gpui_kit::component::{
    Disableable as _, Icon, IconName,
    checkbox::Checkbox,
    menu::{DropdownMenu as _, PopupMenuItem},
    scroll::ScrollableElement as _,
    tab::{Tab, TabBar},
};
use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px,
};
use teleark_core::{
    ApplicationErrorKind, EncryptionState, FileKind, RemoteState, VerificationState,
};
use teleark_i18n::{
    MessageArgs,
    format::{format_bytes, format_integer, format_unix_millis},
};

use crate::{
    app::{Page, TeleArkApp, is_preview_library_selection},
    components::{self, Tone},
    layout::LayoutPolicy,
    library_state::{ImportActivity, ImportFeedback, LibraryContent, LibraryRowView, LibraryView},
    theme,
};

impl TeleArkApp {
    pub(crate) fn render_library(
        &self,
        _window: &mut Window,
        layout: LayoutPolicy,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let padding = layout.content_padding();
        let preview_collection = is_preview_library_selection(self.nav_selection);
        let result_count = if preview_collection {
            0
        } else {
            self.library_content
                .snapshot()
                .map_or(0, |snapshot| snapshot.total_matching)
        };
        let result_label = self.tr_with(
            if self.library_view == LibraryView::Local {
                "library-visible-files"
            } else {
                "library-result-count-dynamic"
            },
            MessageArgs::new().with("count", result_count),
        );
        let import_label = match self.import_activity {
            ImportActivity::Idle => self.tr("action-import-files"),
            ImportActivity::Picking => self.tr("library-choosing-files"),
            ImportActivity::Importing => self.tr("library-importing-files"),
        };
        let import_icon = if self.import_activity == ImportActivity::Idle {
            IconName::FolderOpen
        } else {
            IconName::LoaderCircle
        };

        let selectable = self
            .library_content
            .snapshot()
            .map(|snapshot| {
                snapshot
                    .rows
                    .iter()
                    .filter(|file| self.library_row_selectable(file))
                    .map(|file| file.id.clone())
                    .take(5000)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let all_selected = !selectable.is_empty()
            && selectable
                .iter()
                .all(|id| self.library_selection.contains(id));
        let toolbar = components::page_toolbar(padding)
            .child(
                div()
                    .flex_1()
                    .min_w(px(220.0))
                    .flex()
                    .items_baseline()
                    .gap_3()
                    .child(components::section_title(self.tr("library-title")))
                    .when(self.library_content.snapshot().is_some(), |title| {
                        title.child(
                            div()
                                .text_xs()
                                .text_color(theme::text_muted())
                                .child(result_label),
                        )
                    }),
            )
            .child(
                components::button("library-import", import_label, Some(import_icon), true)
                    .on_click(cx.listener(|this, _, _, cx| this.choose_library_files(cx))),
            );

        let header = components::list_row()
            .px_4()
            .flex()
            .items_center()
            .bg(theme::sidebar())
            .border_b_1()
            .border_color(theme::border())
            .child(
                div().w(px(28.0)).flex_none().child(
                    Checkbox::new("library-select-all")
                        .debug_selector(|| "library-select-all".into())
                        .accessibility_label(self.tr("library-select-loaded"))
                        .checked(all_selected)
                        .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                            this.library_selection = if *checked {
                                selectable.clone()
                            } else {
                                Vec::new()
                            };
                            cx.notify();
                        })),
                ),
            )
            .child(table_header(self.tr("table-name"), None))
            .child(table_header(self.tr("table-size"), Some(90.0)))
            .when(!layout.is_compact(), |header| {
                header
                    .child(table_header(self.tr("table-type"), Some(110.0)))
                    .child(table_header(self.tr("table-source"), Some(140.0)))
                    .child(table_header(self.tr("table-modified"), Some(154.0)))
            })
            .child(table_header(
                self.tr("table-status"),
                Some(layout.library_status_width()),
            ))
            .when(layout.shows_library_encryption_parts(), |header| {
                header
                    .child(table_header(self.tr("table-encrypted"), Some(72.0)))
                    .child(table_header(self.tr("table-parts"), Some(54.0)))
            })
            .child(div().w(px(32.0)).flex_none());

        let facets = [
            (LibraryView::Local, "library-tab-local"),
            (LibraryView::Remote, "library-tab-remote"),
        ];
        let kinds = [
            ("library-types-all", "library-types-all"),
            ("nav-videos", "nav-videos"),
            ("nav-docs", "nav-documents"),
            ("nav-archives", "nav-archives"),
            ("nav-images", "library-images"),
            ("nav-audio", "library-audio"),
            ("nav-disk-images", "library-disk-images"),
            ("nav-other", "library-other"),
        ];
        let type_label = kinds
            .iter()
            .find(|(id, _)| *id == self.library_kind_selection)
            .map_or("library-types-all", |(_, label)| *label);
        let labels = kinds.map(|(id, label)| (id, self.tr(label)));
        let selected_kind = self.library_kind_selection;
        let owner = cx.entity().downgrade();
        let type_menu = components::button(
            "library-type-filter",
            self.tr(type_label),
            Some(IconName::ChevronDown),
            false,
        )
        .h(px(28.0))
        .accessibility_label(self.tr("library-type-filter"))
        .dropdown_menu(move |mut menu, _, _| {
            for (id, label) in &labels {
                let id = *id;
                let owner = owner.clone();
                menu = menu.item(
                    PopupMenuItem::new(label.clone())
                        .checked(id == selected_kind)
                        .on_click(move |_, _, cx| {
                            let _ = owner.update(cx, |this, cx| {
                                this.library_kind_selection = id;
                                this.refresh_library(cx);
                            });
                        }),
                );
            }
            menu
        });
        let categories = div()
            .flex_none()
            .px(px(padding))
            .pb_3()
            .flex()
            .items_center()
            .gap_3()
            .child(
                TabBar::new("library-facets")
                    .segmented()
                    .selected_index(if self.library_view == LibraryView::Local {
                        0
                    } else {
                        1
                    })
                    .children(facets.iter().map(|(_, label)| {
                        let label = *label;
                        Tab::new()
                            .label(self.tr(label))
                            .debug_selector(move || label.into())
                    }))
                    .on_click(cx.listener(move |this, index: &usize, _, cx| {
                        if let Some((view, _)) = facets.get(*index) {
                            this.library_view = *view;
                            this.refresh_library(cx);
                        }
                    })),
            )
            .child(div().flex_1())
            .child(type_menu);
        let bulk = div()
            .px(px(padding))
            .pb_3()
            .flex()
            .items_center()
            .gap_3()
            .child(div().text_sm().child(self.tr_with(
                "library-selected-count",
                MessageArgs::new().with("count", self.library_selection.len() as u64),
            )))
            .child(
                components::button(
                    "library-bulk-action",
                    self.tr(if self.library_view == LibraryView::Local {
                        "action-show-in-folder"
                    } else {
                        "action-download"
                    }),
                    Some(if self.library_view == LibraryView::Local {
                        IconName::FolderOpen
                    } else {
                        IconName::ArrowDown
                    }),
                    false,
                )
                .disabled(self.library_action_busy || self.visual_preview)
                .on_click(cx.listener(|this, _, _, cx| this.act_on_library_selection(cx))),
            )
            .child(
                components::button(
                    "library-clear-selection",
                    self.tr("telegram-files-clear-selection"),
                    None,
                    false,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.library_selection.clear();
                    cx.notify();
                })),
            );

        let body = if preview_collection {
            self.render_library_state(
                IconName::FolderOpen,
                "library-collection-preview-title",
                "library-collection-preview-description",
                None,
                cx,
            )
        } else {
            match &self.library_content {
                LibraryContent::Loading => self.render_library_state(
                    IconName::LoaderCircle,
                    "library-loading-title",
                    "library-loading-description",
                    None,
                    cx,
                ),
                LibraryContent::Failed(kind) => self.render_library_state(
                    IconName::TriangleAlert,
                    "library-error-title",
                    application_error_message_id(*kind),
                    Some("common-retry"),
                    cx,
                ),
                LibraryContent::Empty(_) => self.render_library_state(
                    IconName::FolderOpen,
                    if self.library_view == LibraryView::Local {
                        "library-local-empty-title"
                    } else {
                        "library-empty-title"
                    },
                    if self.library_view == LibraryView::Local {
                        "library-local-empty-description"
                    } else {
                        "library-remote-empty-description"
                    },
                    Some("action-import-files"),
                    cx,
                ),
                LibraryContent::Ready(snapshot) => {
                    let rows = snapshot
                        .rows
                        .clone()
                        .into_iter()
                        .enumerate()
                        .map(|(index, file)| self.render_library_row(index, file, layout, cx));
                    div()
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scrollbar()
                        .children(rows)
                        .into_any_element()
                }
            }
        };

        let table = components::card()
            .mx(px(padding))
            .mb(px(padding))
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(header)
            .child(body)
            .when(!preview_collection, |table| {
                table.when_some(self.library_content.snapshot(), |table, snapshot| {
                    table.child(self.render_library_footer(snapshot, layout, cx))
                })
            });

        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .bg(theme::canvas())
            .child(toolbar)
            .child(categories)
            .when(!self.library_selection.is_empty(), |view| view.child(bulk))
            .child(
                div()
                    .flex_none()
                    .px(px(padding))
                    .py_2()
                    .text_sm()
                    .text_color(theme::text_secondary())
                    .child(self.tr(if self.library_view == LibraryView::Local {
                        "library-local-explanation"
                    } else {
                        "library-remote-explanation"
                    })),
            )
            .when_some(self.library_action_error, |page, error| {
                page.child(
                    div()
                        .px(px(padding))
                        .pb_3()
                        .text_sm()
                        .text_color(theme::red())
                        .child(self.tr(application_error_message_id(error))),
                )
            })
            .when(self.library_action_busy, |page| {
                page.child(
                    components::button(
                        "library-cancel-batch",
                        self.tr("common-cancel"),
                        None,
                        false,
                    )
                    .mx(px(padding))
                    .mb_3()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.library_batch_cancellation
                            .store(true, std::sync::atomic::Ordering::Relaxed);
                        cx.notify();
                    })),
                )
            })
            .when_some(self.import_feedback, |page, feedback| {
                page.child(self.render_import_feedback(feedback, padding))
            })
            .child(table)
            .into_any_element()
    }

    fn render_library_state(
        &self,
        icon: IconName,
        title_id: &'static str,
        description_id: &'static str,
        action_id: Option<&'static str>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex_1()
            .min_h(px(180.0))
            .p_6()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .text_center()
            .child(
                div()
                    .size(px(44.0))
                    .rounded_full()
                    .bg(theme::blue_soft())
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Icon::new(icon).text_color(theme::blue())),
            )
            .child(
                div()
                    .mt_4()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.tr(title_id)),
            )
            .child(
                div()
                    .mt_2()
                    .max_w(px(440.0))
                    .text_sm()
                    .text_color(theme::text_secondary())
                    .child(self.tr(description_id)),
            )
            .when_some(action_id, |state, action_id| {
                state.child(
                    components::button(
                        "library-state-action",
                        self.tr(action_id),
                        Some(IconName::Redo2),
                        true,
                    )
                    .mt_4()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if action_id == "action-import-files" {
                            this.choose_library_files(cx);
                        } else {
                            this.refresh_library(cx);
                        }
                    })),
                )
            })
            .into_any_element()
    }

    fn render_import_feedback(&self, feedback: ImportFeedback, padding: f32) -> AnyElement {
        let (tone, icon, title, detail) = match feedback {
            ImportFeedback::Succeeded { imported } => (
                Tone::Green,
                IconName::CircleCheck,
                self.tr_with(
                    "library-import-success",
                    MessageArgs::new().with("count", imported),
                ),
                None,
            ),
            ImportFeedback::PartiallySucceeded { imported, failed } => (
                Tone::Amber,
                IconName::TriangleAlert,
                self.tr_with(
                    "library-import-partial",
                    MessageArgs::new()
                        .with("imported", imported)
                        .with("failed", failed),
                ),
                None,
            ),
            ImportFeedback::Failed { failed, reason } => (
                Tone::Red,
                IconName::TriangleAlert,
                self.tr_with(
                    "library-import-failed",
                    MessageArgs::new().with("count", failed),
                ),
                Some(self.tr(application_error_message_id(reason))),
            ),
            ImportFeedback::NoFilesSelected => (
                Tone::Neutral,
                IconName::FolderOpen,
                self.tr("library-import-empty"),
                None,
            ),
            ImportFeedback::PickerFailed => (
                Tone::Red,
                IconName::TriangleAlert,
                self.tr("library-picker-failed"),
                None,
            ),
        };
        let color = match tone {
            Tone::Green => theme::green(),
            Tone::Amber => theme::amber(),
            Tone::Red => theme::red(),
            Tone::Blue => theme::blue(),
            Tone::Purple => theme::purple(),
            Tone::Neutral => theme::text_secondary(),
        };

        div()
            .mx(px(padding))
            .mb_3()
            .px_4()
            .py_3()
            .rounded(theme::RADIUS_MEDIUM)
            .border_1()
            .border_color(theme::border())
            .bg(theme::surface())
            .flex()
            .items_start()
            .gap_3()
            .child(Icon::new(icon).text_color(color))
            .child(div().min_w_0().flex_1().text_sm().child(title).when_some(
                detail,
                |message, detail| {
                    message.child(
                        div()
                            .mt_1()
                            .text_xs()
                            .text_color(theme::text_secondary())
                            .child(detail),
                    )
                },
            ))
            .into_any_element()
    }

    fn render_library_footer(
        &self,
        snapshot: &crate::library_state::LibrarySnapshot,
        layout: LayoutPolicy,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let total_files = self.tr_with(
            "library-visible-files",
            MessageArgs::new().with("count", snapshot.rows.len() as u64),
        );
        let total_size = self.tr_with(
            "library-visible-size",
            MessageArgs::new().with(
                "size",
                format_bytes(
                    self.locale(),
                    snapshot
                        .rows
                        .iter()
                        .map(|row| row.size_bytes)
                        .fold(0_u64, u64::saturating_add),
                ),
            ),
        );

        components::list_footer("library-list-footer")
            .child(total_files)
            .child(total_size)
            .when(!layout.is_compact(), |footer| footer.child(div().flex_1()))
            .when(self.library_load_more_error.is_some(), |footer| {
                footer.child(
                    div()
                        .text_color(theme::red())
                        .child(self.tr("library-load-more-failed")),
                )
            })
            .when(snapshot.next_cursor.is_some(), |footer| {
                footer.child(
                    components::button(
                        "library-load-more",
                        if self.library_loading_more {
                            self.tr("library-loading-more")
                        } else {
                            self.tr("library-load-more")
                        },
                        Some(if self.library_loading_more {
                            IconName::LoaderCircle
                        } else {
                            IconName::ArrowDown
                        }),
                        false,
                    )
                    .h(px(22.0))
                    .flex_none()
                    .disabled(self.library_loading_more)
                    .on_click(cx.listener(|this, _, _, cx| this.load_more_library(cx))),
                )
            })
            .into_any_element()
    }

    fn render_library_row(
        &self,
        index: usize,
        file: LibraryRowView,
        layout: LayoutPolicy,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.selected_file == index;
        let name = div()
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .gap_2()
            .child(file_icon(file.kind))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme::text_primary())
                    .debug_selector(move || format!("library-name-{index}"))
                    .child(file.name.clone()),
            );
        let (state_id, state_tone) = file_status(file.remote_state, file.verification_state);
        let source = file.source_name.clone().map_or_else(
            || {
                file.source_chat_id.map_or_else(
                    || self.tr("library-source-local"),
                    |chat_id| {
                        self.tr_with(
                            "library-source-telegram-chat",
                            MessageArgs::new().with("chat_id", chat_id),
                        )
                    },
                )
            },
            SharedString::from,
        );
        let modified = file.modified_at_unix_ms.map_or_else(
            || self.tr("common-not-applicable"),
            |timestamp| SharedString::from(format_unix_millis(self.locale(), timestamp)),
        );

        let action_file = file.clone();
        let action_path = file.local_source_path.clone();
        let download_source = file.download_source(self.telegram_account.as_ref().map(|a| a.id));
        let action = components::list_icon_button(
            file.id.element_id("library-row-action"),
            if action_path.is_some() {
                IconName::FolderOpen
            } else {
                IconName::ArrowDown
            },
            self.tr(if action_path.is_some() {
                "action-show-in-folder"
            } else {
                download_source.err().unwrap_or("action-download")
            }),
        )
        .debug_selector(move || format!("library-action-{index}"))
        .disabled(
            self.visual_preview
                || action_path.is_none()
                    && (download_source.is_err()
                        || self.library_action_busy
                        || self.transfers.is_none()),
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            cx.stop_propagation();
            if let Some(path) = &action_path {
                cx.reveal_path(path);
            } else {
                this.selected_file = index;
                this.download_library_file(action_file.clone(), cx);
            }
        }));

        let selection_id = file.id.clone();
        let checkbox = Checkbox::new(file.id.element_id("library-select"))
            .accessibility_label(self.tr("library-select-file"))
            .checked(self.library_selection.contains(&file.id))
            .disabled(!self.library_row_selectable(&file))
            .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                cx.stop_propagation();
                this.library_selection.retain(|id| id != &selection_id);
                if *checked && this.library_selection.len() < 5000 {
                    this.library_selection.push(selection_id.clone());
                }
                cx.notify();
            }));
        components::list_row()
            .id(file.id.element_id("library-row"))
            .debug_selector(move || format!("library-row-{index}"))
            .px_4()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(theme::border_subtle())
            .text_size(theme::LIST_TEXT_SIZE)
            .cursor_pointer()
            .focusable()
            .tab_index(0)
            .when(selected, |row| row.bg(theme::blue_pale()))
            .hover(|row| row.bg(theme::blue_pale()))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.selected_file = index;
                this.set_page(Page::FileDetail, cx);
            }))
            .on_key_down(
                cx.listener(move |this, event: &gpui_kit::KeyDownEvent, _, cx| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        this.selected_file = index;
                        this.set_page(Page::FileDetail, cx);
                    }
                }),
            )
            .child(div().w(px(28.0)).flex_none().child(checkbox))
            .child(name)
            .child(table_value(
                format_bytes(self.locale(), file.size_bytes),
                90.0,
            ))
            .when(!layout.is_compact(), |row| {
                row.child(table_value(self.tr(file_kind_message_id(file.kind)), 110.0))
                    .child(table_value(source, 140.0))
                    .child(table_value(modified, 154.0))
            })
            .child(
                div()
                    .w(px(layout.library_status_width()))
                    .child(components::badge(self.tr(state_id), state_tone)),
            )
            .when(layout.shows_library_encryption_parts(), |row| {
                row.child(
                    div().w(px(72.0)).flex().items_center().child(
                        Icon::new(
                            if matches!(
                                file.encryption_state,
                                EncryptionState::Encrypted | EncryptionState::Locked
                            ) {
                                IconName::Asterisk
                            } else {
                                IconName::Dash
                            },
                        )
                        .text_color(
                            if matches!(
                                file.encryption_state,
                                EncryptionState::Encrypted | EncryptionState::Locked
                            ) {
                                theme::text_secondary()
                            } else {
                                theme::text_muted()
                            },
                        ),
                    ),
                )
                .child(
                    div()
                        .w(px(54.0))
                        .text_color(theme::text_secondary())
                        .child(format_integer(self.locale(), u64::from(file.part_count))),
                )
            })
            .child(div().w(px(32.0)).flex_none().child(action))
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
        FileKind::Other => (theme::text_secondary(), theme::border_subtle()),
        _ => (theme::text_secondary(), theme::border_subtle()),
    };

    div()
        .size(theme::LIST_BADGE_HEIGHT)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme::RADIUS_SMALL)
        .bg(background)
        .text_color(foreground)
        .child(
            Icon::new(file_kind_icon(kind))
                .size(theme::LIST_ICON_SIZE)
                .text_color(foreground),
        )
        .into_any_element()
}

fn file_kind_icon(kind: FileKind) -> IconName {
    match kind {
        FileKind::Video => IconName::GalleryVerticalEnd,
        FileKind::Document => IconName::File,
        FileKind::Archive => IconName::Inbox,
        FileKind::Audio => IconName::File,
        FileKind::Image => IconName::File,
        FileKind::DiskImage => IconName::File,
        FileKind::Other => IconName::File,
        _ => IconName::File,
    }
}

pub(crate) fn file_kind_message_id(kind: FileKind) -> &'static str {
    match kind {
        FileKind::Video => "file-type-video",
        FileKind::Document => "file-type-document",
        FileKind::Archive => "file-type-archive",
        FileKind::Audio => "file-type-audio",
        FileKind::Image => "file-type-image",
        FileKind::DiskImage => "file-type-disk-image",
        FileKind::Other => "file-type-other",
        _ => "file-type-other",
    }
}

pub(crate) fn file_status(
    remote_state: RemoteState,
    verification_state: VerificationState,
) -> (&'static str, Tone) {
    match verification_state {
        VerificationState::Verified => ("file-state-verified", Tone::Green),
        VerificationState::Verifying => ("file-state-verifying", Tone::Blue),
        VerificationState::Failed => ("file-state-verification-failed", Tone::Red),
        VerificationState::Unverified => match remote_state {
            RemoteState::LocalOnly => ("file-state-local", Tone::Neutral),
            RemoteState::Uploading => ("file-state-uploading", Tone::Blue),
            RemoteState::Uploaded => ("file-state-remote-indexed", Tone::Neutral),
            RemoteState::RemoteMissing => ("file-state-remote-missing", Tone::Red),
            _ => ("file-state-local", Tone::Neutral),
        },
        _ => ("file-state-local", Tone::Neutral),
    }
}

pub(crate) fn application_error_message_id(kind: ApplicationErrorKind) -> &'static str {
    match kind {
        ApplicationErrorKind::InvalidRequest => "error-library-invalid-request",
        ApplicationErrorKind::NotFound => "error-library-not-found",
        ApplicationErrorKind::Conflict => "error-library-conflict",
        ApplicationErrorKind::Persistence => "error-library-persistence",
        ApplicationErrorKind::SourceMissing => "error-library-source-missing",
        ApplicationErrorKind::SourceChanged => "error-library-source-changed",
        ApplicationErrorKind::PermissionDenied => "error-library-permission-denied",
        ApplicationErrorKind::Capacity => "error-library-capacity",
        ApplicationErrorKind::Authorization => "error-library-authorization",
        ApplicationErrorKind::Network => "error-library-network",
        ApplicationErrorKind::Cancelled => "error-library-cancelled",
        _ => "error-library-unknown",
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_file_states_map_to_semantic_localized_statuses() {
        assert_eq!(
            file_status(RemoteState::LocalOnly, VerificationState::Unverified),
            ("file-state-local", Tone::Neutral)
        );
        assert_eq!(
            file_status(RemoteState::Uploaded, VerificationState::Verified),
            ("file-state-verified", Tone::Green)
        );
        assert_eq!(
            file_status(RemoteState::Uploaded, VerificationState::Unverified),
            ("file-state-remote-indexed", Tone::Neutral)
        );
        assert_eq!(
            file_status(RemoteState::RemoteMissing, VerificationState::Unverified),
            ("file-state-remote-missing", Tone::Red)
        );
    }

    #[test]
    fn every_current_core_file_kind_has_a_presentation_message() {
        for kind in [
            FileKind::Video,
            FileKind::Document,
            FileKind::Archive,
            FileKind::Audio,
            FileKind::Image,
            FileKind::DiskImage,
            FileKind::Other,
        ] {
            assert!(file_kind_message_id(kind).starts_with("file-type-"));
        }
    }
}
