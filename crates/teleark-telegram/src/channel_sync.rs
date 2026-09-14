//! Official MTProto channel differences. Transport hints never become durable
//! coverage; only a caller's committed difference advances its PTS.
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
};

use super::*;

const MAX_DIRTY_CHANNELS: usize = 10_000;
const MAX_PENDING_PUSHES: usize = 512;
const MAX_PENDING_BYTES: usize = 4 * 1024 * 1024;

/// Normalized metadata only: no grammers entities or media bytes escape into the sync queue.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChannelFileUpdate {
    pub message_id: i64,
    pub sent_at_unix_ms: i64,
    pub modified_at_unix_ms: i64,
    pub file_name: String,
    pub caption: String,
    pub mime_type: Option<String>,
    pub size_bytes: u64,
}

#[derive(Clone, Debug)]
pub struct ChannelPush {
    pub channel_id: i64,
    pub pts: i32,
    pub pts_count: i32,
    pub files: Vec<ChannelFileUpdate>,
    pub removed: Vec<i64>,
    pub edited: Vec<i64>,
}

impl ChannelPush {
    pub fn estimated_bytes(&self) -> usize {
        64 + 8 * (self.removed.len() + self.edited.len())
            + self
                .files
                .iter()
                .map(|f| {
                    64 + f.file_name.len()
                        + f.caption.len()
                        + f.mime_type.as_ref().map_or(0, String::len)
                })
                .sum::<usize>()
    }
}

/// A wake carries only routing information. Consumers must remain nonblocking;
/// durable state changes still belong to the account synchronization owner.
pub type ChannelWake = Arc<dyn Fn(Option<i64>, bool) + Send + Sync>;

#[derive(Default)]
struct PendingUpdates {
    hints: ChannelUpdateHints,
    wake: Option<ChannelWake>,
}

#[derive(Clone, Debug, Default)]
pub struct ChannelUpdateHints {
    pub channels: BTreeMap<i64, i32>,
    pub pushes: VecDeque<ChannelPush>,
    pending_bytes: usize,
    pub reconcile_all: bool,
    pub metadata_changed: bool,
    pub overflow_count: u64,
}

#[derive(Clone, Default)]
pub struct ChannelUpdateSignals(Arc<Mutex<PendingUpdates>>);

impl ChannelUpdateSignals {
    pub fn take(&self) -> ChannelUpdateHints {
        match self.0.lock() {
            Ok(mut pending) => std::mem::take(&mut pending.hints),
            Err(_) => ChannelUpdateHints {
                reconcile_all: true,
                ..Default::default()
            },
        }
    }

    pub fn set_waker(&self, wake: Option<ChannelWake>) {
        let ready = if let Ok(mut pending) = self.0.lock() {
            pending.wake = wake.clone();
            !pending.hints.channels.is_empty()
                || pending.hints.reconcile_all
                || pending.hints.metadata_changed
        } else {
            true
        };
        if ready && let Some(wake) = wake {
            wake(None, false);
        }
    }

    /// A retiring consumer must not clear a replacement consumer's wake callback.
    pub fn clear_waker(&self, expected: &ChannelWake) {
        if let Ok(mut pending) = self.0.lock()
            && pending
                .wake
                .as_ref()
                .is_some_and(|wake| Arc::ptr_eq(wake, expected))
        {
            pending.wake = None;
        }
    }

    fn channel(hints: &mut ChannelUpdateHints, id: i64, pts: i32) {
        if !hints.channels.contains_key(&id) && hints.channels.len() >= MAX_DIRTY_CHANNELS {
            hints.reconcile_all = true;
            hints.overflow_count = hints.overflow_count.saturating_add(1);
            return;
        }
        hints
            .channels
            .entry(id)
            .and_modify(|value| {
                *value = if *value == 0 || pts == 0 {
                    0
                } else {
                    (*value).max(pts)
                }
            })
            .or_insert(pts);
    }

