use super::*;

#[gpui_kit::test]
fn batch_checkboxes_select_children_and_partial_selection_survives_collapse(
    cx: &mut gpui_kit::TestAppContext,
) {
    let (app, cx) = crate::app::test_support::preview_app(cx, crate::app::Page::Transfers);
    for full in [false, true] {
        cx.simulate_resize(gpui_kit::size(px(900.0), px(600.0)));
        cx.update(|window, _| {
            if window.is_fullscreen() != full {
                window.toggle_fullscreen();
            }
            assert_eq!(window.is_fullscreen(), full);
        });
        app.update(cx, |app, cx| {
            populate_native_batch(app, 9);
            app.selected_transfer_keys.clear();
            app.expanded_transfer_batches.insert(7);
            cx.notify();
        });
        cx.run_until_parked();
        let header = cx
            .debug_bounds("transfer-checkbox-4611686018427387911")
            .expect("group checkbox");
        cx.simulate_click(header.center(), Default::default());
        cx.run_until_parked();
        app.read_with(cx, |app, _| {
            for id in 1..=9 {
                assert!(app.selected_transfer_keys.contains(&id));
            }
            assert!(app.selected_transfer_keys.contains(&0x4000_0000_0000_0007));
        });
        let member = cx
            .debug_bounds("transfer-checkbox-9")
            .expect("child checkbox");
        cx.simulate_click(member.center(), Default::default());
        cx.run_until_parked();
        app.update(cx, |app, _| {
            assert!(!app.selected_transfer_keys.contains(&9));
            assert!(!app.selected_transfer_keys.contains(&0x4000_0000_0000_0007));
            app.expanded_transfer_batches.clear();
            let source = app.transfer_items();
            let rows = projection::visible_items(
                &source,
                app,
                "nav-transfers-all",
                "",
                &app.expanded_transfer_batches,
            );
            let snapshots = (1..=9).map(|id| (id, Some(7))).collect::<Vec<_>>();
            assert_eq!(
                projection::scope_ids(&rows, &app.selected_transfer_keys, &snapshots),
                (1..=8).collect()
            );
            assert!(
                projection::scope_vault_ids(&rows, &app.selected_transfer_keys, &[(1, None)])
                    .is_empty()
            );
        });
    }
}

#[gpui_kit::test]
fn batch_members_sort_running_then_pending_and_keep_selection_identity(
    cx: &mut gpui_kit::TestAppContext,
) {
    let (app, cx) = crate::app::test_support::preview_app(cx, crate::app::Page::Transfers);
    app.update(cx, |app, _| {
        populate_native_batch(app, 48);
        let mut rows = app.native_transfer_view.items.to_vec();
        for (index, state) in [
            ChannelDownloadState::Running,
            ChannelDownloadState::Queued,
            ChannelDownloadState::Paused,
            ChannelDownloadState::Failed(ApplicationErrorKind::Network),
        ]
        .into_iter()
        .enumerate()
        {
            let mut row = (*rows[index]).clone();
            row.state = state;
            rows[index] = std::sync::Arc::new(row);
        }
        app.native_transfer_view.items = rows.into();
        let items = app.transfer_items();
        assert_eq!(
            items
                .iter()
                .skip(1)
                .take(4)
                .map(|item| item.key(0))
                .collect::<Vec<_>>(),
            vec![1, 2, 3, 4]
        );
        projection::set_selection(&items, &mut app.selected_transfer_keys, &[2], true);
        let mut rows = app.native_transfer_view.items.to_vec();
        let mut row = (*rows[0]).clone();
        row.state = ChannelDownloadState::Completed;
        rows[0] = std::sync::Arc::new(row);
        app.native_transfer_view.items = rows.into();
        assert_eq!(app.transfer_items()[1].key(0), 2);
        assert_eq!(
            app.selected_transfer_keys,
            std::collections::BTreeSet::from([2])
        );
    });
}

#[gpui_kit::test]
fn large_batch_popout_has_a_scrollbar_and_reaches_last_member(cx: &mut gpui_kit::TestAppContext) {
    let (app, cx) = crate::app::test_support::preview_app(cx, crate::app::Page::Transfers);
    for full in [false, true] {
        cx.simulate_resize(gpui_kit::size(px(900.0), px(600.0)));
        cx.update(|window, _| {
            if window.is_fullscreen() != full {
                window.toggle_fullscreen();
            }
            assert_eq!(window.is_fullscreen(), full);
        });
        app.update(cx, |app, cx| {
            populate_native_batch(app, 48);
            app.selected_transfer_keys.clear();
            app.focused_transfer_key = None;
            app.show_transfer_detail = false;
            cx.notify();
        });
        cx.run_until_parked();
        let header = cx
            .debug_bounds("transfer-row-4611686018427387911")
            .expect("header");
        cx.simulate_click(header.center(), Default::default());
        cx.run_until_parked();
        let handle = app.read_with(cx, |app, _| app.transfer_batch_window.expect("popout").1);
        let mut child = gpui_kit::VisualTestContext::from_window(handle.into(), cx);
        let viewport = child
            .debug_bounds("batch-window-list-viewport")
            .expect("viewport");
        let scrollbar = child
            .debug_bounds("batch-window-scrollbar")
            .expect("scrollbar");
        assert_eq!(viewport, scrollbar);
        assert!(child.debug_bounds("transfer-row-48").is_some());
        child.simulate_event(gpui_kit::ScrollWheelEvent {
            position: viewport.center(),
            delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.0), px(-10000.0))),
            ..Default::default()
        });
        child.run_until_parked();
        let last = child
            .debug_bounds("transfer-row-1")
            .expect("last member reachable");
        assert!(last.bottom() <= viewport.bottom());
        let checkbox = child
            .debug_bounds("transfer-checkbox-1")
            .expect("last checkbox");
        child.simulate_click(checkbox.center(), Default::default());
        child.run_until_parked();
        app.read_with(&child, |app, _| {
            assert!(app.selected_transfer_keys.contains(&1))
        });
        app.update(&mut child, |app, cx| {
            app.focused_transfer_key = Some(1);
            app.show_transfer_detail = true;
            cx.notify();
        });
        child.run_until_parked();
        let inspector = child
            .debug_bounds("transfer-inspector")
            .expect("member inspector");
        child.simulate_event(gpui_kit::ScrollWheelEvent {
            position: gpui_kit::point(inspector.center().x, inspector.bottom() - px(40.0)),
            delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.0), px(-1000.0))),
            ..Default::default()
        });
        child.run_until_parked();
        assert_eq!(
            child
                .debug_bounds("transfer-row-1")
                .expect("list stays at last member"),
            last
        );
        app.read_with(&child, |app, _| {
            assert!(app.transfer_detail_scroll.offset().y < px(0.0))
        });
        // The native close callback uses this same deferred view-removal path.
        // simulate_close tries to reattach its callback after removal, so use
        // the real application command here and exercise native Close in preview.
        child.update(|window, cx| {
            window.activate_window();
            window.dispatch_action(Box::new(crate::CloseWindow), cx);
        });
        cx.run_until_parked();
        assert_eq!(
            cx.windows().len(),
            1,
            "closing a batch keeps the main window"
        );
    }
}
