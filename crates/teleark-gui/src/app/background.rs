//! Background presentation owner. Business operations stay in the runtime.

use super::*;

impl TeleArkApp {
    pub(crate) fn monitor_channel_download(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(transfers) = self.transfers.clone() else {
            return;
        };
        self.transfer_monitor_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(250))
                    .await;
                let snapshots = transfers.snapshots();
                let Some(entity) = this.upgrade() else { return };
                let outcome = entity.update(cx, |this, cx| {
                    let snapshot = snapshots.ok().and_then(|snapshots| {
                        snapshots.into_iter().find(|snapshot| snapshot.id == id)
                    });
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
                if terminal {
                    break;
                }
            }
        }));
    }

    pub(super) fn start_transfer_refresh(&mut self, cx: &mut Context<Self>) {
        let transfers = self.transfers.clone();
        let vault = self.vault.clone();
        if transfers.is_none() && vault.is_none() {
            return;
        }
        self.transfer_refresh_task = Some(cx.spawn(async move |this, cx| {
            let mut previous = Vec::new();
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(250))
                    .await;
                let Some(entity) = this.upgrade() else { return };
                let authorized = entity.update(cx, |this, _cx| {
                    this.transfers_account_ready
                        && matches!(this.telegram_auth, TelegramAuthState::Authorized(_))
                });
                if authorized && let Some(transfers) = transfers.as_ref() {
                    let _ = transfers.activate_pending_downloads();
                }
                let mut signature: Vec<_> = transfers
                    .as_ref()
                    .and_then(|transfers| transfers.snapshots().ok())
                    .unwrap_or_default()
                    .into_iter()
                    .map(|snapshot| {
                        (
                            snapshot.id,
                            format!("native:{:?}", snapshot.state),
                            snapshot.transferred_bytes,
                            snapshot.current_bytes_per_second,
                            0_u32,
                        )
                    })
                    .collect();
                signature.extend(
                    vault
                        .as_ref()
                        .map(DesktopVault::transfers)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|snapshot| {
                            (
                                snapshot.id,
                                format!("vault:{:?}", snapshot.state),
                                snapshot.transferred_bytes,
                                snapshot.average_bytes_per_second,
                                snapshot.completed_parts,
                            )
                        }),
                );
                if signature == previous {
                    continue;
                }
                previous = signature;
                entity.update(cx, |_, cx| cx.notify());
            }
        }));
    }

    pub(super) fn start_storage_metrics_refresh(&mut self, cx: &mut Context<Self>) {
        let Some(library) = self.library.clone() else {
            return;
        };
        let volume_library = library.clone();
        self.volume_space_task = Some(cx.spawn(async move |this, cx| {
            loop {
                let library = volume_library.clone();
                let space = cx
                    .background_spawn(async move { library.download_volume_space().ok() })
                    .await;
                let Some(entity) = this.upgrade() else { return };
                entity.update(cx, |this, cx| {
                    this.volume_space = space;
                    cx.notify();
                });
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(5))
                    .await;
            }
        }));
        self.storage_metrics_task = Some(cx.spawn(async move |this, cx| {
            loop {
                let library = library.clone();
                let metrics = cx
                    .background_spawn(async move { library.managed_storage_metrics() })
                    .await
                    .ok();
                let Some(entity) = this.upgrade() else { return };
                entity.update(cx, |this, cx| {
                    this.overall_storage_metrics = metrics;
                    cx.notify();
                });
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(15))
                    .await;
            }
        }));
    }
}
