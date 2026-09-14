use crate::{
    app::{Page, SettingsSection, TeleArkApp, proxy::ProxyAction},
    components::{self, Tone},
    layout::LayoutPolicy,
    theme,
};
use gpui_kit::component::{
    Disableable as _, Icon, IconName,
    button::ButtonVariants as _,
    input::{Input, InputState},
};
use gpui_kit::{
    AnyElement, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Styled as _, div, prelude::FluentBuilder as _, px,
};
use teleark_i18n::{MessageArgs, format::format_duration_millis};
use teleark_runtime::{NetworkPhase, ProxyFailure, ProxyProtocol};

pub(crate) fn phase_id(phase: NetworkPhase) -> &'static str {
    match phase {
        NetworkPhase::Direct => "proxy-phase-direct",
        NetworkPhase::ProxyReady => "proxy-phase-ready",
        NetworkPhase::Applying => "proxy-phase-applying",
        NetworkPhase::Connecting => "proxy-phase-connecting",
        NetworkPhase::Connected => "proxy-phase-connected",
        NetworkPhase::TestQueued => "proxy-phase-test-queued",
        NetworkPhase::Testing => "proxy-phase-testing",
        NetworkPhase::TestSucceeded => "proxy-phase-tested",
        NetworkPhase::Blocked(reason) => match reason {
            ProxyFailure::Unreachable => "proxy-error-unreachable",
            ProxyFailure::Timeout => "proxy-error-timeout",
            ProxyFailure::Authentication => "proxy-error-authentication",
            ProxyFailure::Rejected => "proxy-error-rejected",
            ProxyFailure::Protocol => "proxy-error-protocol",
            ProxyFailure::Disconnected => "proxy-error-disconnected",
            ProxyFailure::Configuration => "proxy-error-configuration",
            ProxyFailure::Persistence => "proxy-error-persistence",
            ProxyFailure::Capacity => "proxy-error-capacity",
            ProxyFailure::Cancelled => "proxy-test-cancelled",
        },
    }
}