    pub(super) fn observe(&self, value: &UpdatesLike) {
        let Ok(mut pending) = self.0.lock() else {
            return;
        };
        let hints = &mut pending.hints;
        let mut activity = Vec::new();
        match value {
            UpdatesLike::Updates(tl::enums::Updates::Updates(value)) => {
                for update in &value.updates {
                    if let Some(changed) = observe_update(hints, update) {
                        activity.push(changed);
                    }
                }
            }
            UpdatesLike::Updates(tl::enums::Updates::Combined(value)) => {
                for update in &value.updates {
                    if let Some(changed) = observe_update(hints, update) {
                        activity.push(changed);
                    }
                }
            }
            UpdatesLike::Updates(tl::enums::Updates::UpdateShort(value)) => {
                if let Some(changed) = observe_update(hints, &value.update) {
                    activity.push(changed);
                }
            }
            UpdatesLike::AffectedChannelMessages {
                affected,
                channel_id,
                ..
            } => {
                Self::channel(hints, *channel_id, affected.pts);
                activity.push((Some(*channel_id), false));
            }
            UpdatesLike::Updates(tl::enums::Updates::TooLong)
            | UpdatesLike::ConnectionClosed
            | UpdatesLike::MalformedUpdates => {
                hints.reconcile_all = true;
                activity.push((None, false));
            }
            _ => {}
        }
        let wake = pending.wake.clone();
        drop(pending);
        if let Some(wake) = wake {
            for (chat, mutation) in activity {
                wake(chat, mutation);
            }
        }
    }
}

fn observe_update(
    hints: &mut ChannelUpdateHints,
    update: &tl::enums::Update,
) -> Option<(Option<i64>, bool)> {
    let (id, pts, count, message, edited, removed) = match update {
        tl::enums::Update::NewChannelMessage(u) => (
            message_channel(&u.message)?,
            u.pts,
            u.pts_count,
            Some(&u.message),
            false,
            vec![],
        ),
        tl::enums::Update::EditChannelMessage(u) => (
            message_channel(&u.message)?,
            u.pts,
            u.pts_count,
            Some(&u.message),
            true,
            vec![],
        ),
        tl::enums::Update::DeleteChannelMessages(u) => (
            u.channel_id,
            u.pts,
            u.pts_count,
            None,
            false,
            u.messages.iter().copied().map(i64::from).collect(),
        ),
        tl::enums::Update::Channel(u) => {
            hints.metadata_changed = true;
            return Some((Some(u.channel_id), false));
        }
        tl::enums::Update::ChannelTooLong(u) => {
            ChannelUpdateSignals::channel(hints, u.channel_id, 0);
            return Some((Some(u.channel_id), false));
        }
        _ => return None,
    };
    ChannelUpdateSignals::channel(hints, id, pts);
    let mutation = edited || !removed.is_empty();
    let mut changes = BTreeMap::new();
    let mut edited_ids = Vec::new();
    if let Some(message) = message {
        if edited && let tl::enums::Message::Message(m) = message {
            edited_ids.push(i64::from(m.id));
        }
        if record_message(id, message.clone(), edited, &mut changes).is_err() {
            ChannelUpdateSignals::channel(hints, id, 0);
            return Some((Some(id), mutation));
        }
    }
    let mut push = ChannelPush {
        channel_id: id,
        pts,
        pts_count: count,
        files: Vec::new(),
        removed,
        edited: edited_ids,
    };
    for (message, file) in changes {
        if let Some(file) = file {
            push.files.push(file);
        } else {
            push.removed.push(message);
        }
    }
    let bytes = push.estimated_bytes();
    if count <= 0
        || pts <= 0
        || push.removed.iter().any(|id| *id <= 0)
        || hints.pushes.len() >= MAX_PENDING_PUSHES
        || hints.pending_bytes.saturating_add(bytes) > MAX_PENDING_BYTES
    {
        // Fall back to the committed PTS. Never silently discard change coverage.
        hints.overflow_count = hints.overflow_count.saturating_add(1);
        ChannelUpdateSignals::channel(hints, id, 0);
    } else if !hints
        .pushes
        .iter()
        .any(|p| p.channel_id == id && p.pts == pts)
    {
        hints.pending_bytes += bytes;
        hints.pushes.push_back(push);
    }
    Some((Some(id), mutation))
}

fn message_channel(message: &tl::enums::Message) -> Option<i64> {
    let peer = match message {
        tl::enums::Message::Message(message) => Some(&message.peer_id),
        tl::enums::Message::Service(message) => Some(&message.peer_id),
        tl::enums::Message::Empty(message) => message.peer_id.as_ref(),
    };
    match peer {
        Some(tl::enums::Peer::Channel(peer)) => Some(peer.channel_id),
        _ => None,
    }
}

#[derive(Clone, Debug)]
pub struct ChannelDifferencePage {
    pub files: Vec<ChannelFileUpdate>,
    pub removed: Vec<i64>,
    pub edited: Vec<i64>,
    pub timeout_seconds: Option<u32>,
    pub pts: i32,
    pub complete: bool,
    pub history_gap: bool,
}

