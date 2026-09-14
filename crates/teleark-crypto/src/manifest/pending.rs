//! Independent v1 pending-upload envelopes. They never authorize file completion.
use super::*;
use crate::{OsRandom, RandomSource};

const MAGIC: &[u8; 8] = b"TARKUPL\0";
const PREFIX: usize = 68;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PendingUploadScope {
    pub account_id: i64,
    pub chat_id: i64,
}

pub struct PendingUpload {
    pub scope: PendingUploadScope,
    pub public_header: ManifestPublicHeader,
    pub metadata: ManifestMetadata,
    pub source_hash_known: bool,
    pub file_key: FileKey,
}

fn validate(
    public: &ManifestPublicHeader,
    metadata: &ManifestMetadata,
    limits: ManifestLimits,
    known: bool,
) -> Result<(), CryptoError> {
    public.validate(limits)?;
    if public.logical_file_size == 0
        || metadata.parts.len() > public.part_count as usize
        || (!known && (!metadata.parts.is_empty() || metadata.whole_plaintext_blake3 != [0; 32]))
    {
        return Err(CryptoError::InvalidField {
            field: "pending upload progress",
        });
    }
    let mut prefix = public.clone();
    prefix.part_count = metadata.parts.len() as u32;
    prefix.logical_file_size = metadata
        .parts
        .iter()
        .try_fold(0u64, |n, p| n.checked_add(p.plaintext_length))
        .ok_or(CryptoError::ArithmeticOverflow {
            field: "pending upload range",
        })?;
    if prefix.logical_file_size > public.logical_file_size {
        return Err(CryptoError::InvalidField {
            field: "pending upload range",
        });
    }
    metadata.validate(&prefix, limits)
}

/// Each immutable update uses a fresh 256-bit salt and a separate HKDF domain.
/// Content keys, completed manifests, and different writers never share this key.
pub fn seal_pending_upload(
    public: &ManifestPublicHeader,
    metadata: &ManifestMetadata,
    known: bool,
    scope: PendingUploadScope,
    key: &FileKey,
    limits: ManifestLimits,
) -> Result<Vec<u8>, CryptoError> {
    validate(public, metadata, limits, known)?;
    if scope.account_id <= 0 || scope.chat_id <= 0 {
        return Err(CryptoError::InvalidField {
            field: "pending upload scope",
        });
    }
    let header = encode_public_header(public)?;
    let mut payload = Zeroizing::new(encode_metadata(metadata)?);
    if header.len() > limits.max_public_header_bytes
        || payload.len() > limits.max_encrypted_metadata_bytes
    {
        return Err(CryptoError::InvalidField {
            field: "pending upload size",
        });
    }
    let mut salt = [0; 32];
    OsRandom.fill_bytes(&mut salt)?;
    let mut aad = MAGIC.to_vec();
    aad.extend_from_slice(&1u16.to_be_bytes());
    aad.extend_from_slice(&u16::from(known).to_be_bytes());
    aad.extend_from_slice(
        &u32::try_from(header.len())
            .map_err(|_| CryptoError::ArithmeticOverflow {
                field: "pending header length",
            })?
            .to_be_bytes(),
    );
    aad.extend_from_slice(
        &u32::try_from(payload.len())
            .map_err(|_| CryptoError::ArithmeticOverflow {
                field: "pending payload length",
            })?
            .to_be_bytes(),
    );
    aad.extend_from_slice(&scope.account_id.to_be_bytes());
    aad.extend_from_slice(&scope.chat_id.to_be_bytes());
    aad.extend_from_slice(&salt);
    aad.extend_from_slice(&header);
    let derived = crate::kdf::pending_upload_key(key.as_bytes(), &public.package_id, &salt)?;
    let tag = encrypt_detached(derived.as_ref(), &ZERO_NONCE, &aad, payload.as_mut())?;
    aad.extend_from_slice(&payload);
    aad.extend_from_slice(&tag);
    Ok(aad)
}

