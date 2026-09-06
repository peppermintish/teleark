//! Telegram-owned creation and validation of the private TeleArk destination.
//! The marker is a versioned discovery identifier, never a localized title.

use grammers_client::{peer::Peer, tl};
use grammers_session::types::PeerRef;

use crate::{
    TelegramChat, TelegramChatKind, TelegramConnection, TelegramError, TelegramErrorKind,
    map_invocation, require_authorized,
};

const STORAGE_MARKER: &str = "teleark:storage:v1";
const MAX_STORAGE_CANDIDATES: usize = 64;
const MAX_AVATAR_BYTES: usize = 1024 * 1024;

pub(super) fn is_private_storage_candidate(channel: &tl::types::Channel) -> bool {
    channel.creator
        && channel.broadcast
        && !channel.megagroup
        && !channel.left
        && !channel.min
        && !channel.gigagroup
        && channel.username.is_none()
        && channel
            .usernames
            .as_ref()
            .is_none_or(|names| names.is_empty())
}

fn has_storage_marker(about: &str) -> bool {
    about.lines().next() == Some(STORAGE_MARKER)
}

impl TelegramConnection {
    /// Returns only owned, private broadcast channels carrying our exact marker.
    /// The caller must supply a complete bounded dialog snapshot. Truncation or
    /// an incomplete inspection fails rather than being treated as absence.
    pub async fn discover_storage_channels(
        &self,
        dialogs: &[TelegramChat],
    ) -> Result<Vec<TelegramChat>, TelegramError> {
        if dialogs.len() >= crate::MAX_DIALOGS_PER_REQUEST {
            return Err(TelegramError::new(TelegramErrorKind::LimitExceeded));
        }
        let candidates: Vec<_> = dialogs
            .iter()
            .filter(|chat| chat.owned_private_broadcast)
            .collect();
        if candidates.len() > MAX_STORAGE_CANDIDATES {
            return Err(TelegramError::new(TelegramErrorKind::LimitExceeded));
        }
        let mut channels = Vec::new();
        for chat in candidates {
            if self.is_storage_channel(chat).await? {
                channels.push(chat.clone());
            }
        }
        Ok(channels)
    }

    /// Revalidates metadata with Telegram; a stale local title/flag cannot grant
    /// permission to publish an encrypted package to a public or foreign chat.
    pub async fn is_storage_channel(&self, chat: &TelegramChat) -> Result<bool, TelegramError> {
        if chat.kind != TelegramChatKind::Channel {
            return Ok(false);
        }
        let full = self
            .client
            .invoke(&tl::functions::channels::GetFullChannel {
                channel: chat.peer_ref.into(),
            })
            .await
            .map_err(map_invocation)?;
        let tl::enums::messages::ChatFull::Full(full) = full;
        let valid_peer = full.chats.iter().any(|peer| {
            matches!(peer, tl::enums::Chat::Channel(channel)
                if channel.id == chat.id && is_private_storage_candidate(channel))
        });
        Ok(valid_peer
            && matches!(full.full_chat, tl::enums::ChatFull::ChannelFull(channel)
                if channel.id == chat.id && has_storage_marker(&channel.about)))
    }

    /// Creates a private broadcast channel. No username, invitation, member,
    /// message, or public link is created. Callers discover before requesting
    /// creation, and never automatically retry this non-idempotent RPC.
    pub async fn create_storage_channel(
        &self,
        title: &str,
        description: &str,
    ) -> Result<TelegramChat, TelegramError> {
        require_authorized(&self.client).await?;
        if title.trim().is_empty()
            || title.chars().count() > 128
            || description.chars().count() > 200
        {
            return Err(TelegramError::new(TelegramErrorKind::InvalidConfiguration));
        }
        let updates = self
            .client
            .invoke(&tl::functions::channels::CreateChannel {
                broadcast: true,
                megagroup: false,
                for_import: false,
                forum: false,
                title: title.to_owned(),
                about: format!("{STORAGE_MARKER}\n{description}"),
                geo_point: None,
                address: None,
                ttl_period: None,
            })
            .await
            .map_err(map_invocation)?;
        let chats = match updates {
            tl::enums::Updates::Updates(updates) => updates.chats,
            tl::enums::Updates::Combined(updates) => updates.chats,
            _ => return Err(TelegramError::new(TelegramErrorKind::Network)),
        };
        let mut candidates = chats.into_iter().filter_map(|chat| match chat {
            tl::enums::Chat::Channel(channel) if is_private_storage_candidate(&channel) => {
                Some(channel)
            }
            _ => None,
        });
        let channel = candidates
            .next()
            .ok_or_else(|| TelegramError::new(TelegramErrorKind::Network))?;
        if candidates.next().is_some() {
            return Err(TelegramError::new(TelegramErrorKind::Network));
        }
        let peer_ref = PeerRef::from(&channel);
        Ok(TelegramChat {
            id: channel.id,
            name: channel.title,
            username: None,
            kind: TelegramChatKind::Channel,
            peer_ref,
            owned_private_broadcast: true,
        })
    }

