//! A distinct home for authenticated TeleArk files and their underlying source.
use crate::app::storage::StorageAction;
use crate::assets::Symbol;
use crate::{
    app::{Page, StorageView, TeleArkApp, UnlockIntent},
    components::{self, Tone},
    layout::LayoutPolicy,
    theme,
};
use gpui_kit::component::{
    Disableable as _, Icon, IconName,
    button::ButtonVariants as _,
    scroll::ScrollableElement as _,
    tab::{Tab, TabBar},
};
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _, Styled as _,
    Window, div, prelude::FluentBuilder as _, px,
};
use teleark_runtime::StorageMaintenancePhase;

impl TeleArkApp {
    pub(crate) fn render_storage_workspace(
        &self,
        _window: &mut Window,
        layout: LayoutPolicy,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let legacy = self.page == Page::LegacyRecovery;
        let ready = legacy || self.storage_status.usable_channel().is_some();
        let header = div()
            .h(px(76.0))
            .flex_none()
            .flex()
            .items_center()
            .gap_3()
            .child(components::app_mark(38.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_size(px(23.0))
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(self.tr(if legacy {
                                "storage-legacy-title"
                            } else {
                                "storage-nav-title"
                            })),
                    )
                    .child(
                        div()
                            .mt_1()
                            .text_xs()
                            .text_color(theme::text_secondary())
                            .child(self.tr(if legacy {
                                "storage-legacy-description"
                            } else {
                                "storage-private-label"
                            })),
                    ),
            )
            .child(
                components::icon_button(
                    "storage-help",
                    Symbol::Help,
                    self.tr("storage-guide-title"),
                )
                .ghost()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.show_storage_guide = !this.show_storage_guide;
                    cx.notify();
                })),
            )
            .when(ready && !legacy, |bar| {
                bar.child(
                    components::button(
                        "storage-upload",
                        self.tr("storage-channel-upload-action"),
                        Some(IconName::Plus),
                        true,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.request_vault_unlock(UnlockIntent::Upload, cx)
                    })),
                )
            });
        let body = if ready {
            let locked = self.storage_view == StorageView::Files && self.vault_locked;
            let overview = div()
                .flex_none()
                .flex()
                .flex_col()
                .gap_3()
                .when(self.show_storage_guide, |body| {
                    body.child(self.storage_guide(true, cx))
                })
                .when(!legacy, |body| body.child(self.render_storage_controls(cx)));
            let content = if locked {
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .debug_selector(|| "storage-locked-viewport".into())
                    .child(
                        div().flex_1().min_h_0().overflow_y_scrollbar().child(
                            div()
                                .w_full()
                                .max_w(px(960.0))
                                .mx_auto()
                                .py_2()
                                .flex()
                                .flex_col()
                                .gap_3()
                                .child(overview)
                                .child(self.storage_locked_state(cx)),
                        ),
                    )
                    .into_any_element()
            } else {
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .flex_none()
                            .max_h(px(270.0))
                            .overflow_y_scrollbar()
                            .child(overview),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .child(self.render_telegram_channels(layout, cx)),
                    )
                    .into_any_element()
            };
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    TabBar::new("storage-modes")
                        .segmented()
                        .selected_index(usize::from(self.storage_view == StorageView::RawFiles))
                        .child(Tab::new().label(self.tr("storage-channel-teleark-files")))
                        .child(Tab::new().label(self.tr("storage-channel-telegram-files")))
                        .on_click(cx.listener(|this, index: &usize, _, cx| {
                            this.select_storage(
                                if *index == 0 {
                                    StorageView::Files
                                } else {
                                    StorageView::RawFiles
                                },
                                cx,
                            )
                        })),
                )
                .child(content)
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_h_0()
                .overflow_y_scrollbar()
                .child(
                    div()
                        .max_w(px(720.0))
                        .mx_auto()
                        .py_4()
                        .flex()
                        .flex_col()
                        .gap_4()
                        .child(
                            div()
                                .flex()
                                .items_start()
                                .gap_3()
                                .child(components::app_mark(38.0))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .flex()
                                        .flex_col()
                                        .gap_2()
                                        .child(
                                            div()
                                                .text_size(px(18.0))
                                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                                .text_color(theme::text_primary())
                                                .child(self.tr("storage-setup-title")),
                                        )
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(theme::text_secondary())
                                                .child(self.tr("storage-setup-description")),
                                        ),
                                ),
                        )
                        .when(!self.dialogs.ready && self.dialogs.has_activity(), |body| {
                            body.child(self.render_dialog_activity(cx))
                        })
                        .when(
                            self.storage_loading || self.storage_error.is_some(),
                            |body| body.child(self.render_storage_activity(cx)),
                        )
                        .child(self.storage_guide(false, cx)),
                )
                .into_any_element()
        };
        div()
            .flex_1()
            .h_full()
            .min_h_0()
            .overflow_hidden()
            .min_w_0()
            .flex()
            .flex_col()
            .px(px(layout.content_padding()))
            .pb(px(layout.content_padding()))
            .child(header)
            .child(body)
            .into_any_element()
    }

    fn render_storage_activity(&self, cx: &mut Context<Self>) -> AnyElement {
        let waiting = self.storage_waiting_for_retry();
        let (state, tone, icon) = if self.storage_loading {
            ("activity-state-running", Tone::Blue, IconName::LoaderCircle)
        } else if waiting {
            ("activity-state-waiting", Tone::Amber, IconName::Redo2)
        } else if self.storage_error.is_some() {
            (
                "activity-state-failed",
                Tone::Amber,
                IconName::TriangleAlert,
            )
        } else {
            (
                "activity-state-complete",
                Tone::Green,
                IconName::CircleCheck,
            )
        };
        components::activity_card(
            self.tr("storage-activity-title"),
            self.tr(state),
            tone,
            icon,
        )
        .debug_selector(|| "storage-activity-card".into())
        .child(
            div()
                .px_4()
                .pb_4()
                .text_sm()
                .text_color(theme::text_secondary())
                .child(self.tr(if self.storage_loading {
                    "storage-loading"
                } else if waiting {
                    "storage-activity-retry-wait"
                } else if self.storage_error.is_some() {
                    "storage-setup-error"
                } else {
                    self.storage_notice.unwrap_or("storage-auto-found")
                }))
                .when_some(self.storage_error, |body, kind| {
                    body.child(
                        div()
                            .mt_3()
                            .p_3()
                            .rounded(theme::RADIUS_MEDIUM)
                            .bg(theme::canvas())
                            .border_1()
                            .border_color(theme::border_subtle())
                            .child(
                                div()
                                    .mb_1()
                                    .text_xs()
                                    .text_color(theme::text_muted())
                                    .child(self.tr("activity-last-response")),
                            )
                            .child(self.tr(match kind {
                                teleark_core::ApplicationErrorKind::Server => {
                                    "telegram-error-server"
                                }
                                teleark_core::ApplicationErrorKind::Network => {
                                    "telegram-error-network"
                                }
                                teleark_core::ApplicationErrorKind::Authorization => {
                                    "telegram-error-authorization"
                                }
                                teleark_core::ApplicationErrorKind::Conflict => {
                                    "storage-identity-conflict"
                                }
                                teleark_core::ApplicationErrorKind::StorageConfigurationUnsafe => {
                                    "storage-health-unsafe"
                                }
                                teleark_core::ApplicationErrorKind::StorageAccessDenied => {
                                    "storage-health-access"
                                }
                                teleark_core::ApplicationErrorKind::StorageIdentityUnsupported => {
                                    "storage-health-unsupported"
                                }
                                teleark_core::ApplicationErrorKind::StorageIdentityDamaged => {
                                    "storage-health-repair"
                                }
                                teleark_core::ApplicationErrorKind::PermissionDenied => {
                                    "storage-identity-invalid"
                                }
                                _ => "storage-setup-error",
                            })),
                    )
                }),
        )
        .when(
            !self.storage_loading && !waiting && self.storage_error.is_some(),
            |card| {
                card.child(
                    div()
                        .px_4()
                        .py_3()
                        .border_t_1()
                        .border_color(theme::border_subtle())
                        .bg(theme::canvas())
                        .flex()
                        .justify_end()
                        .child(
                            components::button(
                                "storage-activity-retry",
                                self.tr("common-retry"),
                                Some(IconName::Redo2),
                                true,
                            )
                            .debug_selector(|| "storage-activity-retry".into())
                            .on_click(cx.listener(|app, _, _, cx| app.refresh_storage_channel(cx))),
                        ),
                )
            },
        )
        .into_any_element()
    }

    fn render_storage_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let health = self.storage_status.health();
        let repair = health.is_some_and(|health| health.needs_repair());
        let confirming_repair = self.storage_confirmation == Some(StorageAction::Repair);
        let show_activity = self.storage_loading
            || self.storage_error.is_some()
            || self.storage_waiting_for_retry()
            || self
                .storage_notice
                .is_some_and(|id| !matches!(id, "storage-auto-found" | "storage-auto-created"));
        components::card()
            .flex_none()
            .overflow_hidden()
            .debug_selector(|| "storage-management-card".into())
            .when(repair, |card| card.border_color(theme::amber()))
            .child(
                div()
                    .p_4()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_3()
                    .when(repair, |header| header.bg(theme::amber_soft()))
                    .child(
                        Icon::new(if repair {
                            IconName::TriangleAlert
                        } else {
                            IconName::CircleCheck
                        })
                        .size(px(20.0))
                        .text_color(if repair {
                            theme::amber()
                        } else {
                            theme::green()
                        }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(180.0))
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(self.tr(if repair {
                                "storage-repair-title"
                            } else {
                                "storage-connected-title"
                            })),
                    )
                    .when(
                        repair && !self.storage_loading && !confirming_repair,
                        |header| {
                            header.child(
                                components::button(
                                    "repair-storage",
                                    self.tr("storage-repair-action"),
                                    None,
                                    true,
                                )
                                .debug_selector(|| "storage-repair-action".into())
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.storage_confirmation = Some(StorageAction::Repair);
                                        cx.notify();
                                    },
                                )),
                            )
                        },
                    ),
            )
            .child(
                div()
                    .px_4()
                    .py_3()
                    .bg(theme::canvas())
                    .border_b_1()
                    .border_color(theme::border_subtle())
                    .flex()
                    .flex_col()
                    .gap_2()
                    .debug_selector(|| "storage-channel-location".into())
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(self.tr("storage-location-title")),
                    )
                    .when_some(self.storage_status.channel(), |body, channel| {
                        body.child(
                            div().text_sm().child(
                                self.tr_with(
                                    "storage-location-target",
                                    teleark_i18n::MessageArgs::new()
                                        .with("title", channel.name.clone())
                                        .with("id", channel.id.to_string()),
                                ),
                            ),
                        )
                    })
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme::text_secondary())
                            .child(self.tr(if repair {
                                "storage-location-bound"
                            } else {
                                "storage-location-method"
                            })),
                    ),
            )
            .when(repair, |card| {
                card.child(
                    div()
                        .px_4()
                        .py_3()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .text_sm()
                        .child(div().text_color(theme::text_primary()).child(self.tr(
                            match health {
                                Some(teleark_runtime::StorageChannelHealth::IdentityUnpinned) => {
                                    "storage-repair-reason-unpinned"
                                }
                                Some(teleark_runtime::StorageChannelHealth::IdentityInvalid) => {
                                    "storage-repair-reason-invalid"
                                }
                                _ => "storage-repair-reason-missing",
                            },
                        )))
                        .child(
                            div()
                                .text_color(theme::text_secondary())
                                .child(self.tr("storage-repair-explanation")),
                        )
                        .children(
                            [
                                "storage-repair-step-message",
                                "storage-repair-step-pin",
                                "storage-repair-step-description",
                            ]
                            .into_iter()
                            .enumerate()
                            .map(|(index, id)| {
                                div()
                                    .flex()
                                    .items_start()
                                    .gap_2()
                                    .child(
                                        div()
                                            .flex_none()
                                            .text_color(theme::text_muted())
                                            .child(format!("{}.", index + 1)),
                                    )
                                    .child(div().flex_1().min_w_0().child(self.tr(id)))
                            }),
                        )
                        .child(
                            div()
                                .mt_1()
                                .text_xs()
                                .text_color(theme::text_secondary())
                                .child(self.tr("storage-repair-scope")),
                        ),
                )
            })
            .when(confirming_repair, |card| {
                card.child(self.storage_confirmation_controls(StorageAction::Repair, cx))
            })
            .when(
                show_activity && self.storage_maintenance.is_none(),
                |card| card.child(self.render_storage_activity(cx)),
            )
            .when(self.storage_maintenance.is_some(), |card| {
                card.child(self.render_storage_maintenance(cx))
                    .when(self.storage_error.is_some(), |card| {
                        card.child(self.render_storage_activity(cx))
                    })
            })
            .child(
                div()
                    .px_4()
                    .py_3()
                    .border_t_1()
                    .border_color(theme::border_subtle())
                    .flex()
                    .flex_col()
                    .gap_2()
                    .debug_selector(|| "storage-channel-notifications".into())
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(self.tr("storage-notifications-title")),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme::text_secondary())
                            .child(self.tr("storage-notifications-explanation")),
                    ),
            )
            .child(
                div()
                    .px_4()
                    .py_3()
                    .border_t_1()
                    .border_color(theme::border_subtle())
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(160.0))
                            .text_xs()
                            .text_color(theme::text_secondary())
                            .child(self.tr("storage-channel-options")),
                    )
                    .child(
                        components::button(
                            "recheck-storage",
                            self.tr("storage-recheck-action"),
                            Some(IconName::Redo2),
                            false,
                        )
                        .disabled(self.storage_loading)
                        .on_click(cx.listener(|this, _, _, cx| this.refresh_storage_channel(cx))),
                    )
                    .child(
                        components::button(
                            "archive-storage",
                            self.tr("storage-archive-action"),
                            None,
                            false,
                        )
                        .disabled(self.storage_loading)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.storage_confirmation = Some(StorageAction::Archive);
                            cx.notify();
                        })),
                    ),
            )
            .when(
                self.storage_confirmation == Some(StorageAction::Archive),
                |card| card.child(self.storage_confirmation_controls(StorageAction::Archive, cx)),
            )
            .into_any_element()
    }

    fn storage_confirmation_controls(
        &self,
        action: StorageAction,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .p_4()
            .border_t_1()
            .border_color(theme::border_subtle())
            .bg(theme::blue_soft())
            .debug_selector(|| "storage-maintenance-confirmation".into())
            .child(
                div()
                    .text_sm()
                    .child(self.tr(if action == StorageAction::Repair {
                        "storage-repair-confirm"
                    } else {
                        "storage-archive-confirm"
                    })),
            )
            .child(
                div()
                    .mt_3()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        components::button(
                            "confirm-storage-maintenance",
                            self.tr(if action == StorageAction::Repair {
                                "storage-repair-confirm-action"
                            } else {
                                "storage-archive-action"
                            }),
                            None,
                            true,
                        )
                        .disabled(self.storage_loading)
                        .debug_selector(|| "storage-confirm-action".into())
                        .on_click(
                            cx.listener(|this, _, _, cx| this.confirm_storage_maintenance(cx)),
                        ),
                    )
                    .child(
                        components::button(
                            "dismiss-storage-maintenance",
                            self.tr("common-cancel"),
                            None,
                            false,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.storage_confirmation = None;
                            cx.notify();
                        })),
                    ),
            )
            .into_any_element()
    }

    pub(crate) fn render_storage_maintenance(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(progress) = &self.storage_maintenance else {
            return div().into_any_element();
        };
        let state = progress.snapshot();
        let phase = storage_phase_id(state.phase);
        let duration = if state.finished {
            state.last_activity.duration_since(state.phase_since)
        } else {
            state.phase_since.elapsed()
        };
        div()
            .max_h(px(120.0))
            .overflow_y_scrollbar()
            .px_3()
            .py_2()
            .bg(theme::canvas())
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .child(self.tr(phase))
                    .child(
                        div().text_xs().text_color(theme::text_secondary()).child(
                            self.tr_with(
                                "storage-maintenance-time",
                                teleark_i18n::MessageArgs::new()
                                    .with("seconds", duration.as_secs().to_string())
                                    .with(
                                        "idle",
                                        state.last_activity.elapsed().as_secs().to_string(),
                                    ),
                            ),
                        ),
                    ),
            )
            .when(!state.finished && self.storage_loading, |row| {
                row.child(
                    components::button(
                        "cancel-storage-maintenance",
                        self.tr("common-cancel"),
                        None,
                        false,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(progress) = &this.storage_maintenance {
                            progress.cancel();
                        }
                        cx.notify();
                    })),
                )
            })
            .child(
                div().w_full().text_xs().child(
                    state
                        .timeline
                        .iter()
                        .map(|(phase, millis)| {
                            format!("{} · {}s", self.tr(storage_phase_id(*phase)), millis / 1000)
                        })
                        .collect::<Vec<_>>()
                        .join(" → "),
                ),
            )
            .when(state.omitted > 0, |row| {
                row.child(div().text_xs().child(self.tr_with(
                    "storage-maintenance-omitted",
                    teleark_i18n::MessageArgs::new().with("count", state.omitted.to_string()),
                )))
            })
            .into_any_element()
    }

    fn storage_locked_state(&self, cx: &mut Context<Self>) -> AnyElement {
        // Intrinsic height: never shrink the icon or place an action outside its card.
        components::card()
            .flex_none()
            .p_4()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_4()
            .debug_selector(|| "storage-locked-card".into())
            .child(
                div()
                    .size(px(44.0))
                    .flex_none()
                    .rounded(theme::RADIUS_MEDIUM)
                    .bg(theme::blue_soft())
                    .flex()
                    .items_center()
                    .justify_center()
                    .debug_selector(|| "storage-locked-icon".into())
                    .child(
                        Icon::new(Symbol::Lock)
                            .size(px(22.0))
                            .text_color(theme::blue()),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(220.0))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(self.tr("storage-locked-title")),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme::text_secondary())
                            .child(self.tr("storage-channel-managed-vault-locked")),
                    ),
            )
            .child(
                components::button("storage-unlock", self.tr("vault-unlock-action"), None, true)
                    .icon(Symbol::Lock)
                    .debug_selector(|| "storage-unlock-action".into())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.request_vault_unlock(UnlockIntent::Browse, cx)
                    })),
            )
            .into_any_element()
    }

    fn storage_guide(&self, compact: bool, cx: &mut Context<Self>) -> AnyElement {
        let steps = [
            (
                Icon::new(Symbol::Lock),
                "storage-guide-private-title",
                "storage-guide-private-body",
            ),
            (
                Icon::new(Symbol::Layers),
                "storage-guide-files-title",
                "storage-guide-files-body",
            ),
            (
                Icon::new(IconName::Eye),
                "storage-guide-raw-title",
                "storage-guide-raw-body",
            ),
            (
                Icon::new(IconName::Asterisk),
                "storage-guide-key-title",
                "storage-guide-key-body",
            ),
        ];
        components::card()
            .p_4()
            .bg(theme::blue_pale())
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(self.tr("storage-guide-title")),
                    )
                    .when(compact, |row| {
                        row.child(
                            components::button(
                                "storage-guide-dismiss",
                                self.tr("storage-guide-done"),
                                None,
                                false,
                            )
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_storage_guide = false;
                                cx.notify();
                            })),
                        )
                    }),
            )
            .child(
                div()
                    .mt_3()
                    .grid()
                    .grid_cols(if compact { 2 } else { 1 })
                    .gap_4()
                    .children(steps.into_iter().map(|(icon, title, body)| {
                        div()
                            .flex()
                            .items_start()
                            .gap_3()
                            .child(Icon::new(icon).size(px(17.0)).text_color(theme::blue()))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                            .child(self.tr(title)),
                                    )
                                    .child(
                                        div()
                                            .mt_1()
                                            .text_xs()
                                            .text_color(theme::text_secondary())
                                            .child(self.tr(body)),
                                    ),
                            )
                    })),
            )
            .into_any_element()
    }
}

