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
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _, Styled as _,
    div, prelude::FluentBuilder as _, px,
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
                teleark_i18n::format::format_bytes(app.locale(), app.upload_source_total_bytes),
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
        .debug_selector(|| "upload-drop-target".into())
        .when(app.can_accept_upload_files(), |popup| {
            popup
                .drag_over::<gpui_kit::ExternalPaths>(|style, _, _, _| {
                    style.border_color(theme::blue()).bg(theme::blue_soft())
                })
                .on_drop(cx.listener(|this, paths: &gpui_kit::ExternalPaths, _, cx| {
                    this.drop_upload_files(paths.paths(), cx);
                    cx.stop_propagation();
                }))
        })
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
                                    .debug_selector(|| "upload-choose".into())
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
                                .child(app.tr("upload-files-independent"))
                                .child(div().child(app.tr("upload-folders-unsupported"))),
                        )
                        .child(
                            div()
                                .mt_2()
                                .text_xs()
                                .text_color(theme::blue())
                                .debug_selector(|| "upload-drop-feedback".into())
                                .child(app.tr("upload-drop-files")),
                        )
                        .when(app.upload_preparing, |area| {
                            area.child(app.render_upload_selection_progress(true, cx))
                        })
                        .when(!app.upload_sources.is_empty(), |area| {
                            area.child(
                                gpui_kit::uniform_list(
                                    "upload-selected-files",
                                    app.upload_sources.len(),
                                    cx.processor(
                                        move |this, range: std::ops::Range<usize>, _, cx| {
                                            range
                                                .filter_map(|index| {
                                                    this.upload_sources.get(index).map(|source| {
                                                        let tooltip = source
                                                            .path
                                                            .to_string_lossy()
                                                            .into_owned();
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
                                                            this.locale(),
                                                            source.size_bytes,
                                                        )),
                                                ),
                                        )
                                        .child(
                                            components::list_icon_button(
                                                ("upload-remove-source", index),
                                                IconName::Close,
                                                this.tr("upload-remove-file"),
                                            )
                                            .ghost()
                                            .tooltip(tooltip)
                                            .debug_selector(move || format!("upload-source-action-{index}"))
                                            .disabled(this.upload_preparing)
                                            .on_click(
                                                cx.listener(move |this, _, _, cx| {
                                                    this.remove_upload_source(index, cx);
                                                }),
                                            ),
                                        )
                                                    })
                                                })
                                                .collect::<Vec<_>>()
                                        },
                                    ),
                                )
                                .h(px((app.upload_sources.len() as f32 * f32::from(theme::ROW_HEIGHT)).min(168.0)))
                                .w_full()
                                .mt_3()
                                .debug_selector(|| "upload-selected-files".into()),
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

impl TeleArkApp {
    pub(crate) fn render_upload_selection_progress(
        &self,
        preparation: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let progress = if preparation {
            &self.upload_preparation_progress
        } else {
            &self.upload_selection_progress
        };
        let Some(progress) = progress else {
            return div().into_any_element();
        };
        let state = progress.snapshot();
        let processed = state
            .completed
            .saturating_add(state.failed)
            .saturating_add(state.cancelled)
            .saturating_add(state.paused);
        let pending = state.total.saturating_sub(processed);
        let integer =
            |value: usize| teleark_i18n::format::format_integer(self.locale(), value as u64);
        let elapsed = |value: std::time::Instant| {
            teleark_i18n::format::format_duration_millis(
                self.locale(),
                value.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
            )
        };
        let cancel = progress.clone();
        div()
            .flex_none()
            .px_3()
            .py_2()
            .bg(theme::blue_soft())
            .text_xs()
            .debug_selector(|| "upload-selection-progress".into())
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().min_w_0().child(self.tr(
                        if state.cancel_requested && !state.finished {
                            "upload-selection-cancelling"
                        } else {
                            upload_selection_phase_id(state.phase)
                        },
                    )))
                    .when(!state.finished, |row| {
                        row.child(
                            components::button(
                                "cancel-upload-selection",
                                self.tr(if preparation {
                                    "action-cancel"
                                } else {
                                    "upload-stop-after-current"
                                }),
                                None,
                                false,
                            )
                            .ghost()
                            .disabled(state.cancel_requested)
                            .on_click(cx.listener(
                                move |_, _, _, cx| {
                                    cancel.cancel();
                                    cx.notify();
                                },
                            )),
                        )
                    }),
            )
            .child(
                self.tr_with(
                    if state.phase == teleark_runtime::VaultUploadSelectionPhase::SavingQueue {
                        "upload-selection-saved-count"
                    } else if preparation {
                        "upload-selection-inspecting"
                    } else {
                        "upload-selection-counts"
                    },
                    teleark_i18n::MessageArgs::new()
                        .with("total", integer(state.total))
                        .with("inspected", integer(state.inspected))
                        .with("saved", integer(state.saved))
                        .with("completed", integer(state.completed))
                        .with("failed", integer(state.failed))
                        .with("cancelled", integer(state.cancelled))
                        .with("paused", integer(state.paused))
                        .with("pending", integer(pending)),
                ),
            )
            .when(!state.finished, |body| {
                body.child(
                    self.tr_with(
                        "upload-selection-timing",
                        teleark_i18n::MessageArgs::new()
                            .with("elapsed", elapsed(state.phase_since))
                            .with("idle", elapsed(state.last_activity)),
                    ),
                )
            })
            .when(!preparation, |body| {
                body.child(self.tr("upload-selection-retention"))
            })
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .text_color(theme::text_secondary())
                    .children(state.timeline.iter().map(|(phase, millis)| {
                        div().child(
                            self.tr_with(
                                "upload-selection-history-entry",
                                teleark_i18n::MessageArgs::new()
                                    .with(
                                        "phase",
                                        self.tr(upload_selection_phase_id(*phase)).to_string(),
                                    )
                                    .with(
                                        "time",
                                        teleark_i18n::format::format_duration_millis(
                                            self.locale(),
                                            *millis,
                                        ),
                                    ),
                            ),
                        )
                    })),
            )
            .when(state.finished && state.error.is_some(), |body| {
                body.when_some(
                    super::settings::vault_activity_message(self),
                    |body, (message, tone)| {
                        body.child(div().text_color(tone.foreground()).child(message))
                    },
                )
            })
            .into_any_element()
    }
}

