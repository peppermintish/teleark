//! Private-channel setup and discovery, owned independently of route navigation.

use super::*;
use teleark_core::ApplicationErrorKind;
use teleark_runtime::{StorageChannelHealth, StorageChannelStatus};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StorageAction {
    Repair,
}

impl TeleArkApp {
    pub(crate) fn storage_waiting_for_retry(&self) -> bool {
        self.storage_retry_task.is_some()
    }

    pub(crate) fn refresh_storage_channel(&mut self, cx: &mut Context<Self>) {
        self.manage_storage_channel(0, cx);
    }

    pub(crate) fn confirm_storage_maintenance(&mut self, cx: &mut Context<Self>) {
        let Some(_action) = self.storage_confirmation.take() else {
            return;
        };
        if self.storage_loading {
            return;
        }
        self.storage_details_expanded = false;
        let progress = teleark_runtime::StorageMaintenance::new();
        self.storage_maintenance = Some(progress.clone());
        self.storage_loading = true;
        self.storage_error = None;
        cx.notify();
        if self.visual_preview {
            self.storage_loading = false;
            self.storage_maintenance = None;
            self.storage_notice = Some("storage-maintenance-preview");
            cx.notify();
            return;
        }
        let (Some(telegram), Some(library), Some(account)) = (
            self.telegram.clone(),
            self.library.clone(),
            self.telegram_account.clone(),
        ) else {
            self.storage_loading = false;
            return;
        };
        let account_id = account.id;
        let generation = self.telegram_login_generation;
        let title = self.tr("storage-remote-title").to_string();
        let description = self.tr("storage-remote-description").to_string();
        // Presentation-only ticks expose phase duration and cancellation while
        // the retained background task owns network and storage operations.
        self.storage_maintenance_presentation = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(200))
                    .await;
                let Some(entity) = this.upgrade() else { return };
                let done = entity.update(cx, |this, cx| {
                    if this.telegram_login_generation != generation {
                        return true;
                    }
                    cx.notify();
                    this.storage_maintenance
                        .as_ref()
                        .is_none_or(|p| p.snapshot().finished)
                        || !this.storage_loading
                });
                if done {
                    return;
                }
            }
        }));
        let work = cx.background_spawn(async move {
            telegram.maintain_storage_channel(&library, account_id, title, description, progress)
        });
        self.storage_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let Some(entity) = this.upgrade() else { return };
            entity.update(cx, |this, cx| {
                if this.telegram_login_generation != generation
                    || this.telegram_account.as_ref().map(|a| a.id) != Some(account_id)
                {
                    return;
                }
                this.storage_loading = false;
                this.storage_maintenance_presentation = None;
                match result {
                    Ok(()) => {
                        this.storage_notice = Some("storage-repair-completed");
                        this.refresh_storage_channel(cx);
                    }
                    Err(error) => {
                        this.storage_error = Some(error.kind());
                    }
                }
                cx.notify();
            });
        }));
    }

    fn manage_storage_channel(&mut self, attempt: u32, cx: &mut Context<Self>) {
        if !self.dialogs.ready && !self.visual_preview {
            if self.dialogs.needs_initial_load() {
                self.load_telegram_dialogs(cx);
            }
            return;
        }
        if self.storage_loading {
            return;
        }
        self.storage_retry_task = None;
        let (Some(telegram), Some(library), Some(account)) = (
            self.telegram.clone(),
            self.library.clone(),
            self.telegram_account.clone(),
        ) else {
            return;
        };
        let account_id = account.id;
        let generation = self.telegram_login_generation;
        let title = self.tr("storage-remote-title").to_string();
        let description = self.tr("storage-remote-description").to_string();
        self.storage_details_expanded = false;
        self.storage_loading = true;
        self.storage_error = None;
        cx.notify();
        let work = cx.background_spawn(async move {
            let managed =
                telegram.ensure_storage_channel(&library, account_id, title, description)?;
            library.save_telegram_sources(&account, std::slice::from_ref(&managed.channel))?;
            Ok::<_, ApplicationError>(managed)
        });
        self.storage_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let Some(this) = this.upgrade() else { return };
            this.update(cx, |this, cx| {
                if this.telegram_login_generation != generation
                    || this.telegram_account.as_ref().map(|a| a.id) != Some(account_id)
                {
                    return;
                }
                this.storage_loading = false;
                match result {
                    Ok(managed) => {
                        this.storage_notice = this.storage_notice.or(Some(if managed.created {
                            "storage-auto-created"
                        } else {
                            "storage-auto-found"
                        }));
                        this.storage_error = match managed.health {
                            StorageChannelHealth::AccessDenied => {
                                Some(ApplicationErrorKind::StorageAccessDenied)
                            }
                            StorageChannelHealth::UnsafeConfiguration => {
                                Some(ApplicationErrorKind::StorageConfigurationUnsafe)
                            }
                            StorageChannelHealth::UnsupportedIdentity => {
                                Some(ApplicationErrorKind::StorageIdentityUnsupported)
                            }
                            _ => None,
                        };
                        this.storage_status = if managed.health == StorageChannelHealth::Healthy {
                            StorageChannelStatus::Ready(managed.channel)
                        } else {
                            StorageChannelStatus::Degraded {
                                channel: managed.channel,
                                health: managed.health,
                            }
                        };
                        if let Some(channel) = this.storage_status.usable_channel() {
                            let chat_id = channel.id;
                            if let Some(existing) = this
                                .telegram_chats
                                .iter_mut()
                                .find(|chat| chat.id == channel.id)
                            {
                                *existing = channel.clone();
                            } else {
                                this.telegram_chats.push(channel.clone());
                            }
                            this.start_channel_sync(cx);
                            if let Some(sync) = &this.channel_sync
                                && let Err(error) = sync.watch_managed_channel(chat_id)
                            {
                                this.storage_error = Some(error.kind());
                            }
                            this.apply_managed_channel_changes(cx);
                            if this.page == Page::Storage {
                                this.select_storage(this.storage_view, cx);
                            }
                        }
                    }
                    Err(error) => {
                        // Preserve the known channel on a failed recheck. The error
                        // disables mutations without replacing the account's binding.
                        this.storage_notice = None;
                        this.storage_error = Some(error.kind());
                        let health = match error.kind() {
                            ApplicationErrorKind::StorageAccessDenied => {
                                Some(StorageChannelHealth::AccessDenied)
                            }
                            ApplicationErrorKind::StorageConfigurationUnsafe => {
                                Some(StorageChannelHealth::UnsafeConfiguration)
                            }
                            ApplicationErrorKind::StorageIdentityUnsupported => {
                                Some(StorageChannelHealth::UnsupportedIdentity)
                            }
                            _ => None,
                        };
                        if let (Some(health), Some(channel)) =
                            (health, this.storage_status.channel().cloned())
                        {
                            this.storage_status =
                                StorageChannelStatus::Degraded { channel, health };
                        }
                        if let Some(delay) = storage_retry_delay(attempt, error.kind()) {
                            this.storage_retry_task = Some(cx.spawn(async move |this, cx| {
                                cx.background_executor().timer(delay).await;
                                let Some(this) = this.upgrade() else {
                                    return;
                                };
                                this.update(cx, |this, cx| {
                                    if this.telegram_login_generation == generation
                                        && this.telegram_account.as_ref().map(|a| a.id)
                                            == Some(account_id)
                                    {
                                        this.manage_storage_channel(attempt.saturating_add(1), cx);
                                    }
                                });
                            }));
                        }
                    }
                }
                cx.notify();
            });
        }));
    }
}