impl TeleArkApp {
    pub(crate) fn render_storage_managed(
        &self,
        layout: LayoutPolicy,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let query = self.search_input.read(cx).value().to_lowercase();
        let rows = self
            .managed_projection
            .borrow_mut()
            .rows(&self.managed_vault_files, &query);
        let source = self.managed_vault_files.clone();
        let selected = self
            .managed_projection
            .borrow()
            .selected(self.selected_telegram_message_id)
            .cloned();
        let count = rows.len();
        let list = gpui_kit::uniform_list(
            "managed-files",
            count,
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                range
                    .filter_map(|index| rows.get(index).and_then(|index| source.get(*index)))
                    .map(|file| {
                        let message_id = file.manifest_message_id;
                        let package_id = file.package_numeric_id;
                        let local = this
                            .selected_chat_id
                            .and_then(|chat_id| {
                                this.local_download_for_source(
                                    chat_id,
                                    None,
                                    Some(&file.package_id),
                                )
                            })
                            .cloned();
                        gpui_kit::base::Button::new(("managed-file", package_id))
                            .accessibility_label(file.logical_name.clone())
                            .w_full()
                            .h(px(60.0))
                            .px_4()
                            .flex()
                            .items_center()
                            .gap_3()
                            .border_b_1()
                            .border_color(theme::border_subtle())
                            .when(
                                this.selected_telegram_message_id == Some(message_id),
                                |row| row.bg(theme::blue_pale()),
                            )
                            .child(
                                Icon::new(IconName::File)
                                    .size(px(22.0))
                                    .text_color(theme::blue()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_left()
                                    .child(
                                        div()
                                            .truncate()
                                            .text_size(px(13.0))
                                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                                            .child(file.logical_name.clone()),
                                    )
                                    .child(
                                        div()
                                            .mt_1()
                                            .text_xs()
                                            .text_color(theme::text_muted())
                                            .child(format!(
                                            "{} · {}",
                                            if file.health
                                                == teleark_runtime::VaultFileHealth::KeyUnavailable
                                            {
                                                this.tr("vault-health-unknown-size").to_string()
                                            } else {
                                                teleark_i18n::format::format_bytes(
                                                    this.locale(),
                                                    file.size_bytes,
                                                )
                                            },
                                            this.tr(vault_health_id(file.health)),
                                        )),
                                    ),
                            )
                            .when_some(local.as_ref(), |row, observation| {
                                row.child(components::badge(
                                    this.local_presence_label(Some(observation.presence)),
                                    if observation.presence
                                        == teleark_runtime::LocalFilePresence::Present
                                    {
                                        Tone::Green
                                    } else {
                                        Tone::Amber
                                    },
                                ))
                            })
                            .child(
                                components::icon_button(
                                    ("managed-download", package_id),
                                    IconName::ArrowDown,
                                    this.tr("storage-channel-download-restored-action"),
                                )
                                .ghost()
                                .disabled(matches!(
                                    file.health,
                                    teleark_runtime::VaultFileHealth::KeyUnavailable
                                        | teleark_runtime::VaultFileHealth::MissingParts
                                        | teleark_runtime::VaultFileHealth::MissingManifest
                                        | teleark_runtime::VaultFileHealth::InvalidManifest
                                ))
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.download_managed_vault_file(package_id, cx);
                                    },
                                )),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if this.selected_telegram_message_id != Some(message_id) {
                                    this.raw_detail_scroll
                                        .set_offset(gpui_kit::point(px(0.0), px(0.0)));
                                }
                                this.selected_telegram_message_id = Some(message_id);
                                this.show_channel_detail = true;
                                cx.notify();
                            }))
                            .into_any_element()
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .w_full()
        .flex_1()
        .min_h_0();
        components::card()
            .h_full()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .relative()
            .overflow_hidden()
            .child(
                div()
                    .h(px(44.0))
                    .px_4()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .child(self.tr("storage-channel-managed-title")),
                    )
                    .child(
                        components::icon_button(
                            "managed-refresh",
                            IconName::Redo2,
                            self.tr("telegram-files-refresh-action"),
                        )
                        .ghost()
                        .disabled(self.managed_scan_loading)
                        .on_click(
                            cx.listener(|this, _, _, cx| this.refresh_managed_vault_files(cx)),
                        ),
                    ),
            )
            .when_some(self.managed_health_checked, |body, count| {
                body.child(div().p_3().text_xs().child(self.tr_with(
                    "vault-health-check-summary",
                    teleark_i18n::MessageArgs::new().with("count", count.to_string()),
                )))
            })
            .when(self.managed_catalog_limited, |body| {
                body.child(
                    div()
                        .p_3()
                        .text_xs()
                        .bg(theme::amber_soft())
                        .child(self.tr("vault-health-scope-limited")),
                )
            })
            .when(count > 0, |body| body.child(list))
            .when(count == 0, |body| {
                body.child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap_3()
                        .p_5()
                        .child(
                            Icon::new(IconName::FolderOpen)
                                .size(px(36.0))
                                .text_color(theme::blue()),
                        )
                        .child(
                            div()
                                .max_w(px(420.0))
                                .text_center()
                                .text_sm()
                                .text_color(theme::text_secondary())
                                .child(self.tr(if self.managed_catalog_pending {
                                    "managed-catalog-syncing"
                                } else if self.managed_scan_loading {
                                    "storage-loading"
                                } else {
                                    "storage-channel-managed-empty"
                                })),
                        ),
                )
            })
            .when_some(
                super::settings::vault_activity_message(self),
                |body, (message, tone)| {
                    body.child(
                        div()
                            .px_4()
                            .py_2()
                            .text_xs()
                            .text_color(tone.foreground())
                            .child(message),
                    )
                },
            )
            .child(
                div()
                    .px_4()
                    .py_2()
                    .border_t_1()
                    .border_color(theme::border())
                    .text_xs()
                    .text_color(theme::text_muted())
                    .when(self.page == Page::Storage, |footer| {
                        footer.child(self.tr("managed-catalog-coverage"))
                    })
                    .child(
                        self.tr_with(
                            "storage-scan-summary",
                            teleark_i18n::MessageArgs::new()
                                .with(
                                    "count",
                                    teleark_i18n::format::format_integer(
                                        self.locale(),
                                        count as u64,
                                    ),
                                )
                                .with(
                                    "rejected",
                                    teleark_i18n::format::format_integer(
                                        self.locale(),
                                        self.managed_vault_rejected as u64,
                                    ),
                                ),
                        ),
                    ),
            )
            .when_some(
                selected.filter(|_| self.show_channel_detail),
                |body, selected| {
                    body.child(
                        components::inspector_panel("managed-file-inspector", 340.0)
                            .absolute()
                            .right_0()
                            .top_0()
                            .bottom_0()
                            .shadow_lg()
                            .child(
                                div().px_3().py_2().flex().justify_end().child(
                                    components::icon_button(
                                        "managed-close-detail",
                                        IconName::Close,
                                        self.tr("action-close-details"),
                                    )
                                    .ghost()
                                    .on_click(cx.listener(
                                        |this, _, _, cx| {
                                            this.show_channel_detail = false;
                                            cx.notify();
                                        },
                                    )),
                                ),
                            )
                            .child(self.render_managed_package_detail(Some(&selected), layout, cx)),
                    )
                },
            )
            .into_any_element()
    }
}

