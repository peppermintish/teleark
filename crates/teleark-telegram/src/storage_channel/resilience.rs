//! Repair only an explicitly bound channel after fresh account/owner/privacy checks.
use super::*;
use crate::ScanCancellation;
use grammers_mtsender::InvocationError;

fn management_health(
    about: &str,
    message: Option<(&str, bool, bool)>,
    account: i64,
    channel: i64,
) -> StorageChannelHealth {
    if about.starts_with("teleark:")
        && about.lines().next() != Some(STORAGE_MARKER_V2)
        && !has_storage_marker(about)
    {
        return StorageChannelHealth::UnsupportedIdentity;
    }
    if identity_pointer(about).is_none() {
        return StorageChannelHealth::IdentityMissing;
    }
    match message {
        None => StorageChannelHealth::IdentityMissing,
        Some((text, _, _))
            if text.starts_with("teleark:channel-identity:")
                && text.lines().next() != Some(IDENTITY_MAGIC) =>
        {
            StorageChannelHealth::UnsupportedIdentity
        }
        Some((text, _, forwarded)) if forwarded || !identity_matches(text, account, channel) => {
            StorageChannelHealth::IdentityInvalid
        }
        Some((_, false, _)) => StorageChannelHealth::IdentityUnpinned,
        Some(_) => StorageChannelHealth::Healthy,
    }
}
fn check_cancel(cancel: &ScanCancellation) -> Result<(), TelegramError> {
    if cancel.is_cancelled() {
        Err(TelegramError::new(TelegramErrorKind::Cancelled))
    } else {
        Ok(())
    }
}
fn require_safe(health: StorageChannelHealth) -> Result<(), TelegramError> {
    match health {
        StorageChannelHealth::AccessDenied => {
            Err(TelegramError::new(TelegramErrorKind::StorageAccessDenied))
        }
        StorageChannelHealth::UnsafeConfiguration => Err(TelegramError::new(
            TelegramErrorKind::StorageConfigurationUnsafe,
        )),
        StorageChannelHealth::UnsupportedIdentity => Err(TelegramError::new(
            TelegramErrorKind::StorageIdentityUnsupported,
        )),
        _ => Ok(()),
    }
}
fn map_channel_error(error: InvocationError) -> TelegramError {
    match &error {
        InvocationError::Rpc(rpc)
            if rpc.code == 403
                || matches!(
                    rpc.name.as_str(),
                    "CHANNEL_PRIVATE" | "CHANNEL_INVALID" | "CHAT_ADMIN_REQUIRED"
                ) =>
        {
            TelegramError::new(TelegramErrorKind::StorageAccessDenied)
        }
        _ => map_invocation(error),
    }
}

impl TelegramConnection {
    /// Callers must supply the persisted account/channel binding, never a display name.
    pub async fn storage_channel_health(
        &self,
        chat: &TelegramChat,
        account: i64,
    ) -> Result<StorageChannelHealth, TelegramError> {
        if chat.kind != TelegramChatKind::Channel || self.current_account().await?.id != account {
            return Ok(StorageChannelHealth::AccessDenied);
        }
        let tl::enums::messages::ChatFull::Full(full) = self
            .client
            .invoke(&tl::functions::channels::GetFullChannel {
                channel: chat.peer_ref.into(),
            })
            .await
            .map_err(map_channel_error)?;
        let tl::enums::ChatFull::ChannelFull(details) = full.full_chat else {
            return Ok(StorageChannelHealth::AccessDenied);
        };
        let Some(channel) = full.chats.iter().find_map(|p| match p {
            tl::enums::Chat::Channel(c) if c.id == chat.id => Some(c),
            _ => None,
        }) else {
            return Ok(StorageChannelHealth::AccessDenied);
        };
        if details.id != chat.id || !channel.creator || channel.left {
            return Ok(StorageChannelHealth::AccessDenied);
        }
        if !is_private_storage_candidate(channel)
            || !private_configuration(
                details.participants_count,
                details.admins_count,
                details.bot_info.len(),
                details.linked_chat_id,
                details.ttl_period,
            )
        {
            return Ok(StorageChannelHealth::UnsafeConfiguration);
        }
        let Some(id) = identity_pointer(&details.about) else {
            return Ok(management_health(&details.about, None, account, chat.id));
        };
        let messages = self
            .client
            .get_messages_by_id(chat.peer_ref, &[id])
            .await
            .map_err(map_channel_error)?;
        Ok(management_health(
            &details.about,
            messages
                .first()
                .and_then(Option::as_ref)
                .map(|m| (m.text(), m.pinned(), m.forward_header().is_some())),
            account,
            chat.id,
        ))
    }

