//! Private-channel setup and discovery, owned independently of route navigation.

use super::*;
use teleark_runtime::StorageChannelStatus;

impl TeleArkApp {
    pub(crate) fn refresh_storage_channel(&mut self, cx: &mut Context<Self>) {
        self.manage_storage_channel(0, cx);
    }

    fn manage_storage_channel(&mut self, attempt: u8, cx: &mut Context<Self>) {
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
                        this.storage_notice = Some(if managed.created {
                            "storage-auto-created"
                        } else {
                            "storage-auto-found"
                        });
                        this.storage_status = StorageChannelStatus::Ready(managed.channel);
                        if let StorageChannelStatus::Ready(channel) = &this.storage_status {
                            if let Some(existing) = this
                                .telegram_chats
                                .iter_mut()
                                .find(|chat| chat.id == channel.id)
                            {
                                *existing = channel.clone();
                            } else {
                                this.telegram_chats.push(channel.clone());
                            }
                            if this.page == Page::Storage {
                                this.select_storage(this.storage_view, cx);
                            }
                        }
                    }
                    Err(error) => {
                        this.storage_status = StorageChannelStatus::Missing;
                        this.storage_notice = None;
                        this.storage_error = Some(error.kind());
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
                                        this.manage_storage_channel(attempt + 1, cx);
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

fn storage_retry_delay(attempt: u8, error: teleark_core::ApplicationErrorKind) -> Option<Duration> {
    (attempt < 2
        && matches!(
            error,
            teleark_core::ApplicationErrorKind::Network
                | teleark_core::ApplicationErrorKind::Conflict
        ))
    .then(|| Duration::from_secs(2_u64 << attempt))
}

#[cfg(test)]
mod tests {
    use super::*;
    use teleark_core::ApplicationErrorKind;

    #[test]
    fn automatic_rechecks_are_bounded_and_do_not_retry_permanent_failures() {
        for kind in [
            ApplicationErrorKind::Network,
            ApplicationErrorKind::Conflict,
        ] {
            assert_eq!(storage_retry_delay(0, kind), Some(Duration::from_secs(2)));
            assert_eq!(storage_retry_delay(1, kind), Some(Duration::from_secs(4)));
            assert_eq!(storage_retry_delay(2, kind), None);
            assert_eq!(storage_retry_delay(u8::MAX, kind), None);
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