impl TelegramConnection {
    pub fn channel_update_signals(&self) -> ChannelUpdateSignals {
        self.channel_updates.clone()
    }

    pub async fn channel_difference(
        &self,
        chat: &TelegramChat,
        pts: i32,
        cancellation: &ScanCancellation,
    ) -> Result<ChannelDifferencePage, TelegramError> {
        if chat.kind != TelegramChatKind::Channel || pts <= 0 {
            return Err(TelegramError::new(TelegramErrorKind::InvalidConfiguration));
        }
        let request = tl::functions::updates::GetChannelDifference {
            force: false,
            channel: tl::types::InputChannel {
                channel_id: chat.id,
                access_hash: chat.peer_ref.auth.hash(),
            }
            .into(),
            filter: tl::enums::ChannelMessagesFilter::Empty,
            pts,
            limit: 100,
        };
        let response = tokio::select! {
            _ = cancellation.cancelled() => return Err(TelegramError::new(TelegramErrorKind::Cancelled)),
            response = self.sync_client.invoke(&request) => response.map_err(map_invocation)?,
        };
        normalize_difference(chat.id, pts, response)
    }

    pub async fn verify_channel_messages(
        &self,
        chat: &TelegramChat,
        ids: &[i64],
        cancellation: &ScanCancellation,
    ) -> Result<(Vec<TelegramFile>, Vec<i64>), TelegramError> {
        if chat.kind != TelegramChatKind::Channel {
            return Err(TelegramError::new(TelegramErrorKind::InvalidConfiguration));
        }
        validate_limit(ids.len(), 100)?;
        let ids32 = ids
            .iter()
            .map(|id| {
                i32::try_from(*id).map_err(|_| TelegramError::new(TelegramErrorKind::LimitExceeded))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let messages = tokio::select! {
            _ = cancellation.cancelled() => return Err(TelegramError::new(TelegramErrorKind::Cancelled)),
            messages = self.sync_client.get_messages_by_id(chat.peer_ref, &ids32) => messages.map_err(map_invocation)?,
        };
        if messages.len() != ids.len() {
            return Err(TelegramError::new(TelegramErrorKind::Network));
        }
        let mut files = Vec::new();
        let mut removed = Vec::new();
        for (id, message) in ids.iter().zip(messages) {
            match message.map(file_from_message).transpose()?.flatten() {
                Some(file) if file.message_id == *id => files.push(file),
                None => removed.push(*id),
                _ => return Err(TelegramError::new(TelegramErrorKind::Network)),
            }
        }
        Ok((files, removed))
    }
}

fn normalize_difference(
    chat_id: i64,
    previous_pts: i32,
    response: tl::enums::updates::ChannelDifference,
) -> Result<ChannelDifferencePage, TelegramError> {
    let mut result = ChannelDifferencePage {
        files: Vec::new(),
        removed: Vec::new(),
        edited: Vec::new(),
        timeout_seconds: None,
        pts: previous_pts,
        complete: false,
        history_gap: false,
    };
    let (messages, updates) = match response {
        tl::enums::updates::ChannelDifference::Empty(value) => {
            result.pts = value.pts;
            result.complete = value.r#final;
            result.timeout_seconds = value.timeout.map(|seconds| seconds.max(1) as u32);
            (Vec::new(), Vec::new())
        }
        tl::enums::updates::ChannelDifference::Difference(value) => {
            result.pts = value.pts;
            result.complete = value.r#final;
            result.timeout_seconds = value.timeout.map(|seconds| seconds.max(1) as u32);
            (value.new_messages, value.other_updates)
        }
        tl::enums::updates::ChannelDifference::TooLong(value) => {
            let tl::enums::Dialog::Dialog(dialog) = value.dialog else {
                return Err(TelegramError::new(TelegramErrorKind::Network));
            };
            if !matches!(dialog.peer, tl::enums::Peer::Channel(ref peer) if peer.channel_id == chat_id)
            {
                return Err(TelegramError::new(TelegramErrorKind::Network));
            }
            result.pts = dialog
                .pts
                .ok_or_else(|| TelegramError::new(TelegramErrorKind::Network))?;
            result.complete = value.r#final;
            result.timeout_seconds = value.timeout.map(|seconds| seconds.max(1) as u32);
            result.history_gap = true;
            (value.messages, Vec::new())
        }
    };
    if result.pts < previous_pts
        || (!result.complete && result.pts == previous_pts)
        || messages.len() + updates.len() > 2_000
    {
        return Err(TelegramError::new(TelegramErrorKind::Network));
    }
    let mut changes = BTreeMap::new();
    for message in messages {
        record_message(chat_id, message, false, &mut changes)?;
    }
    for update in updates {
        match update {
            tl::enums::Update::NewChannelMessage(update) => {
                record_message(chat_id, update.message, false, &mut changes)?
            }
            tl::enums::Update::EditChannelMessage(update) => {
                if let tl::enums::Message::Message(message) = &update.message {
                    result.edited.push(i64::from(message.id));
                }
                record_message(chat_id, update.message, true, &mut changes)?
            }
            tl::enums::Update::DeleteChannelMessages(update) => {
                if update.channel_id != chat_id || update.messages.iter().any(|id| *id <= 0) {
                    return Err(TelegramError::new(TelegramErrorKind::Network));
                }
                for id in update.messages {
                    changes.insert(i64::from(id), None);
                }
            }
            _ => {}
        }
    }
    if changes.len() > 2_000 {
        return Err(TelegramError::new(TelegramErrorKind::LimitExceeded));
    }
    for (id, file) in changes {
        if let Some(file) = file {
            result.files.push(file);
        } else {
            result.removed.push(id);
        }
    }
    Ok(result)
}

fn record_message(
    chat: i64,
    message: tl::enums::Message,
    edited: bool,
    changes: &mut BTreeMap<i64, Option<ChannelFileUpdate>>,
) -> Result<(), TelegramError> {
    if message_channel(&message) != Some(chat) {
        return Err(TelegramError::new(TelegramErrorKind::Network));
    }
    let tl::enums::Message::Message(message) = message else {
        return Ok(());
    };
    let id = i64::from(message.id);
    let file = if let Some(tl::enums::MessageMedia::Document(media)) = message.media {
        let document = Document::from_raw_media(media);
        match document.size() {
            Some(size) => Some(ChannelFileUpdate {
                message_id: id,
                sent_at_unix_ms: i64::from(message.date) * 1000,
                modified_at_unix_ms: i64::from(message.edit_date.unwrap_or(message.date)) * 1000,
                file_name: document.name().unwrap_or_default().to_owned(),
                caption: message.message,
                mime_type: document.mime_type().map(ToOwned::to_owned),
                size_bytes: u64::try_from(size)
                    .map_err(|_| TelegramError::new(TelegramErrorKind::LimitExceeded))?,
            }),
            None => None,
        }
    } else {
        None
    };
    if file.is_some() || edited {
        changes.insert(id, file);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_metadata_wakes_and_old_consumer_cannot_clear_replacement_subscription() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let signals = ChannelUpdateSignals::default();
        let old: ChannelWake = Arc::new(|_, _| panic!("retired callback"));
        signals.set_waker(Some(old.clone()));
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let current: ChannelWake = Arc::new(move |_, _| {
            count.fetch_add(1, Ordering::Relaxed);
        });
        signals.set_waker(Some(current.clone()));
        signals.clear_waker(&old);
        signals.observe(&UpdatesLike::Updates(
            tl::types::UpdateShort {
                update: tl::types::UpdateChannel { channel_id: 2 }.into(),
                date: 1,
            }
            .into(),
        ));
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert!(signals.take().metadata_changed);
        signals.clear_waker(&current);
        signals.observe(&UpdatesLike::ConnectionClosed);
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn push_hints_coalesce_without_losing_an_explicit_gap() {
        let signals = ChannelUpdateSignals::default();
        for pts in [51, 50, 52, 52] {
            signals.observe(&UpdatesLike::Updates(
                tl::types::UpdateShort {
                    update: tl::types::UpdateDeleteChannelMessages {
                        channel_id: 2,
                        messages: vec![77],
                        pts,
                        pts_count: 1,
                    }
                    .into(),
                    date: 1,
                }
                .into(),
            ));
        }
        assert_eq!(signals.take().channels, BTreeMap::from([(2, 52)]));
        let mut hints = ChannelUpdateHints::default();
        ChannelUpdateSignals::channel(&mut hints, 2, 52);
        ChannelUpdateSignals::channel(&mut hints, 2, 0);
        ChannelUpdateSignals::channel(&mut hints, 2, 51);
        assert_eq!(hints.channels[&2], 0);
        signals.observe(&UpdatesLike::ConnectionClosed);
        assert!(signals.take().reconcile_all);
    }

    #[test]
    fn overflowing_hints_request_reconciliation_instead_of_silent_loss() {
        let mut hints = ChannelUpdateHints::default();
        for id in 1..=MAX_DIRTY_CHANNELS as i64 + 20 {
            ChannelUpdateSignals::channel(&mut hints, id, 1);
        }
        assert_eq!(hints.channels.len(), MAX_DIRTY_CHANNELS);
        assert_eq!(hints.overflow_count, 20);
        assert!(hints.reconcile_all);
    }

    #[test]
    fn difference_deletions_are_scoped_and_regressing_cursors_are_rejected() {
        let difference = tl::types::updates::ChannelDifference {
            r#final: true,
            pts: 51,
            timeout: None,
            new_messages: vec![],
            other_updates: vec![
                tl::types::UpdateDeleteChannelMessages {
                    channel_id: 2,
                    messages: vec![77, 78],
                    pts: 51,
                    pts_count: 2,
                }
                .into(),
            ],
            chats: vec![],
            users: vec![],
        };
        assert!(normalize_difference(3, 49, difference.clone().into()).is_err());
        let page = normalize_difference(2, 49, difference.into()).expect("valid difference");
        assert_eq!(page.removed, vec![77, 78]);
        assert_eq!(page.pts, 51);
        assert!(page.complete);
        assert!(
            normalize_difference(
                2,
                51,
                tl::types::updates::ChannelDifferenceEmpty {
                    r#final: true,
                    pts: 50,
                    timeout: None
                }
                .into()
            )
            .is_err()
        );
        assert!(
            normalize_difference(
                2,
                51,
                tl::types::updates::ChannelDifferenceEmpty {
                    r#final: false,
                    pts: 51,
                    timeout: None
                }
                .into()
            )
            .is_err()
        );
    }
    #[test]
    fn push_wakes_after_unlocking_and_retains_payload_without_waiting_for_a_scan() {
        let signals = ChannelUpdateSignals::default();
        let received = Arc::new(Mutex::new(None));
        let output = received.clone();
        let drain = signals.clone();
        signals.set_waker(Some(Arc::new(move |chat, mutation| {
            assert_eq!(chat, Some(2));
            assert!(mutation);
            *output.lock().expect("output") = Some(drain.take()); // would deadlock if the queue lock were retained
        })));
        signals.observe(&UpdatesLike::Updates(
            tl::types::UpdateShort {
                update: tl::types::UpdateDeleteChannelMessages {
                    channel_id: 2,
                    messages: vec![77, 78],
                    pts: 52,
                    pts_count: 2,
                }
                .into(),
                date: 1,
            }
            .into(),
        ));
        signals.set_waker(None);
        let received = received
            .lock()
            .expect("output")
            .take()
            .expect("immediate callback");
        assert_eq!(received.pushes.len(), 1);
        assert_eq!(received.pushes[0].removed, vec![77, 78]);
        assert_eq!(
            (received.pushes[0].pts, received.pushes[0].pts_count),
            (52, 2)
        );
    }

    #[test]
    fn pending_push_flood_is_bounded_and_preserves_gap_recovery() {
        let signals = ChannelUpdateSignals::default();
        for pts in 1..=700 {
            signals.observe(&UpdatesLike::Updates(
                tl::types::UpdateShort {
                    update: tl::types::UpdateDeleteChannelMessages {
                        channel_id: 2,
                        messages: vec![pts],
                        pts,
                        pts_count: 1,
                    }
                    .into(),
                    date: 1,
                }
                .into(),
            ));
        }
        let pending = signals.take();
        assert_eq!(pending.pushes.len(), MAX_PENDING_PUSHES);
        assert!(pending.pending_bytes <= MAX_PENDING_BYTES);
        assert_eq!(pending.channels[&2], 0);
        assert_eq!(pending.overflow_count, 700 - MAX_PENDING_PUSHES as u64);
    }

    #[test]
    fn final_difference_preserves_server_subscription_deadlines() {
        for (timeout, expected) in [(Some(30), Some(30)), (Some(0), Some(1)), (None, None)] {
            let page = normalize_difference(
                2,
                50,
                tl::types::updates::ChannelDifferenceEmpty {
                    r#final: true,
                    pts: 50,
                    timeout,
                }
                .into(),
            )
            .expect("empty difference");
            assert_eq!(page.timeout_seconds, expected);
        }
    }
}
