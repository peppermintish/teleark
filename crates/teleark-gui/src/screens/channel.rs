use gpui::{
    AnyElement, Context, FontWeight, IntoElement, ParentElement as _, SharedString, Styled as _,
    Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::IconName;
use gpui_component::scroll::ScrollableElement as _;
use teleark_i18n::{
    MessageArgs,
    format::{format_integer, format_percent},
};

use crate::{
    app::TeleArkApp,
    components::{self, Tone},
    layout::LayoutPolicy,
    theme,
};

impl TeleArkApp {
    pub(crate) fn render_channel(
        &self,
        _window: &mut Window,
        layout: LayoutPolicy,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let padding = layout.content_padding();
        let range_progress = format_percent(self.locale(), 0.62, 0);
        let checkpoint = self.tr_with(
            "index-job-checkpoint-value",
            MessageArgs::new().with("id", format_integer(self.locale(), 653_820)),
        );
        let toolbar = div()
            .h(px(58.0))
            .px(px(padding))
            .flex()
            .items_center()
            .child(components::section_title(self.tr("index-channel-title")))
            .child(div().flex_1())
            .child(components::button(
                "channel-options",
                self.tr("index-options"),
                Some(IconName::Settings2),
                false,
            ));

        let channel_header = components::card()
            .mx(px(padding))
            .p(px(padding))
            .flex()
            .flex_wrap()
            .items_center()
            .gap_4()
            .child(
                div()
                    .size(px(58.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(theme::purple_soft())
                    .text_color(theme::purple())
                    .text_lg()
                    .font_weight(FontWeight::BOLD)
                    .child("4K"),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .text_xl()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(self.selected_source),
                            )
                            .child(components::badge(
                                self.tr("index-status-synced"),
                                Tone::Green,
                            )),
                    )
                    .child(
                        div()
                            .mt_2()
                            .text_sm()
                            .text_color(theme::text_secondary())
                            .child(self.tr("index-channel-description")),
                    ),
            )
            .child(channel_metric(
                self.tr("index-messages-scanned"),
                "1,284,921",
            ))
            .child(channel_metric(self.tr("index-files-indexed"), "392,104"))
            .child(channel_metric(self.tr("index-indexed-size"), "18.76 TB"))
            .child(channel_metric(self.tr("index-latest-sync"), "4m"));

        let overview = div()
            .mt_4()
            .mx(px(padding))
            .grid()
            .when(layout.is_compact(), |overview| overview.grid_cols(2))
            .when(!layout.is_compact(), |overview| overview.grid_cols(4))
            .gap_3()
            .child(index_summary(
                self.tr("index-new-files"),
                "+128",
                self.tr("index-since-last-sync"),
                Tone::Green,
            ))
            .child(index_summary(
                self.tr("index-current-date"),
                "2020-11-18",
                self.tr("index-history-scan"),
                Tone::Blue,
            ))
            .child(index_summary(
                self.tr("index-scan-speed"),
                "2,430 msg/s",
                self.tr("index-batch-size"),
                Tone::Purple,
            ))
            .child(index_summary(
                self.tr("index-estimated-time"),
                "1h 42m",
                self.tr("index-until-complete"),
                Tone::Amber,
            ));

        let coverage = components::card()
            .flex_1()
            .min_w_0()
            .when(!layout.is_compact(), |coverage| coverage.h_full())
            .when(layout.is_compact(), |coverage| coverage.flex_none())
            .p_5()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .child(components::section_title(self.tr("index-coverage-title")))
                            .child(
                                div()
                                    .mt_1()
                                    .text_xs()
                                    .text_color(theme::text_muted())
                                    .child(self.tr("index-coverage-description")),
                            ),
                    )
                    .child(coverage_legend(self)),
            )
            .child(
                div()
                    .mt_5()
                    .h(px(185.0))
                    .flex()
                    .items_end()
                    .gap_3()
                    .children([
                        coverage_year("2018", 100.0, Coverage::Complete),
                        coverage_year("2019", 100.0, Coverage::Complete),
                        coverage_year("2020", 62.0, Coverage::Partial),
                        coverage_year("2021", 0.0, Coverage::Missing),
                        coverage_year("2022", 100.0, Coverage::Complete),
                        coverage_year("2023", 100.0, Coverage::Complete),
                        coverage_year("2024", 100.0, Coverage::Complete),
                        coverage_year("2025", 100.0, Coverage::Complete),
                        coverage_year("2026", 84.0, Coverage::Partial),
                    ]),
            )
            .child(
                div()
                    .mt_5()
                    .pt_4()
                    .border_t_1()
                    .border_color(theme::border())
                    .grid()
                    .grid_cols(3)
                    .gap_3()
                    .child(range_card(
                        self.tr("index-range-complete"),
                        "2018-01-01 — 2019-12-31",
                        Tone::Green,
                    ))
                    .child(range_card(
                        self.tr("index-range-active"),
                        "2020-01-01 — 2020-11-18",
                        Tone::Blue,
                    ))
                    .child(range_card(
                        self.tr("index-range-unscanned"),
                        "2021-01-01 — 2021-12-31",
                        Tone::Neutral,
                    )),
            );

        let job = components::card()
            .w(px(layout.channel_job_width()))
            .when(layout.is_compact(), |job| job.w_full())
            .when(!layout.is_compact(), |job| job.h_full())
            .flex_none()
            .p_5()
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(components::section_title(self.tr("index-current-job")))
                    .child(div().flex_1())
                    .child(components::badge(
                        if self.index_paused {
                            self.tr("index-state-paused")
                        } else {
                            self.tr("index-state-indexing")
                        },
                        if self.index_paused {
                            Tone::Amber
                        } else {
                            Tone::Blue
                        },
                    )),
            )
            .child(
                div()
                    .mt_5()
                    .size(px(136.0))
                    .mx_auto()
                    .rounded_full()
                    .border(px(13.0))
                    .border_color(theme::blue_soft())
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .text_2xl()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::blue())
                            .child(range_progress),
                    )
                    .child(
                        div()
                            .mt_1()
                            .text_xs()
                            .text_color(theme::text_muted())
                            .child(self.tr("index-range-progress")),
                    ),
            )
            .child(
                div()
                    .mt_5()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(job_row(
                        self.tr("index-job-range"),
                        "2020-01-01 — 2020-12-31",
                    ))
                    .child(job_row(self.tr("index-job-checkpoint"), checkpoint))
                    .child(job_row(self.tr("index-job-files-found"), "72,481"))
                    .child(job_row(self.tr("index-job-errors"), "3"))
                    .child(job_row(self.tr("index-job-updated"), "2s")),
            )
            .child(div().flex_1())
            .child(
                components::button(
                    "index-toggle",
                    if self.index_paused {
                        self.tr("action-resume")
                    } else {
                        self.tr("action-pause")
                    },
                    Some(if self.index_paused {
                        IconName::ArrowRight
                    } else {
                        IconName::Dash
                    }),
                    true,
                )
                .w_full()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.index_paused = !this.index_paused;
                    cx.notify();
                })),
            )
            .child(
                components::button(
                    "index-cancel",
                    self.tr("action-cancel"),
                    Some(IconName::Close),
                    false,
                )
                .w_full()
                .mt_2(),
            );
        let job = if layout.is_compact() {
            job.into_any_element()
        } else {
            job.overflow_y_scrollbar().into_any_element()
        };

        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .bg(theme::canvas())
            .overflow_y_scrollbar()
            .child(toolbar)
            .child(channel_header)
            .child(overview)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .p(px(padding))
                    .pt_4()
                    .flex()
                    .when(layout.is_compact(), |content| content.flex_col())
                    .gap_4()
                    .child(coverage)
                    .child(job),
            )
            .into_any_element()
    }
}