    /// Idempotent RPCs plus a persisted SendMessage random_id allow restart/retry.
    /// Existing data messages are never edited or deleted.
    pub async fn repair_bound_storage(
        &self,
        chat: &TelegramChat,
        account: i64,
        branding: (&str, &str),
        random_id: i64,
        cancel: &ScanCancellation,
        observe: &(dyn Fn(StorageMaintenancePhase) + Send + Sync),
    ) -> Result<(), TelegramError> {
        complete_channel_repair(
            cancel,
            self.repair_storage_identity(chat, account, branding, random_id, cancel, observe),
            self.archive_bound_storage(chat, account, cancel, observe),
        )
        .await
    }

    async fn repair_storage_identity(
        &self,
        chat: &TelegramChat,
        account: i64,
        branding: (&str, &str),
        random_id: i64,
        cancel: &ScanCancellation,
        observe: &(dyn Fn(StorageMaintenancePhase) + Send + Sync),
    ) -> Result<(), TelegramError> {
        let (title, description) = branding;
        validate_branding(title, description)?;
        check_cancel(cancel)?;
        observe(StorageMaintenancePhase::Checking);
        let health = self.storage_channel_health(chat, account).await?;
        require_safe(health)?;
        if health == StorageChannelHealth::Healthy {
            observe(StorageMaintenancePhase::Verifying);
            return Ok(());
        }
        check_cancel(cancel)?;
        observe(StorageMaintenancePhase::FindingRecord);
        // Search only identity records with a hard cap. A persisted random_id
        // makes a delayed search result safe to retry after an ambiguous send.
        let mut history = self
            .client
            .search_messages(chat.peer_ref)
            .query(IDENTITY_MAGIC)
            .limit(65);
        let mut existing = None;
        let mut count = 0;
        while let Some(message) = history.next().await.map_err(map_channel_error)? {
            check_cancel(cancel)?;
            count += 1;
            if message.forward_header().is_none()
                && identity_matches(message.text(), account, chat.id)
            {
                existing = Some(message.id());
                break;
            }
        }
        let id = if let Some(id) = existing {
            id
        } else {
            if count >= 65 {
                return Err(TelegramError::new(TelegramErrorKind::LimitExceeded));
            }
            require_safe(self.storage_channel_health(chat, account).await?)?;
            check_cancel(cancel)?;
            observe(StorageMaintenancePhase::Repairing);
            let updates = self
                .client
                .invoke(&tl::functions::messages::SendMessage {
                    no_webpage: true,
                    silent: true,
                    background: false,
                    clear_draft: false,
                    noforwards: false,
                    update_stickersets_order: false,
                    invert_media: false,
                    allow_paid_floodskip: false,
                    peer: chat.peer_ref.into(),
                    reply_to: None,
                    message: identity_text(account, chat.id, description),
                    random_id,
                    reply_markup: None,
                    entities: None,
                    schedule_date: None,
                    schedule_repeat_period: None,
                    send_as: None,
                    quick_reply_shortcut: None,
                    effect: None,
                    allow_paid_stars: None,
                    suggested_post: None,
                    rich_message: None,
                })
                .await
                .map_err(map_channel_error)?;
            sent_message_id(updates, random_id)
                .ok_or_else(|| TelegramError::new(TelegramErrorKind::Network))?
        };
        check_cancel(cancel)?;
        require_safe(self.storage_channel_health(chat, account).await?)?;
        observe(StorageMaintenancePhase::Pinning);
        self.client
            .invoke(&tl::functions::messages::UpdatePinnedMessage {
                silent: true,
                unpin: false,
                pm_oneside: false,
                peer: chat.peer_ref.into(),
                id,
            })
            .await
            .map_err(map_channel_error)?;
        check_cancel(cancel)?;
        observe(StorageMaintenancePhase::Updating);
        self.client
            .invoke(&tl::functions::messages::EditChatAbout {
                peer: chat.peer_ref.into(),
                about: managed_about(id, description),
            })
            .await
            .map_err(map_channel_error)?;
        // Keep user-selected titles; the identity pointer is sufficient.
        check_cancel(cancel)?;
        observe(StorageMaintenancePhase::Verifying);
        if self.storage_channel_health(chat, account).await? != StorageChannelHealth::Healthy {
            return Err(TelegramError::new(
                TelegramErrorKind::StorageIdentityDamaged,
            ));
        }
        observe(StorageMaintenancePhase::Verifying);
        Ok(())
    }