    /// Small current account avatar, kept in memory and bounded before decoding.
    pub async fn account_avatar(&self) -> Result<Option<Vec<u8>>, TelegramError> {
        let user = self.client.get_me().await.map_err(map_invocation)?;
        let Some(photo) = Peer::User(user)
            .photo(false)
            .await
            .map_err(map_invocation)?
        else {
            return Ok(None);
        };
        let mut download = self.client.iter_download(&photo);
        let mut bytes = Vec::new();
        while let Some(chunk) = download.next().await.map_err(map_invocation)? {
            if bytes.len().saturating_add(chunk.len()) > MAX_AVATAR_BYTES {
                return Err(TelegramError::new(TelegramErrorKind::LimitExceeded));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok((!bytes.is_empty()).then_some(bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn private_channel() -> tl::types::Channel {
        tl::types::Channel {
            creator: true,
            left: false,
            broadcast: true,
            verified: false,
            megagroup: false,
            restricted: false,
            signatures: false,
            min: false,
            scam: false,
            has_link: false,
            has_geo: false,
            slowmode_enabled: false,
            call_active: false,
            call_not_empty: false,
            fake: false,
            gigagroup: false,
            noforwards: false,
            join_to_send: false,
            join_request: false,
            forum: false,
            stories_hidden: false,
            stories_hidden_min: false,
            stories_unavailable: false,
            signature_profiles: false,
            autotranslation: false,
            broadcast_messages_allowed: false,
            monoforum: false,
            forum_tabs: false,
            id: 1,
            access_hash: None,
            title: "TeleArk".into(),
            username: None,
            photo: tl::enums::ChatPhoto::Empty,
            date: 1,
            restriction_reason: None,
            admin_rights: None,
            banned_rights: None,
            default_banned_rights: None,
            participants_count: None,
            usernames: None,
            stories_max_id: None,
            color: None,
            profile_color: None,
            emoji_status: None,
            level: None,
            subscription_until_date: None,
            bot_verification_icon: None,
            send_paid_messages_stars: None,
            linked_monoforum_id: None,
        }
    }

    #[test]
    fn storage_candidates_require_full_owned_private_broadcast_metadata() {
        let channel = private_channel();
        assert!(is_private_storage_candidate(&channel));
        for change in [
            |c: &mut tl::types::Channel| c.creator = false,
            |c: &mut tl::types::Channel| c.broadcast = false,
            |c: &mut tl::types::Channel| c.megagroup = true,
            |c: &mut tl::types::Channel| c.gigagroup = true,
            |c: &mut tl::types::Channel| c.left = true,
            |c: &mut tl::types::Channel| c.min = true,
            |c: &mut tl::types::Channel| c.username = Some("public_channel".into()),
            |c: &mut tl::types::Channel| {
                c.usernames = Some(vec![
                    tl::types::Username {
                        editable: false,
                        active: true,
                        username: "public_alias".into(),
                    }
                    .into(),
                ])
            },
        ] {
            let mut altered = channel.clone();
            change(&mut altered);
            assert!(!is_private_storage_candidate(&altered));
        }
        let mut renamed = channel;
        renamed.title = "自分のファイル".into();
        assert!(is_private_storage_candidate(&renamed));
    }

    #[test]
    fn discovery_marker_is_exact_versioned_and_independent_of_title_or_locale() {
        for description in ["", "Private files", "私人文件", "自分のファイル"] {
            assert!(has_storage_marker(&format!(
                "{STORAGE_MARKER}\n{description}"
            )));
        }
        for invalid in [
            "TeleArk",
            "teleark:storage:v2",
            " teleark:storage:v1",
            "about\nteleark:storage:v1",
            "teleark:storage:v1-other",
        ] {
            assert!(!has_storage_marker(invalid));
        }
    }
}
