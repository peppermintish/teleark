//! Group edges follow adjacent visible identities, not the full batch count.
//! This keeps filtering and virtualized scrolling from joining unrelated tasks.
use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum BatchRowPosition {
    Standalone,
    Collapsed,
    Header,
    Member { first: bool, last: bool },
    WindowMember,
}

impl BatchRowPosition {
    pub(super) fn at(items: &[TransferItem], index: usize) -> Self {
        let Some(item) = items.get(index) else {
            return Self::Standalone;
        };
        let same_group = |other: &TransferItem| {
            item.parent_key().is_some() && item.parent_key() == other.parent_key()
        };
        let next_member = items
            .get(index + 1)
            .is_some_and(|next| next.child() && same_group(next));
        if item.batch_count() > 0 {
            return if next_member {
                Self::Header
            } else {
                Self::Collapsed
            };
        }
        if item.child() {
            let connected_before = index
                .checked_sub(1)
                .and_then(|ix| items.get(ix))
                .is_some_and(|previous| {
                    (previous.child() || previous.batch_count() > 0) && same_group(previous)
                });
            return Self::Member {
                first: !connected_before,
                last: !next_member,
            };
        }
        Self::Standalone
    }

    fn starts_group(self) -> bool {
        matches!(self, Self::Header | Self::Member { first: true, .. })
    }

    fn ends_group(self) -> bool {
        matches!(self, Self::Member { last: true, .. })
    }

    fn inline_group(self) -> bool {
        matches!(self, Self::Header | Self::Member { .. })
    }

