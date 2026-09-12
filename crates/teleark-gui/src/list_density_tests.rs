//! Exercise real list rows: controls and text must fit after density changes.
use crate::{
    app::{Page, StorageView},
    theme,
};
use gpui_kit::{
    Bounds, Pixels, ScrollDelta, ScrollWheelEvent, TestAppContext, VisualTestContext, point, px,
    size,
};

fn bounds(cx: &mut VisualTestContext, selector: &str) -> Bounds<Pixels> {
    cx.debug_bounds(Box::leak(selector.to_owned().into_boxed_str()))
        .unwrap_or_else(|| panic!("missing {selector}"))
}

fn contains(row: Bounds<Pixels>, child: Bounds<Pixels>) {
    assert!(
        child.left() >= row.left() && child.right() <= row.right(),
        "horizontal: {row:?} {child:?}"
    );
    assert!(
        child.top() >= row.top() && child.bottom() <= row.bottom(),
        "vertical: {row:?} {child:?}"
    );
}

#[gpui_kit::test]
fn file_lists_keep_contiguous_rows_and_contain_names_and_actions(cx: &mut TestAppContext) {
    let (app, cx) = crate::app::test_support::preview_app(cx, Page::Library);
    for (width, height) in [(900.0, 600.0), (1440.0, 900.0)] {
        cx.simulate_resize(size(px(width), px(height)));
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.page = Page::Library;
                app.vault_locked = false;
                app.show_channel_detail = false;
                app.preferences.appearance = teleark_runtime::AppearancePreference::Light;
                theme::apply_appearance(app.preferences.appearance, window, cx);
                cx.notify();
            })
        });
        cx.run_until_parked();
        let row = bounds(cx, "library-row-0");
        assert_eq!(row.size.height, px(24.0));
        assert_eq!(bounds(cx, "library-row-1").top(), row.bottom());
        contains(row, bounds(cx, "library-name-0"));
        contains(row, bounds(cx, "library-action-0"));
        let keys = app.update(cx, |app, cx| {
            app.page = Page::Storage;
            app.storage_view = StorageView::Files;
            cx.notify();
            app.managed_vault_files
                .iter()
                .take(2)
                .map(|file| file.package_numeric_id)
                .collect::<Vec<_>>()
        });
        cx.run_until_parked();
        let row = bounds(cx, &format!("managed-file-row-{}", keys[0]));
        assert_eq!(row.size.height, px(24.0));
        assert_eq!(
            bounds(cx, &format!("managed-file-row-{}", keys[1])).top(),
            row.bottom()
        );
        contains(row, bounds(cx, &format!("managed-file-name-{}", keys[0])));
        contains(row, bounds(cx, &format!("managed-file-action-{}", keys[0])));
        app.update(cx, |app, cx| {
            app.page = Page::Channel;
            app.storage_view = StorageView::RawFiles;
            app.refresh_channel_file_table(cx);
            cx.notify();
        });
        cx.run_until_parked();
        let row = bounds(cx, "channel-file-row-0");
        assert_eq!(row.size.height, px(24.0));
        assert_eq!(bounds(cx, "channel-file-row-1").top(), row.bottom());
        // Raw columns retain horizontal scrolling in compact windows.
        cx.simulate_event(ScrollWheelEvent {
            position: row.center(),
            delta: ScrollDelta::Pixels(point(px(-2000.0), px(0.0))),
            ..Default::default()
        });
        cx.run_until_parked();
        contains(
            bounds(cx, "channel-file-row-0"),
            bounds(cx, "channel-file-action-0"),
        );
    }
}

#[gpui_kit::test]
fn navigation_and_settings_share_the_data_row_density(cx: &mut TestAppContext) {
    let (app, cx) = crate::app::test_support::preview_app(cx, Page::Settings);
    for (width, height) in [(900.0, 600.0), (1440.0, 900.0)] {
        cx.simulate_resize(size(px(width), px(height)));
        for collapsed in [true, false] {
            app.update(cx, |app, cx| {
                app.preferences.sidebar_collapsed = collapsed;
                cx.notify();
            });
            cx.run_until_parked();
            for selector in [
                "settings-nav-row-0",
                "settings-nav-row-1",
                "primary-nav-row-nav-storage",
                "primary-nav-row-nav-settings",
            ] {
                let row = bounds(cx, selector);
                assert_eq!(row.size.height, px(24.0), "{selector}");
                assert!(row.right() <= px(width) && row.bottom() <= px(height));
            }
        }
    }
}

#[gpui_kit::test]
fn channel_sidebar_entries_use_the_same_row_height(cx: &mut TestAppContext) {
    let (app, cx) = crate::app::test_support::preview_app(cx, Page::Channel);
    for (width, height) in [(900.0, 600.0), (1440.0, 900.0)] {
        cx.simulate_resize(size(px(width), px(height)));
        cx.run_until_parked();
        let id = app.read_with(cx, |app, _| {
            app.telegram_chats
                .iter()
                .find(|chat| {
                    chat.kind == teleark_runtime::TelegramChatKind::Channel
                        && Some(chat.id) != app.storage_channel_id()
                })
                .expect("preview channel")
                .id
        });
        assert_eq!(
            bounds(cx, &format!("channel-sidebar-row-{id}")).size.height,
            px(24.0)
        );
    }
}

#[gpui_kit::test]
fn file_metadata_labels_stay_inside_single_rows(cx: &mut TestAppContext) {
    let (app, cx) = crate::app::test_support::preview_app(cx, Page::Library);
    for (width, height) in [(900.0, 600.0), (1440.0, 900.0)] {
        cx.simulate_resize(size(px(width), px(height)));
        app.update(cx, |app, cx| {
            app.page = Page::FileDetail;
            cx.notify();
        });
        cx.run_until_parked();
        for label in [
            "Name",
            "Source account ID",
            "Source channel ID",
            "Telegram Message ID",
        ] {
            let row = bounds(cx, &format!("file-property-{label}"));
            assert_eq!(row.size.height, px(24.0));
            let text = bounds(cx, &format!("file-property-label-{label}"));
            assert!(text.size.height <= theme::LIST_LINE_HEIGHT);
            contains(row, text);
        }
        let first = bounds(cx, "file-property-Source account ID");
        let second = bounds(cx, "file-property-Source channel ID");
        assert_eq!(first.bottom(), second.top());
    }
}