fn storage_retry_delay(
    attempt: u32,
    error: teleark_core::ApplicationErrorKind,
) -> Option<Duration> {
    (matches!(
        error,
        teleark_core::ApplicationErrorKind::Network
            | teleark_core::ApplicationErrorKind::Server
            | teleark_core::ApplicationErrorKind::Conflict
    ))
    .then(|| Duration::from_secs((2_u64 << attempt.min(5)).min(60)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use teleark_core::ApplicationErrorKind;

    #[test]
    fn automatic_rechecks_are_bounded_and_do_not_retry_permanent_failures() {
        for kind in [
            ApplicationErrorKind::Network,
            ApplicationErrorKind::Server,
            ApplicationErrorKind::Conflict,
        ] {
            assert_eq!(storage_retry_delay(0, kind), Some(Duration::from_secs(2)));
            assert_eq!(storage_retry_delay(1, kind), Some(Duration::from_secs(4)));
            assert_eq!(storage_retry_delay(2, kind), Some(Duration::from_secs(8)));
            assert_eq!(
                storage_retry_delay(u32::MAX, kind),
                Some(Duration::from_secs(60))
            );
        }
        for kind in [
            ApplicationErrorKind::Authorization,
            ApplicationErrorKind::PermissionDenied,
            ApplicationErrorKind::Capacity,
            ApplicationErrorKind::Persistence,
            ApplicationErrorKind::Cancelled,
        ] {
            assert_eq!(storage_retry_delay(0, kind), None);
        }
    }
}