    pub(super) fn decorate(
        self,
        row: gpui_kit::Stateful<gpui_kit::Div>,
        key: u64,
        highlighted: bool,
        cx: &gpui_kit::App,
    ) -> AnyElement {
        let palette = theme::batch_palette(cx);
        let background = match self {
            Self::Header => palette.header,
            Self::Member { .. } | Self::WindowMember => palette.body,
            _ => gpui_kit::component::Theme::global(cx).table.into(),
        };
        let row = row
            .bg(if highlighted {
                background.blend(palette.accent.alpha(0.10))
            } else {
                background
            })
            .hover(move |row| row.bg(background.blend(palette.accent.alpha(0.06))))
            .when(self.inline_group(), |row| {
                row.border_x_1()
                    .border_color(palette.border)
                    .when(self.starts_group(), |row| {
                        row.border_t_1().rounded_t(theme::RADIUS_MEDIUM)
                    })
                    .when(self == Self::Header, |row| row.border_b_1())
                    .when(self.ends_group(), |row| {
                        row.border_b_1().rounded_b(theme::RADIUS_MEDIUM)
                    })
                    .child(
                        div()
                            .debug_selector(move || format!("batch-rail-{key}"))
                            .absolute()
                            .left_0()
                            .top(if self.starts_group() {
                                theme::RADIUS_MEDIUM
                            } else {
                                px(0.0)
                            })
                            .bottom(if self.ends_group() {
                                theme::RADIUS_MEDIUM
                            } else {
                                px(0.0)
                            })
                            .w(theme::BATCH_RAIL_WIDTH)
                            .bg(palette.rail),
                    )
            })
            .when(matches!(self, Self::Standalone | Self::Collapsed), |row| {
                row.border_b_1().border_color(theme::border_subtle())
            });
        div()
            .debug_selector(move || format!("transfer-slot-{key}"))
            .w_full()
            .when(self.inline_group(), |slot| {
                slot.px(theme::BATCH_GROUP_INSET)
                    .when(self.starts_group(), |slot| slot.pt(theme::BATCH_GROUP_GAP))
                    .when(self.ends_group(), |slot| slot.pb(theme::BATCH_GROUP_GAP))
            })
            .child(row)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui_kit::test]
    fn group_edges_follow_filtered_identities_without_materializing_rows(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, crate::app::Page::Transfers);
        app.update(cx, |app, _| {
            app.preview_batch_groups();
            let items = app.transfer_items();
            let visible = projection::visible_items(
                &items,
                app,
                "nav-transfers-all",
                "",
                &app.expanded_transfer_batches,
            );
            projection::reset_materialized_rows();
            assert_eq!(
                (0..8)
                    .map(|index| BatchRowPosition::at(&visible, index))
                    .collect::<Vec<_>>(),
                [
                    BatchRowPosition::Standalone,
                    BatchRowPosition::Header,
                    BatchRowPosition::Member {
                        first: false,
                        last: false
                    },
                    BatchRowPosition::Member {
                        first: false,
                        last: true
                    },
                    BatchRowPosition::Header,
                    BatchRowPosition::Member {
                        first: false,
                        last: false
                    },
                    BatchRowPosition::Member {
                        first: false,
                        last: true
                    },
                    BatchRowPosition::Standalone,
                ]
            );
            // A filtered subset closes after its last visible member, regardless
            // of the original count or a following batch in the other ID space.
            let subset = vec![
                visible[1].clone(),
                visible[2].clone(),
                visible[4].clone(),
                visible[6].clone(),
                visible[7].clone(),
            ];
            assert_eq!(
                BatchRowPosition::at(&subset, 1),
                BatchRowPosition::Member {
                    first: false,
                    last: true
                }
            );
            assert_eq!(
                BatchRowPosition::at(&subset, 3),
                BatchRowPosition::Member {
                    first: false,
                    last: true
                }
            );
            // Status filters can reveal members without their aggregate header.
            // Never draw a continuation across an unrelated task or batch.
            let separated = vec![
                visible[2].clone(),
                visible[7].clone(),
                visible[3].clone(),
                visible[6].clone(),
            ];
            for index in [0, 2, 3] {
                assert_eq!(
                    BatchRowPosition::at(&separated, index),
                    BatchRowPosition::Member {
                        first: true,
                        last: true
                    }
                );
            }
            let collapsed = projection::visible_items(
                &items,
                app,
                "nav-transfers-all",
                "",
                &Default::default(),
            );
            assert_eq!(
                BatchRowPosition::at(&collapsed, 1),
                BatchRowPosition::Collapsed
            );
            assert_eq!(
                BatchRowPosition::at(&collapsed, 2),
                BatchRowPosition::Collapsed
            );
            assert_eq!(projection::materialized_rows(), 0);
        });
    }

    fn bounds(
        cx: &mut gpui_kit::VisualTestContext,
        prefix: &str,
        key: u64,
    ) -> gpui_kit::Bounds<gpui_kit::Pixels> {
        cx.debug_bounds(Box::leak(format!("{prefix}-{key}").into_boxed_str()))
            .expect("visible batch element")
    }

    #[gpui_kit::test]
    fn expanded_groups_keep_title_member_and_neighbor_boundaries_when_selected_and_scrolled(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, crate::app::Page::Transfers);
        app.update(cx, |app, cx| {
            app.preview_batch_groups();
            cx.notify();
        });
        for (width, height) in [(900.0, 600.0), (1360.0, 760.0)] {
            cx.simulate_resize(gpui_kit::size(px(width), px(height)));
            {
                let locale = teleark_i18n::SupportedLocale::EnUs;
                for appearance in [
                    teleark_runtime::AppearancePreference::Light,
                    teleark_runtime::AppearancePreference::Dark,
                ] {
                    let keys = app.update(cx, |app, cx| {
                        app.localizer = teleark_i18n::Localizer::new(locale).expect("catalog");
                        app.preferences.appearance = appearance;
                        app.selected_transfer_keys.clear();
                        app.focused_transfer_key = None;
                        cx.notify();
                        let items = app.transfer_items();
                        projection::visible_items(
                            &items,
                            app,
                            "nav-transfers-all",
                            "",
                            &app.expanded_transfer_batches,
                        )
                        .iter()
                        .enumerate()
                        .map(|(index, item)| item.key(index))
                        .collect::<Vec<_>>()
                    });
                    for index in [1, 4] {
                        app.update(cx, |app, cx| {
                            app.transfer_scroll.scroll_to(gpui_kit::ListOffset {
                                item_ix: index - 1,
                                offset_in_item: px(0.0),
                            });
                            cx.notify();
                        });
                        cx.simulate_mouse_move(
                            gpui_kit::point(px(0.0), px(0.0)),
                            None,
                            Default::default(),
                        );
                        cx.run_until_parked();
                        let header = bounds(cx, "transfer-row", keys[index]);
                        let first = bounds(cx, "transfer-row", keys[index + 1]);
                        let last = bounds(cx, "transfer-row", keys[index + 2]);
                        let following = bounds(cx, "transfer-row", keys[index + 3]);
                        assert_eq!(header.size.height, theme::BATCH_ROW_HEIGHT);
                        assert_eq!(first.size.height, theme::ROW_HEIGHT);
                        assert_eq!(header.bottom(), first.top());
                        assert_eq!(first.bottom(), last.top());
                        assert_eq!(header.left(), last.left());
                        assert_eq!(header.right(), last.right());
                        assert_eq!(
                            following.top() - last.bottom(),
                            theme::BATCH_GROUP_GAP * if index == 1 { 2.0 } else { 1.0 }
                        );
                        assert!(
                            bounds(cx, "transfer-title", keys[index + 1]).left()
                                > bounds(cx, "transfer-title", keys[index]).left()
                        );
                        let checkbox = bounds(cx, "transfer-checkbox", keys[index + 1]);
                        assert!(
                            checkbox.left()
                                >= bounds(cx, "transfer-checkbox", keys[index]).left() + px(24.0)
                        );
                        let header_rail = bounds(cx, "batch-rail", keys[index]);
                        let first_rail = bounds(cx, "batch-rail", keys[index + 1]);
                        let last_rail = bounds(cx, "batch-rail", keys[index + 2]);
                        assert_eq!(header_rail.left(), first_rail.left());
                        assert_eq!(first_rail.left(), last_rail.left());
                        assert!((header_rail.bottom() - first_rail.top()).abs() <= px(1.0));
                        assert_eq!(first_rail.bottom(), last_rail.top());
                        cx.update(|window, cx| {
                            let quads = window.painted_quads();
                            let palette = theme::batch_palette(cx);
                            for (row, color) in [(header, palette.header), (first, palette.body)] {
                                let surfaces = [
                                    color,
                                    color.blend(palette.accent.alpha(0.06)),
                                    color.blend(palette.accent.alpha(0.10)),
                                ];
                                assert!(
                                    quads.iter().any(|quad| quad.bounds
                                        == row.scale(window.scale_factor())
                                        && surfaces
                                            .iter()
                                            .any(|surface| quad.background == (*surface).into())),
                                    "row retains its hierarchy surface under hover and selection"
                                );
                            }
                            assert_ne!(palette.header, palette.body);
                            assert_ne!(
                                palette.body,
                                gpui_kit::Rgba::from(gpui_kit::component::Theme::global(cx).table)
                            );
                        });
                        cx.simulate_click(
                            gpui_kit::point(checkbox.left() + px(7.0), checkbox.center().y),
                            Default::default(),
                        );
                        cx.run_until_parked();
                        app.read_with(cx, |app, _| {
                            assert!(app.selected_transfer_keys.contains(&keys[index + 1]))
                        });
                        // Selecting a file reveals the bulk toolbar. The whole
                        // list can move, but the group's internal geometry stays.
                        let selected_row = bounds(cx, "transfer-row", keys[index + 1]);
                        let selected_rail = bounds(cx, "batch-rail", keys[index + 1]);
                        assert_eq!(
                            selected_rail.origin - selected_row.origin,
                            first_rail.origin - first.origin
                        );
                        assert_eq!(selected_rail.size, first_rail.size);
                        assert_eq!(
                            bounds(cx, "transfer-row", keys[index + 2]).top(),
                            selected_row.bottom()
                        );
                    }
                }
            }
        }
    }
}
