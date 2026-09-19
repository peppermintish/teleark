//! Background presentation owner. Business operations stay in the runtime.

use super::*;

impl TeleArkApp {
    pub(crate) fn monitor_channel_download(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(transfers) = self.transfers.clone() else {
            return;
        };
        let mut subscription = transfers.subscribe();
        self.transfer_monitor_task = Some(cx.spawn(async move |this, cx| {
            loop {
                let reader = transfers.clone();
                let snapshot = cx
                    .background_spawn(async move { reader.snapshot(id) })
                    .await;
                let Some(entity) = this.upgrade() else { return };
                let outcome = entity.update(cx, |this, cx| {
                    let state = snapshot.as_ref().map(|snapshot| snapshot.state).unwrap_or(
                        ChannelDownloadState::Failed(
                            teleark_core::ApplicationErrorKind::Persistence,
                        ),
                    );
                    this.telegram_download = Some((id, state));
                    let file_name = snapshot
                        .as_ref()
                        .map(|snapshot| snapshot.file_name.clone())
                        .unwrap_or_else(|| this.tr("transfer-value-unavailable").to_string());
                    let notification = match state {
                        ChannelDownloadState::Completed
                            if this.preferences.notify_download_completed =>
                        {
                            Some((
                                true,
                                this.tr_with(
                                    "notification-download-completed",
                                    MessageArgs::new().with("name", file_name.clone()),
                                ),
                            ))
                        }
                        ChannelDownloadState::Failed(_)
                            if this.preferences.notify_download_failed =>
                        {
                            Some((
                                false,
                                this.tr_with(
                                    "notification-download-failed",
                                    MessageArgs::new().with("name", file_name),
                                ),
                            ))
                        }
                        _ => None,
                    };
                    let reveal_path = (state == ChannelDownloadState::Completed
                        && this.preferences.reveal_completed_downloads)
                        .then(|| snapshot.map(|snapshot| snapshot.destination))
                        .flatten();
                    this.refresh_volume_space(cx);
                    cx.notify();
                    (
                        matches!(
                            state,
                            ChannelDownloadState::Completed
                                | ChannelDownloadState::Failed(_)
                                | ChannelDownloadState::Cancelled
                        ),
                        notification,
                        reveal_path,
                    )
                });
                let (terminal, notification, reveal_path) = outcome;
                if notification.is_some() || reveal_path.is_some() {
                    cx.update(|cx| {
                        if let Some(path) = reveal_path {
                            cx.reveal_path(&path);
                        }
                        if let Some((success, message)) = notification
                            && let Some(handle) = cx.active_window()
                        {
                            let _ = handle.update(cx, |_, window, cx| {
                                window.push_notification(
                                    if success {
                                        Notification::success(message)
                                    } else {
                                        Notification::error(message)
                                    },
                                    cx,
                                );
                            });
                        }
                    });
                }
                drop(entity);
                if terminal || !subscription.changed().await {
                    break;
                }
            }
        }));
    }

    pub(super) fn start_transfer_refresh(&mut self, cx: &mut Context<Self>) {
        if let Some(transfers) = self.transfers.clone() {
            let mut subscription = transfers.subscribe();
            self.transfer_refresh_task = Some(cx.spawn(async move |this, cx| {
                loop {
                    let reader = transfers.clone();
                    let view = cx
                        .background_spawn(async move { reader.snapshot_view() })
                        .await;
                    let Some(entity) = this.upgrade() else { return };
                    entity.update(cx, |app, cx| {
                        if let Ok(view) = view
                            && view.revision != app.native_transfer_view.revision
                        {
                            app.native_transfer_view = view;
                            app.start_transfer_clock(cx);
                            app.advance_transition(cx);
                            app.refresh_volume_space(cx);
                            cx.notify();
                        }
                    });
                    drop(entity);
                    if !subscription.changed().await {
                        break;
                    }
                }
            }));
        }
        if let Some(vault) = self.vault.clone() {
            let mut subscription = vault.subscribe();
            self.vault_refresh_task = Some(cx.spawn(async move |this, cx| {
                loop {
                    let reader = vault.clone();
                    let view = cx
                        .background_spawn(async move { reader.snapshot_view() })
                        .await;
                    let Some(entity) = this.upgrade() else { return };
                    entity.update(cx, |app, cx| {
                        if let Some(view) = view
                            && view.revision != app.vault_transfer_view.revision
                        {
                            app.vault_transfer_view = view;
                            app.start_transfer_clock(cx);
                            app.advance_transition(cx);
                            app.refresh_volume_space(cx);
                            cx.notify();
                        }
                    });
                    drop(entity);
                    if !subscription.changed().await {
                        break;
                    }
                }
            }));
        }
    }

    pub(super) fn has_active_transfer(&self) -> bool {
        let account = self.telegram_account.as_ref().map(|account| account.id);
        self.native_transfer_view.items.iter().any(|row| {
            row.account_id == account
                && (row.cleanup.is_some_and(|cleanup| {
                    !matches!(
                        cleanup.phase,
                        teleark_runtime::ChannelDownloadCleanupPhase::Failed(_)
                    )
                }) || matches!(
                    row.state,
                    ChannelDownloadState::Queued | ChannelDownloadState::Running
                ))
        }) || self.vault_transfer_view.items.iter().any(|row| {
            Some(row.account_id) == account
                && matches!(
                    row.state,
                    teleark_runtime::VaultTransferState::Queued
                        | teleark_runtime::VaultTransferState::Running
                )
        })
    }

    fn start_transfer_clock(&mut self, cx: &mut Context<Self>) {
        if self.transfer_clock_task.is_some() || !self.has_active_transfer() {
            return;
        }
        self.transfer_clock_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let Some(entity) = this.upgrade() else { return };
                let active = entity.update(cx, |app, cx| {
                    let active = app.has_active_transfer();
                    if active {
                        cx.notify();
                    } else {
                        app.transfer_clock_task = None;
                    }
                    active
                });
                drop(entity);
                if !active {
                    break;
                }
            }
        }));
    }

    pub(crate) fn refresh_volume_space(&mut self, cx: &mut Context<Self>) {
        let Some(library) = self.library.clone() else {
            return;
        };
        if self.volume_space_in_flight {
            self.volume_space_pending = true;
            return;
        }
        self.volume_space_in_flight = true;
        self.volume_space_pending = false;
        self.volume_space_task = Some(cx.spawn(async move |this, cx| {
            let space = cx
                .background_spawn(async move { library.download_volume_space().ok() })
                .await;
            let Some(entity) = this.upgrade() else { return };
            entity.update(cx, |this, cx| {
                this.volume_space_in_flight = false;
                if this.volume_space != space {
                    this.volume_space = space;
                    cx.notify();
                }
                if this.volume_space_pending {
                    this.refresh_volume_space(cx);
                }
            });
        }));
    }
}
