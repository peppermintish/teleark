//! Private-channel setup and discovery, owned independently of route navigation.

use super::*;
use teleark_runtime::StorageChannelStatus;

#[derive(Clone, Copy)]
enum StorageAction {
    Discover,
    Create,
    Select(i64),
}

impl TeleArkApp {
    pub(crate) fn refresh_storage_channel(&mut self, cx: &mut Context<Self>) {
        self.run_storage_action(StorageAction::Discover, cx);
    }

    pub(crate) fn create_storage_channel(&mut self, cx: &mut Context<Self>) {
        self.run_storage_action(StorageAction::Create, cx);
    }

    pub(crate) fn choose_storage_channel(&mut self, chat_id: i64, cx: &mut Context<Self>) {
        self.run_storage_action(StorageAction::Select(chat_id), cx);
    }

    fn run_storage_action(&mut self, action: StorageAction, cx: &mut Context<Self>) {
        if self.storage_loading {
            return;
        }
        let (Some(telegram), Some(library), Some(account)) = (
            self.telegram.clone(),
            self.library.clone(),
            self.telegram_account.clone(),
        ) else {
            return;
        };
        let account_id = account.id;
        let generation = self.telegram_login_generation;
        let description = self.tr("storage-remote-description").to_string();
        self.storage_loading = true;
        self.storage_error = None;
        cx.notify();
        let work = cx.background_spawn(async move {
            let status = match action {
                StorageAction::Discover => {
                    telegram.discover_storage_channel(&library, account_id)?
                }
                StorageAction::Create => telegram.create_storage_channel(
                    &library,
                    account_id,
                    "TeleArk".into(),
                    description,
                )?,
                StorageAction::Select(chat_id) => StorageChannelStatus::Ready(
                    telegram.select_storage_channel(&library, account_id, chat_id)?,
                ),
            };
            if let StorageChannelStatus::Ready(channel) = &status {
                library.save_telegram_sources(&account, std::slice::from_ref(channel))?;
            }
            Ok::<_, ApplicationError>(status)
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
                    Ok(status) => {
                        this.storage_status = status;
                        if let StorageChannelStatus::Ready(channel) = &this.storage_status {
                            if !this.telegram_chats.iter().any(|chat| chat.id == channel.id) {
                                this.telegram_chats.push(channel.clone());
                            }
                            if this.page == Page::Storage {
                                this.select_storage(this.storage_view, cx);
                            }
                        }
                    }
                    Err(error) => this.storage_error = Some(error.kind()),
                }
                cx.notify();
            });
        }));
    }
}
