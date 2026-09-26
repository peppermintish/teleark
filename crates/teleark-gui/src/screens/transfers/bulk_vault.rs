//! Bounded, event-driven dispatch of Vault actions selected in the transfer list.
use super::*;
use std::{
    collections::{BTreeSet, VecDeque},
    time::Instant,
};

const MAX_ACTIVE_ACTIONS: usize = 8;

pub(crate) struct VaultBulkActionQueue {
    scope: (i64, u64),
    action: TransferAction,
    pending: VecDeque<u64>,
    active: BTreeSet<u64>,
    started: Instant,
    last_activity: Instant,
    clock: Option<gpui_kit::Task<()>>,
}

impl VaultBulkActionQueue {
    pub(crate) fn new(scope: (i64, u64), action: TransferAction, ids: &[u64]) -> Self {
        let mut seen = BTreeSet::new();
        Self {
            scope,
            action,
            pending: ids.iter().copied().filter(|id| seen.insert(*id)).collect(),
            active: BTreeSet::new(),
            started: Instant::now(),
            last_activity: Instant::now(),
            clock: None,
        }
    }

    fn next(&mut self, free_jobs: usize) -> Option<u64> {
        if self.active.len() >= MAX_ACTIVE_ACTIONS || free_jobs == 0 {
            return None;
        }
        let id = self.pending.pop_front()?;
        self.active.insert(id);
        self.last_activity = Instant::now();
        Some(id)
    }

    fn finished(&self) -> bool {
        self.pending.is_empty() && self.active.is_empty()
    }
}

impl TeleArkApp {
    pub(crate) fn vault_bulk_action_blocks(&self, action: TransferAction) -> bool {
        self.vault_bulk_action_queue.as_ref().is_some_and(|queue| {
            !matches!(
                (queue.action, action),
                (
                    TransferAction::Resume | TransferAction::Retry,
                    TransferAction::Pause | TransferAction::Cancel
                )
            )
        })
    }