    /// Mandatory repair defaults, re-applied on every explicit repair/retry.
    async fn archive_bound_storage(
        &self,
        chat: &TelegramChat,
        account: i64,
        cancel: &ScanCancellation,
        observe: &(dyn Fn(StorageMaintenancePhase) + Send + Sync),
    ) -> Result<(), TelegramError> {
        check_cancel(cancel)?;
        observe(StorageMaintenancePhase::Checking);
        require_safe(self.storage_channel_health(chat, account).await?)?;
        check_cancel(cancel)?;
        observe(StorageMaintenancePhase::Muting);
        self.client
            .invoke(&tl::functions::account::UpdateNotifySettings {
                peer: tl::types::InputNotifyPeer {
                    peer: chat.peer_ref.into(),
                }
                .into(),
                settings: tl::types::InputPeerNotifySettings {
                    show_previews: None,
                    silent: None,
                    mute_until: Some(i32::MAX),
                    sound: None,
                    stories_muted: None,
                    stories_hide_sender: None,
                    stories_sound: None,
                }
                .into(),
            })
            .await
            .map_err(map_channel_error)?;
        check_cancel(cancel)?;
        observe(StorageMaintenancePhase::Archiving);
        self.client
            .invoke(&tl::functions::folders::EditPeerFolders {
                folder_peers: vec![
                    tl::types::InputFolderPeer {
                        peer: chat.peer_ref.into(),
                        folder_id: 1,
                    }
                    .into(),
                ],
            })
            .await
            .map_err(map_channel_error)?;
        check_cancel(cancel)?;
        observe(StorageMaintenancePhase::Verifying);
        let tl::enums::messages::PeerDialogs::Dialogs(dialogs) = self
            .client
            .invoke(&tl::functions::messages::GetPeerDialogs {
                peers: vec![
                    tl::types::InputDialogPeer {
                        peer: chat.peer_ref.into(),
                    }
                    .into(),
                ],
            })
            .await
            .map_err(map_channel_error)?;
        let verified = dialogs.dialogs.into_iter().any(|dialog| match dialog {
            tl::enums::Dialog::Dialog(dialog) => {
                let tl::enums::PeerNotifySettings::Settings(settings) = dialog.notify_settings;
                matches!(dialog.peer, tl::enums::Peer::Channel(peer) if peer.channel_id == chat.id)
                    && dialog.folder_id == Some(1)
                    && settings.mute_until == Some(i32::MAX)
            }
            _ => false,
        });
        if !verified {
            return Err(TelegramError::new(TelegramErrorKind::Network));
        }
        Ok(())
    }
}
// Defaults are part of completion even when identity was already repaired by an
// interrupted attempt. Never poll their mutations before identity validation.
async fn complete_channel_repair(
    cancel: &ScanCancellation,
    identity: impl std::future::Future<Output = Result<(), TelegramError>>,
    defaults: impl std::future::Future<Output = Result<(), TelegramError>>,
) -> Result<(), TelegramError> {
    check_cancel(cancel)?;
    identity.await?;
    check_cancel(cancel)?;
    defaults.await
}