fn storage_phase_id(phase: StorageMaintenancePhase) -> &'static str {
    match phase {
        StorageMaintenancePhase::Checking => "storage-phase-checking",
        StorageMaintenancePhase::FindingRecord => "storage-phase-finding",
        StorageMaintenancePhase::Repairing => "storage-phase-repairing",
        StorageMaintenancePhase::Pinning => "storage-phase-pinning",
        StorageMaintenancePhase::Updating => "storage-phase-updating",
        StorageMaintenancePhase::Verifying => "storage-phase-verifying",
        StorageMaintenancePhase::Muting => "storage-phase-muting",
        StorageMaintenancePhase::Archiving => "storage-phase-archiving",
        StorageMaintenancePhase::Completed => "storage-phase-completed",
    }
}

pub(crate) fn vault_health_id(health: teleark_runtime::VaultFileHealth) -> &'static str {
    use teleark_runtime::VaultFileHealth;
    match health {
        VaultFileHealth::Unchecked => "vault-health-unchecked",
        VaultFileHealth::Present => "vault-health-present",
        VaultFileHealth::MissingParts => "vault-health-missing-parts",
        VaultFileHealth::MissingManifest => "vault-health-missing-manifest",
        VaultFileHealth::KeyUnavailable => "vault-health-key",
        VaultFileHealth::InvalidManifest => "vault-health-invalid",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use teleark_i18n::{Localizer, SupportedLocale};
    use teleark_runtime::{AppearancePreference, StorageChannelHealth, StorageChannelStatus};

    #[gpui_kit::test]
    fn locked_repair_cards_keep_children_inside_and_actions_reachable(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, Page::Storage);
        for size in [(900.0, 600.0), (1440.0, 900.0)] {
            cx.simulate_resize(gpui_kit::size(px(size.0), px(size.1)));
            for locale in [
                SupportedLocale::EnUs,
                SupportedLocale::ZhCn,
                SupportedLocale::JaJp,
            ] {
                for appearance in [AppearancePreference::Light, AppearancePreference::Dark] {
                    cx.update(|window, cx| {
                        app.update(cx, |app, cx| {
                            app.localizer = Localizer::new(locale).expect("catalog");
                            theme::apply_appearance(appearance, window, cx);
                            let channel = app
                                .storage_status
                                .channel()
                                .expect("preview channel")
                                .clone();
                            app.storage_status = StorageChannelStatus::Degraded {
                                channel,
                                health: StorageChannelHealth::IdentityMissing,
                            };
                            app.storage_notice = Some("storage-auto-found");
                            app.storage_loading = false;
                            app.storage_confirmation = None;
                            app.unlock_intent = None;
                            app.vault_locked = true;
                            app.vault_status.locked = true;
                            app.vault_status.active_key_locked = true;
                            cx.notify();
                        })
                    });
                    cx.run_until_parked();
                    assert!(cx.debug_bounds("storage-channel-location").is_some());
                    assert!(cx.debug_bounds("storage-channel-notifications").is_some());
                    assert!(
                        cx.debug_bounds("storage-activity-card").is_none(),
                        "stale discovery success must not contradict repair"
                    );
                    let viewport = cx
                        .debug_bounds("storage-locked-viewport")
                        .expect("scroll viewport");
                    assert!(
                        viewport.bottom() <= px(size.1),
                        "scroll viewport stays inside the window"
                    );
                    cx.simulate_event(gpui_kit::ScrollWheelEvent {
                        position: viewport.center(),
                        delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.0), px(2000.0))),
                        ..Default::default()
                    });
                    cx.run_until_parked();
                    let review = cx
                        .debug_bounds("storage-repair-action")
                        .expect("review changes");
                    assert!(review.top() >= viewport.top() && review.bottom() <= viewport.bottom());
                    cx.simulate_click(review.center(), gpui_kit::Modifiers::default());
                    cx.run_until_parked();
                    app.update(cx, |app, _| {
                        assert_eq!(app.storage_confirmation, Some(StorageAction::Repair))
                    });
                    assert!(
                        cx.debug_bounds("storage-maintenance-confirmation")
                            .is_some()
                    );
                    cx.simulate_event(gpui_kit::ScrollWheelEvent {
                        position: viewport.center(),
                        delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.0), px(-2000.0))),
                        ..Default::default()
                    });
                    cx.run_until_parked();
                    let card = cx.debug_bounds("storage-locked-card").expect("locked card");
                    let icon = cx.debug_bounds("storage-locked-icon").expect("lock icon");
                    let unlock = cx
                        .debug_bounds("storage-unlock-action")
                        .expect("unlock action");
                    for child in [icon, unlock] {
                        assert!(
                            child.top() >= card.top() && child.bottom() <= card.bottom(),
                            "child must stay within card vertically"
                        );
                        assert!(
                            child.left() >= card.left() && child.right() <= card.right(),
                            "child must stay within card horizontally"
                        );
                    }
                    assert_eq!(icon.size.height, px(44.0), "lock icon must not shrink");
                    assert!(
                        unlock.top() >= viewport.top() && unlock.bottom() <= viewport.bottom(),
                        "locale={locale:?} size={size:?} viewport={viewport:?} card={card:?} unlock={unlock:?}"
                    );
                    cx.simulate_click(unlock.center(), gpui_kit::Modifiers::default());
                    cx.run_until_parked();
                    app.update(cx, |app, _| {
                        assert!(matches!(app.unlock_intent, Some(UnlockIntent::Browse)))
                    });
                }
            }
        }
    }
}