pub fn open_pending_upload(
    bytes: &[u8],
    master: &VaultMasterKey,
    limits: ManifestLimits,
) -> Result<PendingUpload, CryptoError> {
    if bytes.len() < PREFIX + 16 || bytes.get(..8) != Some(MAGIC) {
        return Err(CryptoError::InvalidMagic {
            format: FormatKind::Manifest,
        });
    }
    let version = u16::from_be_bytes(array_at(bytes, 8, "pending version")?);
    if version != 1 {
        return Err(CryptoError::UnsupportedVersion {
            format: FormatKind::Manifest,
            major: version,
            minor: 0,
        });
    }
    let flags = u16::from_be_bytes(array_at(bytes, 10, "pending flags")?);
    if flags > 1 {
        return Err(CryptoError::UnknownFlags {
            field: "pending upload",
            flags: flags.into(),
        });
    }
    let header_len = u32::from_be_bytes(array_at(bytes, 12, "pending header")?) as usize;
    let payload_len = u32::from_be_bytes(array_at(bytes, 16, "pending payload")?) as usize;
    if header_len > limits.max_public_header_bytes
        || payload_len > limits.max_encrypted_metadata_bytes
        || PREFIX
            .checked_add(header_len)
            .and_then(|n| n.checked_add(payload_len))
            .and_then(|n| n.checked_add(16))
            != Some(bytes.len())
    {
        return Err(CryptoError::InvalidField {
            field: "pending envelope length",
        });
    }
    let scope = PendingUploadScope {
        account_id: i64::from_be_bytes(array_at(bytes, 20, "pending account")?),
        chat_id: i64::from_be_bytes(array_at(bytes, 28, "pending chat")?),
    };
    if scope.account_id <= 0 || scope.chat_id <= 0 {
        return Err(CryptoError::InvalidField {
            field: "pending upload scope",
        });
    }
    let end = PREFIX + header_len;
    let public = decode_public_header(&bytes[PREFIX..end], limits)?;
    public.validate(limits)?;
    let key = unwrap_file_key(
        master,
        &public.file_key_wrap,
        &public.vault_id,
        &public.package_id,
        public.master_key_generation,
    )?;
    let derived = crate::kdf::pending_upload_key(
        key.as_bytes(),
        &public.package_id,
        &array_at(bytes, 36, "pending salt")?,
    )?;
    let mut payload = Zeroizing::new(bytes[end..end + payload_len].to_vec());
    decrypt_detached(
        derived.as_ref(),
        &ZERO_NONCE,
        &bytes[..end],
        payload.as_mut(),
        &array_at(bytes, end + payload_len, "pending tag")?,
    )?;
    let metadata = decode_metadata(&payload, limits)?;
    validate(&public, &metadata, limits, flags == 1)?;
    Ok(PendingUpload {
        scope,
        public_header: public,
        metadata,
        source_hash_known: flags == 1,
        file_key: key,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    const SCOPE: PendingUploadScope = PendingUploadScope {
        account_id: 7,
        chat_id: 11,
    };
    #[test]
    fn frozen_pending_v1_preserves_authentication_and_rejects_wrong_keys_and_truncation() {
        let (master, _, public, mut metadata) = crate::manifest::tests::sample();
        metadata.parts.clear();
        metadata.whole_plaintext_blake3 = [0; 32];
        let bytes = include_bytes!("fixtures/pending-v1.bin");
        let opened =
            open_pending_upload(bytes, &master, ManifestLimits::default()).expect("frozen v1");
        assert_eq!(opened.public_header, public);
        assert!(opened.metadata == metadata);
        assert_eq!(opened.scope, SCOPE);
        assert!(!opened.source_hash_known);
        assert!(
            open_pending_upload(
                bytes,
                &VaultMasterKey::from_bytes([0; 32]),
                ManifestLimits::default()
            )
            .is_err()
        );
        for end in 0..bytes.len() {
            assert!(
                open_pending_upload(&bytes[..end], &master, ManifestLimits::default()).is_err()
            );
        }
    }
    #[test]
    fn pending_state_authenticates_progress_and_cannot_be_opened_as_a_completed_file() {
        let (master, key, public, mut metadata) = crate::manifest::tests::sample();
        metadata.parts.clear();
        metadata.whole_plaintext_blake3 = [0; 32];
        let first = seal_pending_upload(
            &public,
            &metadata,
            false,
            SCOPE,
            &key,
            ManifestLimits::default(),
        )
        .expect("announce");
        let opened = open_pending_upload(&first, &master, ManifestLimits::default())
            .expect("read from another device");
        assert!(!opened.source_hash_known);
        assert!(opened.metadata.parts.is_empty());
        assert!(open_manifest(&first, &master, ManifestLimits::default()).is_err());
        let second = seal_pending_upload(
            &public,
            &metadata,
            false,
            SCOPE,
            &key,
            ManifestLimits::default(),
        )
        .expect("new immutable attempt");
        assert_ne!(&first[36..68], &second[36..68]);
        for index in [10, 20, 28, 36, 68, first.len() - 1] {
            let mut corrupt = first.clone();
            corrupt[index] ^= 1;
            assert!(open_pending_upload(&corrupt, &master, ManifestLimits::default()).is_err());
        }
        let (_, _, _, mut metadata) = crate::manifest::tests::sample();
        metadata.parts.truncate(1);
        let bytes = seal_pending_upload(
            &public,
            &metadata,
            true,
            SCOPE,
            &key,
            ManifestLimits::default(),
        )
        .expect("partial receipt");
        let opened = open_pending_upload(&bytes, &master, ManifestLimits::default())
            .expect("authenticated prefix");
        assert_eq!(opened.metadata.parts.len(), 1);
        assert_eq!(opened.public_header.part_count, 2);
        assert!(
            seal_pending_upload(
                &public,
                &metadata,
                false,
                SCOPE,
                &key,
                ManifestLimits::default()
            )
            .is_err()
        );
        let mut newer = bytes;
        newer[9] = 2;
        assert!(matches!(
            open_pending_upload(&newer, &master, ManifestLimits::default()),
            Err(CryptoError::UnsupportedVersion { .. })
        ));
    }
}