fn upload_selection_phase_id(phase: teleark_runtime::VaultUploadSelectionPhase) -> &'static str {
    use teleark_runtime::VaultUploadSelectionPhase;
    match phase {
        VaultUploadSelectionPhase::Queued => "upload-selection-queued",
        VaultUploadSelectionPhase::Inspecting => "upload-selection-checking-files",
        VaultUploadSelectionPhase::SavingQueue => "upload-selection-saving-queue",
        VaultUploadSelectionPhase::CheckingStorage => "upload-selection-checking-channel",
        VaultUploadSelectionPhase::Uploading => "upload-selection-uploading",
        VaultUploadSelectionPhase::Finished => "upload-selection-finished",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Page;
    use teleark_i18n::{Localizer, SupportedLocale};
    use teleark_runtime::AppearancePreference;

    #[gpui_kit::test]
    fn large_selection_only_materializes_visible_rows_and_keeps_actions_reachable(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let source = teleark_runtime::inspect_upload_sources(&[
            std::env::current_exe().expect("test executable")
        ])
        .expect("fixture")
        .remove(0);
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Storage);
        app.update(cx, |app, cx| {
            app.show_upload = true;
            app.upload_sources = (0..10_000)
                .map(|i| {
                    let mut source = source.clone();
                    source.file_name = format!("Synthetic selection {i}.txt");
                    source.size_bytes = 1;
                    source
                })
                .collect();
            app.upload_source_total_bytes = 10_000;
            cx.notify();
        });
        for (width, height) in [(900.0, 600.0), (1360.0, 760.0)] {
            cx.simulate_resize(gpui_kit::size(px(width), px(height)));
            {
                let locale = SupportedLocale::EnUs;
                for appearance in [AppearancePreference::Light, AppearancePreference::Dark] {
                    cx.update(|window, cx| {
                        app.update(cx, |app, cx| {
                            app.localizer = Localizer::new(locale).expect("catalog");
                            theme::apply_appearance(appearance, window, cx);
                            cx.notify();
                        })
                    });
                    cx.run_until_parked();
                    let row = cx
                        .debug_bounds("upload-source-row-0")
                        .expect("first upload row");
                    assert_eq!(row.size.height, px(24.0));
                    for selector in ["upload-source-name-0", "upload-source-action-0"] {
                        let child = cx.debug_bounds(selector).expect("upload row content");
                        assert!(
                            child.top() >= row.top() && child.bottom() <= row.bottom(),
                            "{selector}: {child:?}"
                        );
                    }
                    assert!(
                        cx.debug_bounds("upload-source-row-128").is_none(),
                        "offscreen rows must not be built"
                    );
                    for selector in ["upload-drop-target", "upload-add-queue", "upload-choose"] {
                        let bounds = cx.debug_bounds(selector).expect("upload control");
                        assert!(bounds.left() >= px(0.0) && bounds.right() <= px(width));
                        assert!(
                            bounds.top() >= px(0.0) && bounds.bottom() <= px(height),
                            "{locale:?} {width}x{height} {selector}: {bounds:?}"
                        );
                    }
                }
            }
        }
    }
}
