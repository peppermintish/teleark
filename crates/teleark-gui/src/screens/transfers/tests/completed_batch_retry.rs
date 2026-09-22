use super::*;
use teleark_runtime::LocalFilePresence;

#[test]
fn replacement_selection_uses_the_displayed_identity_for_one_or_multiple_missing_files() {
    let rows = [(1, 11), (2, 22), (3, 22), (4, 33)]
        .into_iter()
        .map(|(id, batch)| {
            let mut row = native_snapshot_fixture(id, 1);
            row.batch_id = Some(batch);
            std::sync::Arc::new(row)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        replacement_selection_keys(&[11, 22], &rows),
        vec![1, 0x4000_0000_0000_0000 | 22]
    );
}

fn set_presence(app: &mut TeleArkApp, index: usize, presence: LocalFilePresence) {
    let path = app.native_transfer_view.items[index].destination.clone();
    let mut observation = app.local_downloads.get(&path).expect("observation").clone();
    observation.presence = presence;
    app.local_downloads.insert(path, observation);
}

fn presentation(app: &TeleArkApp) -> std::rc::Rc<projection::Presentation> {
    let items = app.transfer_items();
    app.transfer_projection_cache
        .borrow_mut()
        .presentation(app, items, "")
}

#[gpui_kit::test]
fn completed_batch_retry_is_reachable_and_respects_partial_collapsed_selection(
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
            app.preview_deleted_download_batch();
            app.visual_preview = false;
            assert_eq!(
                presentation(app).actions[&(TransferAction::Retry as usize)].as_slice(),
                &[901, 902]
            );
            cx.notify();
        });
        cx.run_until_parked();
        let bulk = cx
            .debug_bounds("transfer-bulk-row-retry-enabled")
            .expect("bulk Retry enabled");
        let header = cx
            .debug_bounds("transfer-batch-retry-90")
            .expect("batch Retry available");
        assert!(bulk.top() >= px(0.0) && bulk.bottom() <= px(600.0));
        assert!(header.top() >= px(0.0) && header.bottom() <= px(600.0));
        app.update(cx, |app, _| {
            app.expanded_transfer_batches.clear();
            app.selected_transfer_keys = [901].into();
            assert_eq!(
                presentation(app).actions[&(TransferAction::Retry as usize)].as_slice(),
                &[901]
            );
            app.selected_transfer_keys = [903].into();
            assert!(presentation(app).actions[&(TransferAction::Retry as usize)].is_empty());
            app.selected_transfer_keys = [0x4000_0000_0000_005a].into();
            assert_eq!(
                presentation(app).actions[&(TransferAction::Retry as usize)].as_slice(),
                &[901, 902]
            );
        });
        app.update(cx, |app, cx| {
            set_presence(app, 0, LocalFilePresence::Present);
            set_presence(app, 1, LocalFilePresence::Present);
            cx.notify();
        });
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("transfer-bulk-row-retry-disabled")
                .is_some()
        );
        assert!(cx.debug_bounds("transfer-batch-retry-90").is_none());
    }
}

#[gpui_kit::test]
fn file_observations_refresh_retry_targets_without_rebuilding_unchanged_projections(
    cx: &mut gpui_kit::TestAppContext,
) {
    let (app, cx) = crate::app::test_support::preview_app(cx, crate::app::Page::Transfers);
    app.update(cx, |app, _| {
        app.preview_deleted_download_batch();
        app.visual_preview = false;
        app.selected_transfer_keys = [901].into();
        set_presence(app, 1, LocalFilePresence::Present);
        let source = app.transfer_items();
        for presence in [
            LocalFilePresence::Present,
            LocalFilePresence::Missing,
            LocalFilePresence::Checking,
            LocalFilePresence::Unavailable,
            LocalFilePresence::SizeChanged,
            LocalFilePresence::Missing,
        ] {
            let before = presentation(app);
            set_presence(app, 0, presence);
            let after = presentation(app);
            assert!(
                !std::rc::Rc::ptr_eq(&before, &after),
                "presence invalidates action scopes"
            );
            assert!(
                std::sync::Arc::ptr_eq(&source, &app.transfer_items()),
                "unchanged task projection reused"
            );
            let enabled = presence == LocalFilePresence::Missing;
            assert_eq!(
                !after.actions[&(TransferAction::Retry as usize)].is_empty(),
                enabled
            );
            let batch = app
                .transfer_projection_cache
                .borrow_mut()
                .batch_retry_ids(app, 90);
            assert_eq!(!batch.is_empty(), enabled);
            set_presence(app, 0, presence);
            assert!(
                std::rc::Rc::ptr_eq(&after, &presentation(app)),
                "unchanged observation reuses presentation"
            );
            let again = app
                .transfer_projection_cache
                .borrow_mut()
                .batch_retry_ids(app, 90);
            if enabled {
                assert!(std::sync::Arc::ptr_eq(&batch, &again));
            }
        }
    });
}