impl TeleArkApp {
    fn network_message_id(&self) -> &'static str {
        if self.proxy.load_failed {
            "proxy-load-failed"
        } else if let Some(snapshot) = &self.proxy.snapshot {
            phase_id(snapshot.phase)
        } else if self.proxy.action == ProxyAction::Applying {
            "proxy-phase-applying"
        } else if self.proxy.action == ProxyAction::Testing {
            "proxy-phase-testing"
        } else if self.proxy.action == ProxyAction::Failed {
            "proxy-error-persistence"
        } else {
            "proxy-phase-direct"
        }
    }

    fn proxy_timing(&self) -> gpui_kit::SharedString {
        let (phase, activity) = self
            .proxy
            .snapshot
            .as_ref()
            .map_or((self.proxy.started, self.proxy.started), |s| {
                (s.phase_started, s.last_activity)
            });
        self.tr_with(
            "proxy-fixed-timing",
            MessageArgs::new()
                .with("phase", self.sync_event_time(phase).to_string())
                .with("activity", self.sync_event_time(activity).to_string()),
        )
    }

    pub(crate) fn render_network_banner(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("network-proxy-banner")
            .debug_selector(|| "network-proxy-banner".into())
            .px_4()
            .py_2()
            .bg(theme::amber_soft())
            .flex()
            .items_center()
            .gap_3()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .text_color(theme::text_primary())
                    .child(div().child(self.tr(self.network_message_id())))
                    .when(self.proxy.waiting(), |body| {
                        body.child(div().text_xs().child(self.proxy_timing()))
                    }),
            )
            .child(
                components::button(
                    "network-proxy-settings",
                    self.tr("proxy-settings-title"),
                    Some(IconName::Settings),
                    false,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.page = Page::Settings;
                    this.settings_section = SettingsSection::Network;
                    cx.notify();
                })),
            )
            .into_any_element()
    }

    fn proxy_input(&self, label: &'static str, input: &Entity<InputState>) -> AnyElement {
        div()
            .min_w_0()
            .child(
                div()
                    .mb_1()
                    .text_xs()
                    .text_color(theme::text_secondary())
                    .child(self.tr(label)),
            )
            .child(
                Input::new(input)
                    .h(theme::FORM_CONTROL_HEIGHT)
                    .disabled(self.proxy.action.busy()),
            )
            .into_any_element()
    }

    pub(crate) fn render_proxy_settings(
        &self,
        _layout: LayoutPolicy,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let busy = self.proxy.action.busy();
        let mut card = components::card()
            .rounded(theme::RADIUS_LARGE)
            .p_5()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .size(px(36.0))
                            .flex_none()
                            .rounded(theme::RADIUS_MEDIUM)
                            .bg(theme::blue_pale())
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                Icon::new(IconName::Globe)
                                    .size(px(18.0))
                                    .text_color(theme::blue()),
                            ),
                    )
                    .child(components::section_title(self.tr("proxy-settings-title"))),
            )
            .child(
                div()
                    .mt_3()
                    .text_size(px(13.0))
                    .line_height(px(20.0))
                    .text_color(theme::text_secondary())
                    .child(self.tr("proxy-settings-description")),
            )
            .child(
                div()
                    .mt_4()
                    .flex()
                    .p_1()
                    .gap_1()
                    .rounded(theme::RADIUS_MEDIUM)
                    .bg(theme::sidebar())
                    .child(
                        components::button("proxy-disable", self.tr("proxy-disable"), None, false)
                            .ghost()
                            .flex_1()
                            .h(px(30.0))
                            .when(!self.proxy.enabled, |b| b.bg(theme::surface()).shadow_sm())
                            .disabled(busy || self.proxy.load_failed)
                            .debug_selector(|| "proxy-disable".into())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.proxy.enabled = false;
                                cx.notify();
                            })),
                    )
                    .child(
                        components::button("proxy-enable", self.tr("proxy-enable"), None, false)
                            .ghost()
                            .flex_1()
                            .h(px(30.0))
                            .when(self.proxy.enabled, |b| b.bg(theme::surface()).shadow_sm())
                            .disabled(busy || self.proxy.load_failed)
                            .debug_selector(|| "proxy-enable".into())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.proxy.enabled = true;
                                cx.notify();
                            })),
                    ),
            );
        if self.proxy.enabled {
            card = card
                .child(
                    div()
                        .mt_4()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .child(
                            components::button(
                                "proxy-socks5",
                                self.tr("proxy-protocol-socks5"),
                                None,
                                false,
                            )
                            .ghost()
                            .h(px(28.0))
                            .text_size(px(12.0))
                            .when(self.proxy.protocol == ProxyProtocol::Socks5, |b| {
                                b.bg(theme::blue_pale()).text_color(theme::blue())
                            })
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.proxy.protocol = ProxyProtocol::Socks5;
                                cx.notify();
                            })),
                        )
                        .child(
                            components::button(
                                "proxy-http",
                                self.tr("proxy-protocol-http"),
                                None,
                                false,
                            )
                            .ghost()
                            .h(px(28.0))
                            .text_size(px(12.0))
                            .when(self.proxy.protocol == ProxyProtocol::HttpConnect, |b| {
                                b.bg(theme::blue_pale()).text_color(theme::blue())
                            })
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.proxy.protocol = ProxyProtocol::HttpConnect;
                                cx.notify();
                            })),
                        ),
                )
                .child(
                    div()
                        .mt_4()
                        .grid()
                        .grid_cols(2)
                        .gap_3()
                        .child(self.proxy_input("proxy-host", &self.proxy.host))
                        .child(self.proxy_input("proxy-port", &self.proxy.port))
                        .child(self.proxy_input("proxy-username", &self.proxy.username))
                        .child(self.proxy_input("proxy-password", &self.proxy.password)),
                )
                .child(
                    div()
                        .mt_2()
                        .text_xs()
                        .text_color(theme::text_secondary())
                        .child(self.tr("proxy-address-note")),
                );
        }
        card = card
            .child(
                div()
                    .mt_4()
                    .pt_3()
                    .border_t_1()
                    .border_color(theme::border_subtle())
                    .text_size(px(12.0))
                    .line_height(px(18.0))
                    .text_color(theme::text_secondary())
                    .child(self.tr("proxy-apply-note")),
            )
            .when(self.proxy.invalid, |card| {
                card.child(
                    div()
                        .mt_3()
                        .text_sm()
                        .text_color(theme::red())
                        .child(self.tr("proxy-invalid")),
                )
            })
            .child(
                div()
                    .mt_4()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        components::button("proxy-apply", self.tr("proxy-apply"), None, true)
                            .disabled(busy || self.proxy.load_failed)
                            .debug_selector(|| "proxy-apply".into())
                            .on_click(cx.listener(|this, _, _, cx| this.apply_proxy(cx))),
                    )
                    .child(
                        components::button("proxy-test", self.tr("proxy-test"), None, false)
                            .ghost()
                            .disabled(
                                busy || !self
                                    .proxy
                                    .snapshot
                                    .as_ref()
                                    .is_some_and(|s| s.proxy_enabled),
                            )
                            .debug_selector(|| "proxy-test".into())
                            .on_click(cx.listener(|this, _, _, cx| this.test_applied_proxy(cx))),
                    )
                    .when(
                        self.proxy.snapshot.as_ref().is_some_and(|s| {
                            matches!(s.phase, NetworkPhase::TestQueued | NetworkPhase::Testing)
                        }),
                        |row| {
                            row.child(
                                components::button(
                                    "proxy-cancel-test",
                                    self.tr("proxy-cancel-test"),
                                    None,
                                    false,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, _| {
                                        this.cancel_proxy_test();
                                    },
                                )),
                            )
                        },
                    ),
            )
            .child(div().mt_4().flex().child(components::badge(
                self.tr(self.network_message_id()),
                if self.proxy.show_banner() {
                    Tone::Amber
                } else {
                    Tone::Green
                },
            )));
        if self.proxy.waiting() {
            card = card.child(
                div()
                    .mt_2()
                    .text_xs()
                    .text_color(theme::text_secondary())
                    .child(self.proxy_timing()),
            );
        }
        if let Some(elapsed) = self.proxy.test_elapsed {
            let args = MessageArgs::new().with(
                "elapsed",
                format_duration_millis(self.localizer.locale(), elapsed.as_millis() as u64),
            );
            card = card.child(
                div()
                    .mt_2()
                    .text_xs()
                    .text_color(theme::text_secondary())
                    .child(self.tr_with("proxy-test-result", args)),
            );
        }
        if let Some(snapshot) = &self.proxy.snapshot {
            card = card.child(div().mt_4().text_sm().child(self.tr("proxy-timeline")));
            for event in snapshot
                .events
                .iter()
                .rev()
                .take(if self.proxy.history_expanded { 64 } else { 8 })
            {
                card = card.child(
                    div()
                        .mt_2()
                        .text_xs()
                        .text_color(theme::text_secondary())
                        .child(
                            self.tr_with(
                                "proxy-event-time",
                                MessageArgs::new()
                                    .with("phase", self.tr(phase_id(event.phase)).to_string())
                                    .with("time", self.sync_event_time(event.at).to_string()),
                            ),
                        ),
                );
            }
            if snapshot.events.len() > 8 {
                card = card.child(
                    components::button(
                        "proxy-history-toggle",
                        self.tr(if self.proxy.history_expanded {
                            "proxy-history-collapse"
                        } else {
                            "proxy-history-expand"
                        }),
                        None,
                        false,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.proxy.history_expanded = !this.proxy.history_expanded;
                        cx.notify();
                    })),
                );
            }
            if snapshot.omitted_events > 0 {
                card = card.child(
                    div()
                        .mt_2()
                        .text_xs()
                        .text_color(theme::text_secondary())
                        .child(self.tr_with(
                            "proxy-timeline-truncated",
                            MessageArgs::new().with("count", snapshot.omitted_events.to_string()),
                        )),
                );
            }
        }
        card.into_any_element()
    }
}