use crate::publication::sent_message_id;

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn repair_waits_for_required_defaults_and_propagates_failure_on_retry() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let calls = Arc::new(AtomicUsize::new(0));
        for retry in 0..2 {
            let cancel = ScanCancellation::new();
            let (identity_done, identity_wait) = tokio::sync::oneshot::channel();
            let (defaults_done, defaults_wait) = tokio::sync::oneshot::channel();
            let defaults_calls = calls.clone();
            let task = tokio::spawn(async move {
                complete_channel_repair(
                    &cancel,
                    async {
                        identity_wait.await.expect("identity released");
                        Ok(())
                    },
                    async {
                        defaults_calls.fetch_add(1, Ordering::SeqCst);
                        defaults_wait.await.expect("defaults released")
                    },
                )
                .await
            });
            tokio::task::yield_now().await;
            assert_eq!(calls.load(Ordering::SeqCst), retry);
            assert!(!task.is_finished());
            identity_done.send(()).expect("release identity");
            tokio::task::yield_now().await;
            assert_eq!(calls.load(Ordering::SeqCst), retry + 1);
            assert!(!task.is_finished(), "identity success cannot finish repair");
            if retry == 0 {
                defaults_done
                    .send(Err(TelegramError::new(TelegramErrorKind::Network)))
                    .expect("release failure");
                assert_eq!(
                    task.await
                        .expect("task")
                        .expect_err("required defaults failed")
                        .kind(),
                    TelegramErrorKind::Network
                );
            } else {
                defaults_done.send(Ok(())).expect("verified defaults");
                task.await.expect("task").expect("complete retry");
            }
        }
    }

    #[tokio::test]
    async fn failed_identity_and_cancellation_prevent_defaults() {
        let cancel = ScanCancellation::new();
        let result = complete_channel_repair(
            &cancel,
            async { Err(TelegramError::new(TelegramErrorKind::StorageAccessDenied)) },
            async { panic!("must not mutate unsafe channel") },
        )
        .await;
        assert_eq!(
            result.expect_err("identity failed").kind(),
            TelegramErrorKind::StorageAccessDenied
        );
        let result = complete_channel_repair(
            &cancel,
            async {
                cancel.cancel();
                Ok(())
            },
            async { panic!("must not mutate after cancellation") },
        )
        .await;
        assert_eq!(
            result.expect_err("cancelled").kind(),
            TelegramErrorKind::Cancelled
        );
    }

    #[test]
    fn damage_is_localized_and_newer_identity_is_preserved() {
        let about = managed_about(3, "warning");
        let text = identity_text(10, 20, "warning");
        assert_eq!(
            management_health(&about, None, 10, 20),
            StorageChannelHealth::IdentityMissing
        );
        assert_eq!(
            management_health(&about, Some((&text, false, false)), 10, 20),
            StorageChannelHealth::IdentityUnpinned
        );
        assert_eq!(
            management_health(&about, Some((&text, true, false)), 10, 21),
            StorageChannelHealth::IdentityInvalid
        );
        assert_eq!(
            management_health(&about, Some((&text, true, false)), 10, 20),
            StorageChannelHealth::Healthy
        );
        assert_eq!(
            management_health("teleark:storage:v999", None, 10, 20),
            StorageChannelHealth::UnsupportedIdentity
        );
        for health in [
            StorageChannelHealth::IdentityMissing,
            StorageChannelHealth::IdentityInvalid,
            StorageChannelHealth::IdentityUnpinned,
        ] {
            assert!(health.permits_files());
            assert!(health.needs_repair());
            assert!(require_safe(health).is_ok());
        }
        for health in [
            StorageChannelHealth::AccessDenied,
            StorageChannelHealth::UnsafeConfiguration,
            StorageChannelHealth::UnsupportedIdentity,
        ] {
            assert!(!health.permits_files());
            assert!(require_safe(health).is_err());
        }
    }
    #[test]
    fn cancellation_is_checked_before_mutations() {
        let cancel = ScanCancellation::new();
        cancel.cancel();
        assert_eq!(
            check_cancel(&cancel).expect_err("cancelled").kind(),
            TelegramErrorKind::Cancelled
        );
    }
}
