//! Telegram-owned creation and validation of the private TeleArk destination.
//! The marker is a versioned discovery identifier, never a localized title.

use grammers_client::{message::InputMessage, peer::Peer, tl};
use grammers_session::types::PeerRef;

use crate::{
    TelegramChat, TelegramChatKind, TelegramConnection, TelegramError, TelegramErrorKind,
    map_invocation, require_authorized,
};

const STORAGE_MARKER: &str = "teleark:storage:v1";
const STORAGE_MARKER_V2: &str = "teleark:storage:v2";
const STORAGE_TITLE: &str = "🔒 TeleArk · Managed Storage";
const IDENTITY_MAGIC: &str = "teleark:channel-identity:v1";

/// Management records are repairable; access and privacy failures block writes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageChannelHealth {
    Healthy,
    IdentityMissing,
    IdentityUnpinned,
    IdentityInvalid,
    UnsafeConfiguration,
    /// The formerly bound peer is no longer available to this account.
    Unavailable,
    AccessDenied,
    UnsupportedIdentity,
}
impl StorageChannelHealth {
    pub const fn permits_files(self) -> bool {
        matches!(
            self,
            Self::Healthy | Self::IdentityMissing | Self::IdentityUnpinned | Self::IdentityInvalid
        )
    }
    pub const fn needs_repair(self) -> bool {
        matches!(
            self,
            Self::IdentityMissing | Self::IdentityUnpinned | Self::IdentityInvalid
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageMaintenancePhase {
    Checking,
    FindingRecord,
    Repairing,
    Pinning,
    Updating,
    Verifying,
    Muting,
    Archiving,
    Completed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RemoteIdentity {
    Unrelated,
    Unavailable,
    Legacy,
    Verified(i32),
}

fn identity_text(account: i64, channel: i64, warning: &str) -> String {
    format!("{IDENTITY_MAGIC}\naccount:{account}\nchannel:{channel}\n\n{warning}")
}

fn private_configuration(
    participants: Option<i32>,
    admins: Option<i32>,
    bots: usize,
    linked: Option<i64>,
    ttl: Option<i32>,
) -> bool {
    participants == Some(1)
        && admins == Some(1)
        && bots == 0
        && linked.is_none()
        && ttl.is_none_or(|seconds| seconds == 0)
}

fn identity_matches(text: &str, account: i64, channel: i64) -> bool {
    let prefix = format!("{IDENTITY_MAGIC}\naccount:{account}\nchannel:{channel}\n\n");
    text.len() <= 2048
        && text
            .strip_prefix(&prefix)
            .is_some_and(|body| !body.trim().is_empty())
}

fn identity_pointer(about: &str) -> Option<i32> {
    let mut lines = about.lines();
    if lines.next()? != STORAGE_MARKER_V2 {
        return None;
    }
    let raw = lines.next()?.strip_prefix("identity:")?;
    raw.parse::<i32>()
        .ok()
        .filter(|id| *id > 0 && id.to_string() == raw)
}

fn managed_about(id: i32, description: &str) -> String {
    format!("{STORAGE_MARKER_V2}\nidentity:{id}\n{description}")
}
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

fn storage_about(description: &str) -> String {
    format!("{STORAGE_MARKER}\n{description}")
}

fn validate_branding(title: &str, description: &str) -> Result<(), TelegramError> {
    if title != STORAGE_TITLE
        || title.chars().count() > 128
        || description.trim().is_empty()
        || description.chars().count() > 200
    {
        return Err(TelegramError::new(TelegramErrorKind::InvalidConfiguration));
    }
    Ok(())
}

impl TelegramConnection {
    /// Returns remote-verified storage and recognizable legacy upgrade candidates.
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
            .filter(|chat| {
                chat.kind == TelegramChatKind::Channel
                    && (chat.owned_channel || chat.name == STORAGE_TITLE || chat.name == "TeleArk")
            })
            .collect();
        if candidates.len() > MAX_STORAGE_CANDIDATES {
            return Err(TelegramError::new(TelegramErrorKind::LimitExceeded));
        }
        let account = self.current_account().await?.id;
        let mut channels = Vec::new();
        // Independent, read-only inspections share the connection with a fixed
        // fan-out. No new tasks outlive the owning bounded request.
        for group in candidates.chunks(4) {
            let inspect = |index: usize| async move {
                let Some(chat) = group.get(index) else {
                    return Ok(None);
                };
                let identity = self.inspect_storage_identity(chat, account).await?;
                Ok::<_, TelegramError>(
                    matches!(
                        identity,
                        RemoteIdentity::Legacy | RemoteIdentity::Verified(_)
                    )
                    .then(|| (*chat).clone()),
                )
            };
            let (a, b, c, d) = tokio::try_join!(inspect(0), inspect(1), inspect(2), inspect(3))?;
            channels.extend([a, b, c, d].into_iter().flatten());
        }
        Ok(channels)
    }

    /// Remote metadata and the pinned identity record are the sole authority.
    pub async fn is_storage_channel(&self, chat: &TelegramChat) -> Result<bool, TelegramError> {
        let account = self.current_account().await?.id;
        Ok(matches!(
            self.inspect_storage_identity(chat, account).await?,
            RemoteIdentity::Verified(_)
        ))
    }

    async fn inspect_storage_identity(
        &self,
        chat: &TelegramChat,
        account: i64,
    ) -> Result<RemoteIdentity, TelegramError> {
        if chat.kind != TelegramChatKind::Channel {
            return Ok(RemoteIdentity::Unrelated);
        }
        let tl::enums::messages::ChatFull::Full(full) = match self
            .client
            .invoke(&tl::functions::channels::GetFullChannel {
                channel: chat.peer_ref.into(),
            })
            .await
        {
            Ok(full) => full,
            Err(error) if resilience::channel_is_unavailable(&error) => {
                return Ok(RemoteIdentity::Unavailable);
            }
            Err(error) => return Err(map_invocation(error)),
        };
        let tl::enums::ChatFull::ChannelFull(details) = full.full_chat else {
            return Err(TelegramError::new(TelegramErrorKind::PermissionDenied));
        };
        if full.chats.iter().any(
            |peer| matches!(peer, tl::enums::Chat::ChannelForbidden(channel) if channel.id == chat.id),
        ) {
            return Ok(RemoteIdentity::Unavailable);
        }
        let channel = full
            .chats
            .iter()
            .find_map(|peer| match peer {
                tl::enums::Chat::Channel(channel) if channel.id == chat.id => Some(channel),
                _ => None,
            })
            .ok_or_else(|| TelegramError::new(TelegramErrorKind::PermissionDenied))?;
        let marked = details.about.starts_with("teleark:");
        let named = channel.title == STORAGE_TITLE || channel.title == "TeleArk";
        if !marked && !named {
            return Ok(RemoteIdentity::Unrelated);
        }
        if details.id != chat.id
            || !is_private_storage_candidate(channel)
            || !private_configuration(
                details.participants_count,
                details.admins_count,
                details.bot_info.len(),
                details.linked_chat_id,
                details.ttl_period,
            )
        {
            return Err(TelegramError::new(TelegramErrorKind::PermissionDenied));
        }
        // Legacy remote marker plus recognizable app title may be upgraded;
        // a local binding never grants permission to repair an unmarked channel.
        if has_storage_marker(&details.about) && named {
            return Ok(RemoteIdentity::Legacy);
        }
        let id = identity_pointer(&details.about)
            .ok_or_else(|| TelegramError::new(TelegramErrorKind::PermissionDenied))?;
        let messages = self
            .client
            .get_messages_by_id(chat.peer_ref, &[id])
            .await
            .map_err(map_invocation)?;
        let valid = messages
            .first()
            .and_then(Option::as_ref)
            .is_some_and(|message| {
                message.id() == id
                    && message.pinned()
                    && message.forward_header().is_none()
                    && identity_matches(message.text(), account, chat.id)
            });
        if !valid {
            return Err(TelegramError::new(TelegramErrorKind::PermissionDenied));
        }
        Ok(RemoteIdentity::Verified(id))
    }

    /// The remote marker and owner/private metadata must be verified before
    /// migrating legacy storage or repairing its recognizable presentation.
    pub async fn ensure_storage_branding(
        &self,
        chat: &TelegramChat,
        title: &str,
        description: &str,
    ) -> Result<TelegramChat, TelegramError> {
        validate_branding(title, description)?;
        let account = self.current_account().await?.id;
        let identity = self.inspect_storage_identity(chat, account).await?;
        let id = match identity {
            RemoteIdentity::Unrelated | RemoteIdentity::Unavailable => {
                return Err(TelegramError::new(TelegramErrorKind::PermissionDenied));
            }
            RemoteIdentity::Verified(id) => id,
            RemoteIdentity::Legacy => {
                // Recover an interrupted initialization from remote history.
                let mut history = self
                    .client
                    .search_messages(chat.peer_ref)
                    .query(IDENTITY_MAGIC)
                    .limit(64);
                let mut existing = None;
                let mut inspected = 0;
                while let Some(message) = history.next().await.map_err(map_invocation)? {
                    inspected += 1;
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
                    if inspected == 64 {
                        return Err(TelegramError::new(TelegramErrorKind::LimitExceeded));
                    }
                    self.client
                        .send_message(
                            chat.peer_ref,
                            InputMessage::new().text(identity_text(account, chat.id, description)),
                        )
                        .await
                        .map_err(map_invocation)?
                        .id()
                };
                self.client
                    .invoke(&tl::functions::messages::UpdatePinnedMessage {
                        silent: true,
                        unpin: false,
                        pm_oneside: false,
                        peer: chat.peer_ref.into(),
                        id,
                    })
                    .await
                    .map_err(map_invocation)?;
                id
            }
        };
        let tl::enums::messages::ChatFull::Full(full) = self
            .client
            .invoke(&tl::functions::channels::GetFullChannel {
                channel: chat.peer_ref.into(),
            })
            .await
            .map_err(map_invocation)?;
        if !full.chats.iter().any(|peer| matches!(peer, tl::enums::Chat::Channel(channel) if channel.id == chat.id && is_private_storage_candidate(channel))) {
            return Err(TelegramError::new(TelegramErrorKind::PermissionDenied));
        }
        let title_matches = full.chats.iter().any(|peer| matches!(peer, tl::enums::Chat::Channel(channel) if channel.id == chat.id && channel.title == title && is_private_storage_candidate(channel)));
        let about = managed_about(id, description);
        let about_matches = matches!(full.full_chat, tl::enums::ChatFull::ChannelFull(details) if details.id == chat.id && details.about == about);
        if !title_matches {
            self.client
                .invoke(&tl::functions::channels::EditTitle {
                    channel: chat.peer_ref.into(),
                    title: title.to_owned(),
                })
                .await
                .map_err(map_invocation)?;
        }
        if !about_matches {
            self.client
                .invoke(&tl::functions::messages::EditChatAbout {
                    peer: chat.peer_ref.into(),
                    about,
                })
                .await
                .map_err(map_invocation)?;
        }
        if !self.is_storage_channel(chat).await? {
            return Err(TelegramError::new(TelegramErrorKind::PermissionDenied));
        }
        let mut branded = chat.clone();
        branded.name = title.to_owned();
        Ok(branded)
    }

    /// Creates a private broadcast channel with the legacy bootstrap marker.
    /// The manager must publish and verify the remote identity before use.
    /// No username, invitation, member, or public link is created. Callers discover before requesting
    /// creation, and never automatically retry this non-idempotent RPC.
    pub async fn create_storage_channel(
        &self,
        title: &str,
        description: &str,
    ) -> Result<TelegramChat, TelegramError> {
        require_authorized(&self.client).await?;
        validate_branding(title, description)?;
        let updates = self
            .client
            .invoke(&tl::functions::channels::CreateChannel {
                broadcast: true,
                megagroup: false,
                for_import: false,
                forum: false,
                title: title.to_owned(),
                about: storage_about(description),
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
            sync_pts: None,
            id: channel.id,
            name: channel.title,
            username: None,
            kind: TelegramChatKind::Channel,
            peer_ref,
            owned_channel: true,
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
    fn remote_privacy_requires_positive_evidence_and_no_automatic_deletion() {
        assert!(private_configuration(Some(1), Some(1), 0, None, None));
        assert!(private_configuration(Some(1), Some(1), 0, None, Some(0)));
        for (members, admins, bots, linked, ttl) in [
            (None, Some(1), 0, None, None),
            (Some(1), None, 0, None, None),
            (Some(2), Some(1), 0, None, None),
            (Some(1), Some(2), 0, None, None),
            (Some(1), Some(1), 1, None, None),
            (Some(1), Some(1), 0, Some(99), None),
            (Some(1), Some(1), 0, None, Some(86400)),
            (Some(1), Some(1), 0, None, Some(-1)),
        ] {
            assert!(!private_configuration(members, admins, bots, linked, ttl));
        }
    }

    #[test]
    fn remote_identity_has_explicit_compatible_text_fixtures() {
        let warning = "Created and managed by TeleArk. Do not edit or delete.";
        assert_eq!(
            identity_text(100, 700, warning),
            include_str!("../tests/fixtures/storage-identity-v1.txt").trim_end_matches('\n')
        );
        assert_eq!(
            managed_about(17, warning),
            include_str!("../tests/fixtures/storage-description-v2.txt").trim_end_matches('\n')
        );
        assert!(identity_matches(
            include_str!("../tests/fixtures/storage-identity-v1.txt"),
            100,
            700
        ));
        assert_eq!(
            identity_pointer(include_str!("../tests/fixtures/storage-description-v2.txt")),
            Some(17)
        );
        assert!(has_storage_marker("teleark:storage:v1\nLegacy description"));
    }

    #[test]
    fn remote_identity_requires_exact_account_channel_version_and_pointer() {
        let record = identity_text(100, 700, "Do not edit or delete.");
        assert!(identity_matches(&record, 100, 700));
        assert!(!identity_matches(&record, 200, 700));
        assert!(!identity_matches(&record, 100, 701));
        for damaged in [
            record.replace("identity:v1", "identity:v2"),
            record.replace("account:100", "account:0100"),
            record.replace("channel:700", "channel:+700"),
            record.replace("\n", "\r\n"),
            identity_text(100, 700, " "),
        ] {
            assert!(!identity_matches(&damaged, 100, 700));
        }
        assert_eq!(identity_pointer(&managed_about(17, "warning")), Some(17));
        for invalid in [
            "teleark:storage:v2\nidentity:017",
            "teleark:storage:v2\nidentity:0",
            "teleark:storage:v2\nidentity:-1",
            "teleark:storage:v3\nidentity:17",
            "teleark:storage:v2\nidentity:2147483648",
        ] {
            assert_eq!(identity_pointer(invalid), None);
        }
        assert!(!identity_matches(
            &identity_text(100, 700, &"x".repeat(2048)),
            100,
            700
        ));
    }

    #[test]
    fn branding_preserves_discovery_marker_and_bounds_remote_input() {
        assert_eq!(
            storage_about("Managed by TeleArk"),
            "teleark:storage:v1\nManaged by TeleArk"
        );
        assert!(has_storage_marker(&storage_about("管理用の保管庫")));
        assert!(
            validate_branding("🔒 TeleArk · Managed Storage", "Do not edit or delete.").is_ok()
        );
        for (title, about) in [("", "warning"), ("TeleArk", ""), ("TeleArk", " ")] {
            assert!(validate_branding(title, about).is_err());
        }
        assert!(validate_branding(&"a".repeat(129), "warning").is_err());
        assert!(validate_branding("TeleArk", &"あ".repeat(201)).is_err());
        assert!(validate_branding(STORAGE_TITLE, &"あ".repeat(200)).is_ok());
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

mod resilience;
