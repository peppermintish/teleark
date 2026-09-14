//! Stable publication requests; no durable retry may allocate a fresh ID.
use grammers_client::tl;

/// A reserved ID is bound to one immutable object before transport starts.
#[derive(Default)]
pub struct UploadPublicationOptions<'a> {
    pub observer: Option<&'a dyn crate::ByteTransferObserver>,
    pub random_id: Option<i64>,
}

pub(crate) fn document_request(
    peer: tl::enums::InputPeer,
    file: tl::enums::InputFile,
    name: &str,
    caption: &str,
    random_id: i64,
) -> tl::functions::messages::SendMedia {
    tl::functions::messages::SendMedia {
        silent: false,
        background: false,
        clear_draft: false,
        peer,
        reply_to: None,
        media: tl::types::InputMediaUploadedDocument {
            nosound_video: false,
            force_file: true,
            spoiler: false,
            file,
            thumb: None,
            mime_type: "application/octet-stream".into(),
            attributes: vec![
                tl::types::DocumentAttributeFilename {
                    file_name: name.into(),
                }
                .into(),
            ],
            stickers: None,
            ttl_seconds: None,
            video_cover: None,
            video_timestamp: None,
        }
        .into(),
        message: caption.into(),
        random_id,
        reply_markup: None,
        entities: None,
        schedule_date: None,
        schedule_repeat_period: None,
        send_as: None,
        noforwards: false,
        update_stickersets_order: false,
        invert_media: false,
        quick_reply_shortcut: None,
        effect: None,
        allow_paid_floodskip: false,
        allow_paid_stars: None,
        suggested_post: None,
    }
}

pub(crate) fn sent_message_id(updates: tl::enums::Updates, random_id: i64) -> Option<i32> {
    let updates = match updates {
        tl::enums::Updates::UpdateShortSentMessage(value) => return Some(value.id),
        tl::enums::Updates::Updates(value) => value.updates,
        tl::enums::Updates::Combined(value) => value.updates,
        _ => return None,
    };
    updates.into_iter().find_map(|update| match update {
        tl::enums::Update::MessageId(value) if value.random_id == random_id => Some(value.id),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt(entries: &[(i64, i32)]) -> tl::enums::Updates {
        tl::types::Updates {
            updates: entries
                .iter()
                .map(|&(random_id, id)| tl::types::UpdateMessageId { random_id, id }.into())
                .collect(),
            users: vec![],
            chats: vec![],
            date: 0,
            seq: 0,
        }
        .into()
    }

    #[test]
    fn duplicate_publication_response_keeps_the_result_ambiguous() {
        // https://core.telegram.org/method/messages.sendMedia documents
        // RANDOM_ID_DUPLICATE as a 500 error, without a message receipt.
        let error = crate::map_invocation(grammers_mtsender::InvocationError::Rpc(
            grammers_mtsender::RpcError {
                code: 500,
                name: "RANDOM_ID_DUPLICATE".into(),
                value: None,
                caused_by: None,
            },
        ));
        assert_eq!(error.kind(), crate::TelegramErrorKind::Server);
    }

    #[test]
    fn repeated_uploads_keep_the_reserved_message_identity() {
        let request = |temporary_file_id| {
            document_request(
                tl::enums::InputPeer::PeerSelf,
                tl::types::InputFileBig {
                    id: temporary_file_id,
                    parts: 1,
                    name: "part.bin".into(),
                }
                .into(),
                "part.bin",
                "opaque-part-identity",
                -712,
            )
        };
        let first = request(10);
        let retry = request(20);
        assert_eq!(first.random_id, retry.random_id);
        assert_eq!(retry.random_id, -712);
        assert_eq!(first.message, retry.message);
        for sent in [first, retry] {
            let tl::enums::InputMedia::UploadedDocument(document) = sent.media else {
                panic!("document required")
            };
            assert!(document.force_file);
            assert_eq!(document.ttl_seconds, None);
            assert_eq!(document.mime_type, "application/octet-stream");
        }
    }

    #[test]
    fn publication_receipts_are_correlated_to_reserved_identity() {
        assert_eq!(
            sent_message_id(receipt(&[(17, 90), (23, 91)]), 23),
            Some(91)
        );
        assert_eq!(sent_message_id(receipt(&[(17, 90)]), 23), None);
        assert_eq!(sent_message_id(receipt(&[]), 23), None);
    }
}