    pub(crate) fn apply_vault_bulk_action(
        &mut self,
        action: TransferAction,
        ids: &[u64],
        cx: &mut Context<Self>,
    ) {
        if ids.is_empty() || self.visual_preview || self.vault_bulk_action_blocks(action) {
            return;
        }
        // Explicit pause/stop takes precedence over pending resumes. Admitted
        // jobs retain their owners and receive independent control commands.
        self.vault_bulk_action_queue = None;
        let Some(account) = self.telegram_account.as_ref().map(|account| account.id) else {
            return;
        };
        if self.vault.is_none() {
            self.transfer_action_error = Some(ApplicationErrorKind::Persistence);
            cx.notify();
            return;
        }
        let mut queue =
            VaultBulkActionQueue::new((account, self.telegram_login_generation), action, ids);
        // This clock only presents elapsed/idle labels. Commands advance solely
        // through completion callbacks, never a recurring service poll.
        queue.clock = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(1))
                    .await;
                let Some(this) = this.upgrade() else { return };
                let active = this.update(cx, |app, cx| {
                    let active = app.vault_bulk_action_queue.is_some();
                    if active {
                        cx.notify();
                    }
                    active
                });
                if !active {
                    return;
                }
            }
        }));
        self.transfer_action_error = None;
        self.vault_bulk_action_queue = Some(queue);
        cx.notify();
        self.advance_vault_bulk_actions(cx);
    }

    pub(crate) fn advance_vault_bulk_actions(&mut self, cx: &mut Context<Self>) {
        loop {
            let Some(queue) = self.vault_bulk_action_queue.as_mut() else {
                return;
            };
            if Some(queue.scope.0) != self.telegram_account.as_ref().map(|account| account.id)
                || queue.scope.1 != self.telegram_login_generation
                || queue.finished()
            {
                self.vault_bulk_action_queue = None;
                self.advance_transition(cx);
                cx.notify();
                return;
            }
            let Some(id) = queue.next(64_usize.saturating_sub(self.vault_transfer_jobs.len()))
            else {
                return;
            };
            let action = queue.action;
            if !self.apply_vault_transfer_action(id, action, cx) {
                // A changed/deleted task or an overlapping command must never
                // silently discard the rest of the user's selected work.
                if let Some(queue) = self.vault_bulk_action_queue.as_mut() {
                    queue.active.remove(&id);
                }
                self.transfer_action_error
                    .get_or_insert(ApplicationErrorKind::Conflict);
                cx.notify();
            }
        }
    }

    pub(crate) fn finish_vault_bulk_action(
        &mut self,
        scope: (i64, u64),
        id: u64,
        action: TransferAction,
        cx: &mut Context<Self>,
    ) {
        if let Some(queue) = self.vault_bulk_action_queue.as_mut()
            && queue.scope == scope
            && queue.action == action
            && queue.active.remove(&id)
        {
            queue.last_activity = Instant::now();
        }
        self.advance_vault_bulk_actions(cx);
    }

    pub(crate) fn render_vault_bulk_progress(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(queue) = self.vault_bulk_action_queue.as_ref() else {
            return div().into_any_element();
        };
        div()
            .debug_selector(|| "vault-bulk-action-progress".into())
            .w_full()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(
                div().flex_1().text_xs().text_color(theme::blue()).child(
                    self.tr_with(
                        "transfer-vault-bulk-progress",
                        MessageArgs::new()
                            .with("action", self.tr(queue.action.label()).to_string())
                            .with(
                                "queued",
                                format_integer(self.locale(), queue.pending.len() as u64),
                            )
                            .with(
                                "active",
                                format_integer(self.locale(), queue.active.len() as u64),
                            )
                            .with(
                                "seconds",
                                format_integer(self.locale(), queue.started.elapsed().as_secs()),
                            )
                            .with(
                                "idle",
                                format_integer(
                                    self.locale(),
                                    queue.last_activity.elapsed().as_secs(),
                                ),
                            ),
                    ),
                ),
            )
            .child(
                components::button(
                    "vault-bulk-cancel-pending",
                    self.tr("transfer-vault-bulk-cancel"),
                    None,
                    false,
                )
                .debug_selector(|| "vault-bulk-cancel-pending".into())
                .tooltip(self.tr("transfer-vault-bulk-cancel-help"))
                .on_click(cx.listener(|app, _, _, cx| {
                    // Active jobs keep their retained owners and row controls.
                    app.vault_bulk_action_queue = None;
                    app.advance_transition(cx);
                    cx.notify();
                })),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_queue_dispatches_every_selected_id_once_and_releases_slots_on_completion() {
        let ids = (0..200).chain(0..20).collect::<Vec<_>>();
        let mut queue = VaultBulkActionQueue::new((7, 1), TransferAction::Pause, &ids);
        let mut dispatched = Vec::new();
        while !queue.finished() {
            while let Some(id) = queue.next(64) {
                dispatched.push(id);
            }
            assert!(queue.active.len() <= MAX_ACTIVE_ACTIONS);
            assert!(queue.next(64).is_none());
            queue.active.pop_first();
        }
        assert_eq!(dispatched, (0..200).collect::<Vec<_>>());
    }

    #[test]
    fn occupied_job_capacity_preserves_queued_work() {
        let mut queue = VaultBulkActionQueue::new((7, 1), TransferAction::Resume, &[1, 2]);
        assert_eq!(queue.next(0), None);
        assert_eq!(queue.pending.len(), 2);
        assert_eq!(queue.next(1), Some(1));
        assert_eq!(queue.pending.len(), 1);
    }

    #[gpui_kit::test]
    fn pause_and_stop_take_over_pending_resumes_without_dropping_running_owners(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, crate::app::Page::Transfers);
        app.update(cx, |app, cx| {
            let account = app.telegram_account.as_ref().expect("account").id;
            let scope = (account, app.telegram_login_generation);
            app.visual_preview = false;
            app.vault_transfer_jobs.insert(
                (account, 1, false),
                cx.spawn(async |_, _| std::future::pending::<()>().await),
            );
            for action in [TransferAction::Pause, TransferAction::Cancel] {
                let mut queue =
                    VaultBulkActionQueue::new(scope, TransferAction::Resume, &[1, 2, 3]);
                assert_eq!(queue.next(1), Some(1));
                app.vault_bulk_action_queue = Some(queue);
                assert!(!app.vault_bulk_action_blocks(action));
                assert!(app.vault_bulk_action_blocks(TransferAction::Resume));
                assert!(app.vault_bulk_action_blocks(TransferAction::Retry));
                app.apply_vault_bulk_action(action, &[1], cx);
                // This fixture has no runtime. Control admission reports that
                // failure, but the explicit control still discards future resumes.
                assert!(app.vault_bulk_action_queue.is_none());
                assert_eq!(
                    app.transfer_action_error,
                    Some(ApplicationErrorKind::Persistence)
                );
                assert_eq!(app.vault_transfer_jobs.len(), 1);
                app.finish_vault_bulk_action(scope, 1, TransferAction::Resume, cx);
                assert!(
                    app.vault_bulk_action_queue.is_none(),
                    "late resume completion cannot admit discarded work"
                );
            }
            app.visual_preview = true;
        });
    }

    #[gpui_kit::test]
    fn bulk_progress_stays_visible_and_cancel_keeps_running_job_owners(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, crate::app::Page::Transfers);
        app.update(cx, |app, cx| {
            let account = app.telegram_account.as_ref().expect("account").id;
            app.transfer_controls_expanded = false;
            app.show_transfer_detail = false;
            app.selected_transfer_keys.clear();
            let mut queue = VaultBulkActionQueue::new(
                (account, app.telegram_login_generation),
                TransferAction::Resume,
                &[1, 2],
            );
            assert_eq!(queue.next(1), Some(1));
            app.vault_bulk_action_queue = Some(queue);
            app.vault_transfer_jobs.insert(
                (account, 1, false),
                cx.spawn(async |_, _| std::future::pending::<()>().await),
            );
            cx.notify();
        });
        for fullscreen in [false, true] {
            cx.simulate_resize(gpui_kit::size(px(900.0), px(600.0)));
            cx.update(|window, _| {
                if window.is_fullscreen() != fullscreen {
                    window.toggle_fullscreen();
                }
                assert_eq!(window.is_fullscreen(), fullscreen);
            });
            for page in [crate::app::Page::Transfers, crate::app::Page::Settings] {
                app.update(cx, |app, cx| app.set_page(page, cx));
                cx.run_until_parked();
                let feedback = cx
                    .debug_bounds("vault-bulk-action-progress")
                    .expect("bulk status must survive navigation");
                cx.update(|window, _| {
                    assert!(feedback.bottom() < window.viewport_size().height);
                    assert!(feedback.right() <= window.viewport_size().width);
                });
            }
        }
        // Cancelling the remaining queue does not cancel/drop an already
        // running transfer; its explicit pause/stop action remains available.
        let cancel = cx
            .debug_bounds("vault-bulk-cancel-pending")
            .expect("cancel queued actions");
        cx.simulate_click(cancel.center(), Default::default());
        app.read_with(cx, |app, _| {
            assert!(app.vault_bulk_action_queue.is_none());
            assert_eq!(app.vault_transfer_jobs.len(), 1);
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("vault-bulk-action-progress").is_none());
    }

    #[gpui_kit::test]
    fn changed_account_discards_pending_actions_and_capacity_completion_unblocks_dispatch(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (app, cx) = crate::app::test_support::preview_app(cx, crate::app::Page::Transfers);
        app.update(cx, |app, cx| {
            let account = app.telegram_account.as_ref().expect("account").id;
            let scope = (account, app.telegram_login_generation);
            app.vault_bulk_action_queue = Some(VaultBulkActionQueue::new(
                scope,
                TransferAction::Pause,
                &[1, 2],
            ));
            app.telegram_login_generation = app.telegram_login_generation.wrapping_add(1);
            app.advance_vault_bulk_actions(cx);
            assert!(app.vault_bulk_action_queue.is_none());
            assert!(app.vault_transfer_jobs.is_empty());
            assert!(app.transfer_action_error.is_none());

            let scope = (account, app.telegram_login_generation);
            app.vault_bulk_action_queue = Some(VaultBulkActionQueue::new(
                scope,
                TransferAction::Pause,
                &[1, 2],
            ));
            for id in 10..74 {
                app.vault_transfer_jobs.insert(
                    (account, id, true),
                    cx.spawn(async |_, _| std::future::pending::<()>().await),
                );
            }
            app.advance_vault_bulk_actions(cx);
            assert_eq!(
                app.vault_bulk_action_queue
                    .as_ref()
                    .expect("waiting queue")
                    .pending
                    .len(),
                2
            );
            app.vault_transfer_jobs.remove(&(account, 10, true));
            app.finish_vault_bulk_action(scope, 10, TransferAction::Pause, cx);
            // Synthetic preview has no runtime admission owner. Both attempted
            // actions fail explicitly; neither remains silently stranded.
            assert!(app.vault_bulk_action_queue.is_none());
            assert_eq!(
                app.transfer_action_error,
                Some(ApplicationErrorKind::Conflict)
            );
        });
    }
}