#[derive(Clone, Copy)]
enum Coverage {
    Complete,
    Partial,
    Missing,
}

fn channel_metric(label: SharedString, value: impl Into<SharedString>) -> AnyElement {
    div()
        .min_w(px(130.0))
        .pl_5()
        .border_l_1()
        .border_color(theme::border())
        .child(div().text_xs().text_color(theme::text_muted()).child(label))
        .child(
            div()
                .mt_2()
                .font_weight(FontWeight::SEMIBOLD)
                .child(value.into()),
        )
        .into_any_element()
}

fn index_summary(
    label: SharedString,
    value: &'static str,
    hint: SharedString,
    tone: Tone,
) -> AnyElement {
    components::card()
        .p_4()
        .child(
            div()
                .text_xs()
                .text_color(theme::text_secondary())
                .child(label),
        )
        .child(
            div()
                .mt_2()
                .text_xl()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(tone.foreground())
                .child(value),
        )
        .child(
            div()
                .mt_1()
                .text_xs()
                .text_color(theme::text_muted())
                .child(hint),
        )
        .into_any_element()
}

fn coverage_legend(app: &TeleArkApp) -> AnyElement {
    div()
        .flex()
        .gap_4()
        .child(legend_item(
            app.tr("index-coverage-complete"),
            theme::green(),
        ))
        .child(legend_item(app.tr("index-coverage-partial"), theme::blue()))
        .child(legend_item(
            app.tr("index-coverage-missing"),
            theme::border(),
        ))
        .into_any_element()
}

fn legend_item(label: SharedString, color: gpui::Rgba) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap_2()
        .text_xs()
        .text_color(theme::text_secondary())
        .child(div().size_2().rounded_full().bg(color))
        .child(label)
        .into_any_element()
}

fn coverage_year(year: &'static str, percent: f32, coverage: Coverage) -> AnyElement {
    let color = match coverage {
        Coverage::Complete => theme::green(),
        Coverage::Partial => theme::blue(),
        Coverage::Missing => theme::border(),
    };
    div()
        .flex_1()
        .h_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_end()
        .gap_2()
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(if matches!(coverage, Coverage::Missing) {
                    theme::text_muted()
                } else {
                    color
                })
                .child(format!("{percent:.0}%")),
        )
        .child(
            div()
                .w_full()
                .h(px(118.0))
                .flex()
                .items_end()
                .rounded(theme::RADIUS_SMALL)
                .bg(theme::border_subtle())
                .overflow_hidden()
                .child(div().w_full().h(px(118.0 * percent / 100.0)).bg(color)),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme::text_secondary())
                .child(year),
        )
        .into_any_element()
}

fn range_card(label: SharedString, range: &'static str, tone: Tone) -> AnyElement {
    div()
        .p_3()
        .rounded(theme::RADIUS_SMALL)
        .bg(tone.background())
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(tone.foreground())
                .child(label),
        )
        .child(
            div()
                .mt_1()
                .text_xs()
                .text_color(theme::text_secondary())
                .child(range),
        )
        .into_any_element()
}

fn job_row(label: SharedString, value: impl Into<SharedString>) -> AnyElement {
    div()
        .flex()
        .text_xs()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_color(theme::text_muted())
                .child(label),
        )
        .child(
            div()
                .text_color(theme::text_secondary())
                .child(value.into()),
        )
        .into_any_element()
}
