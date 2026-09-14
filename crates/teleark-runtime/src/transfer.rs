mod manifest_outbox;
mod reserved;
pub use reserved::ReservedPublicationStore;

use std::{
    collections::{BTreeMap, BTreeSet},
    io::Cursor,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use teleark_core::{
    AccountId, ApplicationError, ApplicationErrorKind, PackageId, PartIndex, TransferError,
    TransferId, TransferTask,
};
use teleark_crypto::{
    AeadUsageRegistry, CRYPTO_SUITE_ID, CryptoError, FileKey, ManifestLimits, ManifestMetadata,
    ManifestPart, ManifestPublicHeader, MediaKind, NONCE_STRATEGY_ID, OpenedManifest, OsRandom,
    PartHeader, PartInstanceRegistry, PartLimits, RemoteLocator, VaultMasterKey,
    decrypt_part_cancellable, encrypt_part_cancellable, open_manifest, remote_manifest_name,
    remote_part_name, seal_manifest, wrap_file_key,
};
use teleark_telegram::MAX_TRANSFER_OBJECT_BYTES;
use teleark_transfer::{
    Blake3Digest, CheckpointPort, ContentDigest, DestinationId, DigestPort, FileSystemPort,
    NativeFileSystem, PartCheckpoint, RemoteObject, RemotePartKey, RemoteTransport, SourceId,
    SourceIdentity, SourcePort, TransferCheckpoint, UploadError, VerifiedPart,
};

use teleark_storage::{Database, StoredPartState, StoredTransferState, TransferTaskRecord};

use crate::DesktopTelegram;

const FRAME_PLAINTEXT_BYTES: u32 = 8 * 1024 * 1024;
const ENCRYPTED_PART_PLAINTEXT_BYTES: u64 = 60 * 1024 * 1024;
const MAX_RECONCILIATION_RESULTS: usize = 1_000;
pub(crate) const MANIFEST_CAPTION: &str = "teleark-manifest-v1";
const CHECKPOINT_VERSION: u32 = 1;
const CHECKPOINT_MAGIC: &[u8; 8] = b"TARKCP01";

/// Current conservative plaintext ceiling used by the connected encrypted
/// Saved Messages workflow. This is deliberately exposed for frontend-neutral
/// presentation so the GUI never advertises a size the runtime will ignore.
pub const fn encrypted_part_plaintext_limit() -> u64 {
    ENCRYPTED_PART_PLAINTEXT_BYTES
}

/// Splits a non-empty logical file into bounded plaintext ranges whose encoded
/// containers fit the Telegram object's in-memory safety cap.
pub fn encrypted_part_sizes(total_bytes: u64) -> Result<Vec<u64>, TransferError> {
    if total_bytes == 0 {
        return Err(TransferError::SourceMissing);
    }
    let count = total_bytes.div_ceil(ENCRYPTED_PART_PLAINTEXT_BYTES);
    let capacity = usize::try_from(count).map_err(|_| TransferError::SourceChanged)?;
    let mut parts = Vec::with_capacity(capacity);
    let mut remaining = total_bytes;
    while remaining != 0 {
        let size = remaining.min(ENCRYPTED_PART_PLAINTEXT_BYTES);
        parts.push(size);
        remaining -= size;
    }
    Ok(parts)
}

/// Opaque remote byte-object metadata used below the transfer engine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteByteObject {
    pub object_id: u64,
    pub name: String,
    pub encoded_size: u64,
}

/// User-owned metadata required to publish one authenticated recovery
/// manifest after every encrypted part has a verified remote locator.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManifestPublishRequest {
    pub vault_id: [u8; 16],
    pub manifest_generation: u32,
    pub master_key_generation: u32,
    pub wrap_generation: u32,
    pub created_at_unix_ms: u64,
    pub logical_name: String,
    pub relative_path: Option<String>,
    pub mime_type: Option<String>,
    pub media_kind: MediaKind,
    pub whole_plaintext_blake3: [u8; 32],
}

/// One authenticated remote manifest and the object that carried it.
pub struct RecoveredManifest {
    pub object: RemoteByteObject,
    pub manifest: OpenedManifest,
}

/// One candidate rejected during bounded recovery scanning.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RejectedManifest {
    pub object: RemoteByteObject,
    pub error: TransferError,
}

/// Recovery scanning keeps valid manifests available even when unrelated or
/// damaged candidates share the common discovery caption.
#[derive(Default)]
pub struct ManifestRecoveryReport {
    pub recovered: Vec<RecoveredManifest>,
    pub rejected: Vec<RejectedManifest>,
}

/// Narrow byte-store boundary implemented by Telegram and deterministic tests.
pub trait RemoteObjectStore {
    fn search_exact_caption(
        &mut self,
        caption: &str,
        limit: usize,
    ) -> Result<Vec<RemoteByteObject>, TransferError>;

    fn upload(
        &mut self,
        name: &str,
        caption: &str,
        bytes: Vec<u8>,
    ) -> Result<RemoteByteObject, UploadError>;

    fn download(&mut self, object_id: u64) -> Result<Vec<u8>, TransferError>;
}

/// Real Telegram implementation of the byte-store boundary.
pub struct TelegramObjectStore {
    observer: Option<Arc<dyn teleark_telegram::ByteTransferObserver>>,
    cancellation: Option<crate::TelegramScanCancellation>,
    account_id: i64,
    telegram: DesktopTelegram,
    chat_id: i64,
    manifest_catalog: Option<Vec<RemoteByteObject>>,
}

impl TelegramObjectStore {
    #[must_use]
    pub const fn new(telegram: DesktopTelegram, account_id: i64, chat_id: i64) -> Self {
        Self {
            telegram,
            account_id,
            chat_id,
            cancellation: None,
            observer: None,
            manifest_catalog: None,
        }
    }
}

impl TelegramObjectStore {
    pub(crate) fn with_manifest_catalog(mut self, catalog: Vec<RemoteByteObject>) -> Self {
        self.manifest_catalog = Some(catalog);
        self
    }

    pub(crate) fn with_observer(
        mut self,
        observer: Arc<dyn teleark_telegram::ByteTransferObserver>,
    ) -> Self {
        self.observer = Some(observer);
        self
    }

    pub fn with_cancellation(mut self, cancellation: crate::TelegramScanCancellation) -> Self {
        self.cancellation = Some(cancellation);
        self
    }
}

impl TelegramObjectStore {
    /// Publishes an immutable object using its durably reserved identity.
    /// An ambiguous result must be reconciled before completing the local part.
    pub fn upload_reserved(
        &mut self,
        name: &str,
        caption: &str,
        bytes: Vec<u8>,
        random_id: std::num::NonZeroI64,
    ) -> Result<RemoteByteObject, UploadError> {
        self.upload_publication(name, caption, bytes, Some(random_id.get()))
    }

    fn upload_publication(
        &mut self,
        name: &str,
        caption: &str,
        bytes: Vec<u8>,
        publication_random_id: Option<i64>,
    ) -> Result<RemoteByteObject, UploadError> {
        let encoded_size = bytes.len() as u64;
        if let Some(observer) = &self.observer {
            observer.observe(teleark_telegram::ByteTransferEvent::WaitingForUpload);
        }
        match self.telegram.upload_bytes_observed(
            self.account_id,
            self.chat_id,
            name.to_owned(),
            caption.to_owned(),
            bytes,
            crate::telegram::UploadByteOptions {
                publication_random_id,
                observer: self.observer.clone(),
                cancellation: self.cancellation.clone(),
            },
        ) {
            Ok(message_id) => Ok(RemoteByteObject {
                object_id: u64::try_from(message_id)
                    .map_err(|_| UploadError::Definite(TransferError::RemoteMissing))?,
                name: name.to_owned(),
                encoded_size,
            }),
            Err(error)
                if matches!(
                    error.kind(),
                    ApplicationErrorKind::Network
                        | ApplicationErrorKind::Server
                        | ApplicationErrorKind::Cancelled
                ) =>
            {
                Err(UploadError::AmbiguousSuccess)
            }
            Err(error) => Err(UploadError::Definite(map_application_error(error))),
        }
    }
}

impl RemoteObjectStore for TelegramObjectStore {
    fn search_exact_caption(
        &mut self,
        caption: &str,
        limit: usize,
    ) -> Result<Vec<RemoteByteObject>, TransferError> {
        if caption == MANIFEST_CAPTION
            && let Some(catalog) = &self.manifest_catalog
        {
            return Ok(catalog.iter().take(limit).cloned().collect());
        }
        self.telegram
            .search_files_exact_caption(
                self.account_id,
                self.chat_id,
                caption,
                limit,
                self.cancellation.clone(),
            )
            .map_err(map_application_error)
            .and_then(|files| {
                files
                    .into_iter()
                    .map(|file| {
                        Ok(RemoteByteObject {
                            object_id: u64::try_from(file.message_id)
                                .map_err(|_| TransferError::RemoteMissing)?,
                            name: file.file_name,
                            encoded_size: file.size_bytes,
                        })
                    })
                    .collect()
            })
    }

    fn upload(
        &mut self,
        name: &str,
        caption: &str,
        bytes: Vec<u8>,
    ) -> Result<RemoteByteObject, UploadError> {
        self.upload_publication(name, caption, bytes, None)
    }

    fn download(&mut self, object_id: u64) -> Result<Vec<u8>, TransferError> {
        let message_id = i64::try_from(object_id).map_err(|_| TransferError::RemoteMissing)?;
        if let Some(observer) = &self.observer {
            observer
                .observe(teleark_telegram::ByteTransferEvent::Downloading { bytes: 0, total: 0 });
        }
        self.telegram
            .download_bytes_observed(
                self.account_id,
                self.chat_id,
                message_id,
                self.cancellation.clone(),
                self.observer.clone(),
            )
            .map_err(map_application_error)
    }
}

/// Encrypting/decrypting Telegram transport plugged directly into the
/// deterministic transfer engine. One instance owns one package and File Key.
pub struct EncryptedRemoteTransport<S> {
    store: S,
    account_id: AccountId,
    chat_id: i64,
    package_id: PackageId,
    package_bytes: [u8; 16],
    file_key: Arc<FileKey>,
    logical_file_size: u64,
    part_sizes: Vec<u64>,
    limits: PartLimits,
    usage: AeadUsageRegistry,
    instances: PartInstanceRegistry,
    hydrated_objects: BTreeSet<u64>,
    manifest_parts: BTreeMap<u32, ManifestPart>,
    recovered_public: Option<ManifestPublicHeader>,
    recovered_whole_digest: Option<[u8; 32]>,
    manifest_publication: Option<ManifestPublication>,
    published_manifest_envelope: Option<Vec<u8>>,
    cancellation: Option<crate::TelegramScanCancellation>,
}

struct ManifestPublication {
    master_key: VaultMasterKey,
    request: ManifestPublishRequest,
}

#[derive(Clone)]
pub(crate) struct PartEncryptionContext {
    file_key: Arc<FileKey>,
    limits: PartLimits,
    cancellation: Option<crate::TelegramScanCancellation>,
}

#[derive(Clone, Debug)]
pub(crate) struct PartEncryptionPlan {
    key: RemotePartKey,
    chat_id: i64,
    header: PartHeader,
    expected_digest: Option<ContentDigest>,
    expected_encoded_size: u64,
    remote_name: String,
}

pub(crate) struct PreparedEncryptedPart {
    pub(crate) key: RemotePartKey,
    pub(crate) encoded: Vec<u8>,
    pub(crate) manifest_part: ManifestPart,
    pub(crate) plaintext_digest: ContentDigest,
    pub(crate) encryption_duration_micros: u64,
}

impl<S> EncryptedRemoteTransport<S> {
    pub fn new(
        store: S,
        account_id: AccountId,
        chat_id: i64,
        package_id: PackageId,
        file_key: FileKey,
        logical_file_size: u64,
        part_sizes: Vec<u64>,
    ) -> Result<Self, TransferError> {
        validate_part_sizes(logical_file_size, &part_sizes)?;
        Ok(Self {
            store,
            account_id,
            chat_id,
            package_id,
            package_bytes: package_bytes(package_id),
            file_key: Arc::new(file_key),
            logical_file_size,
            part_sizes,
            limits: PartLimits::default(),
            usage: AeadUsageRegistry::new(),
            instances: PartInstanceRegistry::default(),
            hydrated_objects: BTreeSet::new(),
            manifest_parts: BTreeMap::new(),
            recovered_public: None,
            recovered_whole_digest: None,
            manifest_publication: None,
            published_manifest_envelope: None,
            cancellation: None,
        })
    }

    pub(crate) fn with_cancellation(
        mut self,
        cancellation: crate::TelegramScanCancellation,
    ) -> Self {
        self.cancellation = Some(cancellation);
        self
    }

    /// The already re-read/authenticated envelope can be persisted locally
    /// without another network request or retaining decrypted metadata.
    pub(crate) fn take_published_manifest_envelope(&mut self) -> Option<Vec<u8>> {
        self.published_manifest_envelope.take()
    }

    /// Require the engine's upload-finalization step to publish and verify a
    /// recovery manifest before the Core task can become `Completed`.
    pub fn configure_manifest_publication(
        &mut self,
        master_key: VaultMasterKey,
        request: ManifestPublishRequest,
    ) {
        self.manifest_publication = Some(ManifestPublication {
            master_key,
            request,
        });
    }

    /// Rebuild a package transport solely from one authenticated remote
    /// manifest. No SQLite row, caller-supplied File Key, or part layout is
    /// needed.
    pub fn from_opened_manifest(store: S, opened: OpenedManifest) -> Result<Self, TransferError> {
        let (public, metadata, file_key) = opened.into_parts();
        let package_id = package_id_from_bytes(public.package_id)?;
        let first = metadata
            .parts
            .first()
            .ok_or(TransferError::ManifestCorrupted)?;
        let account_id = AccountId::new(first.remote_locator.account_id);
        let chat_id = first.remote_locator.chat_id;
        if metadata.parts.iter().any(|part| {
            part.remote_locator.account_id != account_id.get()
                || part.remote_locator.chat_id != chat_id
        }) {
            return Err(TransferError::ManifestCorrupted);
        }
        let part_sizes = metadata
            .parts
            .iter()
            .map(|part| part.plaintext_length)
            .collect::<Vec<_>>();
        let mut transport = Self::new(
            store,
            account_id,
            chat_id,
            package_id,
            file_key,
            public.logical_file_size,
            part_sizes,
        )?;
        transport.manifest_parts = metadata
            .parts
            .iter()
            .cloned()
            .map(|part| (part.part_index, part))
            .collect();
        transport.recovered_whole_digest = Some(metadata.whole_plaintext_blake3);
        transport.recovered_public = Some(public);
        Ok(transport)
    }

    #[must_use]
    pub fn into_store(self) -> S {
        self.store
    }

    fn validate_key(&self, key: RemotePartKey) -> Result<usize, TransferError> {
        if key.account_id != self.account_id || key.package_id != self.package_id {
            return Err(TransferError::RemoteMissing);
        }
        let position = key.part_index.get() as usize;
        self.part_sizes
            .get(position)
            .map(|_| position)
            .ok_or(TransferError::RemoteMissing)
    }

    fn plaintext_offset(&self, position: usize) -> Result<u64, TransferError> {
        self.part_sizes[..position]
            .iter()
            .try_fold(0_u64, |total, size| total.checked_add(*size))
            .ok_or(TransferError::ManifestCorrupted)
    }

    fn caption(&self, key: RemotePartKey) -> String {
        format!(
            "teleark-object-v1-{}-{:08x}",
            hex_id(&self.package_bytes),
            key.part_index.get()
        )
    }

    fn name(&self, key: RemotePartKey) -> String {
        remote_part_name(&self.package_bytes, key.part_index.get())
    }

    pub(crate) fn encryption_context(&self) -> PartEncryptionContext {
        PartEncryptionContext {
            file_key: Arc::clone(&self.file_key),
            limits: self.limits,
            cancellation: self.cancellation.clone(),
        }
    }

    pub(crate) fn plan_part_encryption(
        &mut self,
        key: RemotePartKey,
        expected_digest: Option<ContentDigest>,
    ) -> Result<PartEncryptionPlan, TransferError> {
        let position = self.validate_key(key)?;
        let expected_size = self.part_sizes[position];
        let instance_id = self
            .instances
            .generate(&mut OsRandom)
            .map_err(map_crypto_error)?;
        self.usage
            .reserve_existing_part(
                &self.file_key,
                &self.package_bytes,
                key.part_index.get(),
                instance_id,
            )
            .map_err(map_crypto_error)?;
        let header = PartHeader::new(
            self.package_bytes,
            instance_id,
            key.part_index.get(),
            u32::try_from(self.part_sizes.len()).map_err(|_| TransferError::ManifestCorrupted)?,
            self.logical_file_size,
            self.plaintext_offset(position)?,
            expected_size,
            FRAME_PLAINTEXT_BYTES,
            self.limits,
        )
        .map_err(map_crypto_error)?;
        let expected_encoded_size = header.expected_encoded_length().map_err(map_crypto_error)?;
        if expected_encoded_size > MAX_TRANSFER_OBJECT_BYTES as u64 {
            return Err(TransferError::ManifestCorrupted);
        }
        Ok(PartEncryptionPlan {
            key,
            chat_id: self.chat_id,
            header,
            expected_digest,
            expected_encoded_size,
            remote_name: self.name(key),
        })
    }

    pub(crate) fn encrypt_planned_part(
        context: &PartEncryptionContext,
        plan: PartEncryptionPlan,
        plaintext: Vec<u8>,
    ) -> Result<PreparedEncryptedPart, TransferError> {
        if context
            .cancellation
            .as_ref()
            .is_some_and(crate::TelegramScanCancellation::is_cancelled)
        {
            return Err(TransferError::Cancelled);
        }
        let plaintext_digest = Blake3Digest.digest(&plaintext);
        if plaintext.len() as u64 != plan.header.plaintext_length
            || plan
                .expected_digest
                .is_some_and(|expected| expected != plaintext_digest)
        {
            return Err(TransferError::SourceChanged);
        }
        let capacity = usize::try_from(plan.expected_encoded_size)
            .map_err(|_| TransferError::ManifestCorrupted)?;
        let mut encoded = Vec::with_capacity(capacity);
        let mut worker_usage = AeadUsageRegistry::new();
        let encryption_started = std::time::Instant::now();
        let summary = encrypt_part_cancellable(
            &mut Cursor::new(plaintext),
            &mut encoded,
            &plan.header,
            &context.file_key,
            context.limits,
            &mut worker_usage,
            || {
                context
                    .cancellation
                    .as_ref()
                    .is_some_and(crate::TelegramScanCancellation::is_cancelled)
            },
        )
        .map_err(map_crypto_error)?;
        let encryption_duration_micros =
            u64::try_from(encryption_started.elapsed().as_micros()).unwrap_or(u64::MAX);
        if summary.encoded_length != plan.expected_encoded_size || encoded.len() != capacity {
            return Err(TransferError::ManifestCorrupted);
        }
        Ok(PreparedEncryptedPart {
            key: plan.key,
            encoded,
            plaintext_digest,
            encryption_duration_micros,
            manifest_part: ManifestPart {
                part_index: plan.key.part_index.get(),
                part_instance_id: summary.header.part_instance_id.0,
                plaintext_offset: summary.header.plaintext_offset,
                plaintext_length: summary.header.plaintext_length,
                encoded_length: summary.encoded_length,
                frame_count: summary.header.frame_count,
                plaintext_blake3: summary.plaintext_blake3,
                encoded_ciphertext_blake3: summary.encoded_blake3,
                remote_locator: RemoteLocator {
                    account_id: plan.key.account_id.get(),
                    chat_id: plan.chat_id,
                    message_id: 0,
                    remote_name: plan.remote_name,
                    locator_version: 1,
                    locator_extension: None,
                },
            },
        })
    }

    fn encrypt(
        &mut self,
        key: RemotePartKey,
        plaintext: &[u8],
        expected_digest: ContentDigest,
    ) -> Result<(Vec<u8>, ManifestPart), TransferError> {
        let plan = self.plan_part_encryption(key, Some(expected_digest))?;
        let context = self.encryption_context();
        let prepared = Self::encrypt_planned_part(&context, plan, plaintext.to_vec())?;
        Ok((prepared.encoded, prepared.manifest_part))
    }

    #[cfg(test)]
    pub(crate) fn upload_prepared_part(
        &mut self,
        mut prepared: PreparedEncryptedPart,
    ) -> Result<RemoteObject, TransferError>
    where
        S: RemoteObjectStore,
    {
        let caption = self.caption(prepared.key);
        let object = self
            .store
            .upload(
                &prepared.manifest_part.remote_locator.remote_name,
                &caption,
                std::mem::take(&mut prepared.encoded),
            )
            .map_err(|error| match error {
                UploadError::Definite(error) => error,
                UploadError::AmbiguousSuccess => TransferError::Network,
            })?;
        self.finish_prepared_part(prepared, object)
    }

    fn finish_prepared_part(
        &mut self,
        prepared: PreparedEncryptedPart,
        object: RemoteByteObject,
    ) -> Result<RemoteObject, TransferError>
    where
        S: RemoteObjectStore,
    {
        if object.object_id == 0
            || object.name != prepared.manifest_part.remote_locator.remote_name
            || object.encoded_size != prepared.manifest_part.encoded_length
        {
            return Err(TransferError::HashMismatch);
        }
        let expected_plaintext_size = prepared.manifest_part.plaintext_length;
        let encoded = self.store.download(object.object_id)?;
        if encoded.len() as u64 != prepared.manifest_part.encoded_length
            || blake3::hash(&encoded).as_bytes()
                != &prepared.manifest_part.encoded_ciphertext_blake3
        {
            return Err(TransferError::HashMismatch);
        }
        self.hydrated_objects.insert(object.object_id);
        let (_, remote_object) = self.decode(prepared.key, &object, &encoded)?;
        if remote_object.plaintext_size != expected_plaintext_size
            || remote_object.digest != prepared.plaintext_digest
        {
            return Err(TransferError::HashMismatch);
        }
        Ok(remote_object)
    }

    fn decode(
        &mut self,
        key: RemotePartKey,
        object: &RemoteByteObject,
        encoded: &[u8],
    ) -> Result<(Vec<u8>, RemoteObject), TransferError> {
        let position = self.validate_key(key)?;
        if object.name != self.name(key)
            || object.encoded_size != encoded.len() as u64
            || encoded.len() > MAX_TRANSFER_OBJECT_BYTES
        {
            return Err(TransferError::HashMismatch);
        }
        let mut plaintext = Vec::with_capacity(self.part_sizes[position] as usize);
        let summary = decrypt_part_cancellable(
            &mut Cursor::new(encoded),
            &mut plaintext,
            &self.file_key,
            self.limits,
            || {
                self.cancellation
                    .as_ref()
                    .is_some_and(crate::TelegramScanCancellation::is_cancelled)
            },
        )
        .map_err(map_crypto_error)?;
        let expected_offset = self.plaintext_offset(position)?;
        if summary.header.package_id != self.package_bytes
            || summary.header.part_index != key.part_index.get()
            || summary.header.part_count != self.part_sizes.len() as u32
            || summary.header.logical_file_size != self.logical_file_size
            || summary.header.plaintext_offset != expected_offset
            || summary.header.plaintext_length != self.part_sizes[position]
            || summary.encoded_length != object.encoded_size
            || plaintext.len() as u64 != self.part_sizes[position]
        {
            return Err(TransferError::ManifestCorrupted);
        }
        if self.hydrated_objects.insert(object.object_id) {
            self.instances
                .register(summary.header.part_instance_id)
                .map_err(map_crypto_error)?;
            self.usage
                .reserve_existing_part(
                    &self.file_key,
                    &self.package_bytes,
                    key.part_index.get(),
                    summary.header.part_instance_id,
                )
                .map_err(map_crypto_error)?;
        }
        if let Some(public) = &self.recovered_public
            && let Some(manifest_part) = self.manifest_parts.get(&key.part_index.get())
        {
            manifest_part
                .verify_decrypted_part(public, &summary)
                .map_err(map_crypto_error)?;
        }
        let manifest_part = ManifestPart {
            part_index: key.part_index.get(),
            part_instance_id: summary.header.part_instance_id.0,
            plaintext_offset: summary.header.plaintext_offset,
            plaintext_length: summary.header.plaintext_length,
            encoded_length: summary.encoded_length,
            frame_count: summary.header.frame_count,
            plaintext_blake3: summary.plaintext_blake3,
            encoded_ciphertext_blake3: summary.encoded_blake3,
            remote_locator: RemoteLocator {
                account_id: self.account_id.get(),
                chat_id: self.chat_id,
                message_id: i64::try_from(object.object_id)
                    .map_err(|_| TransferError::ManifestCorrupted)?,
                remote_name: object.name.clone(),
                locator_version: 1,
                locator_extension: None,
            },
        };
        self.manifest_parts
            .insert(key.part_index.get(), manifest_part);
        let digest = Blake3Digest.digest(&plaintext);
        Ok((
            plaintext,
            RemoteObject {
                object_id: object.object_id,
                key,
                plaintext_size: self.part_sizes[position],
                encoded_size: object.encoded_size,
                digest,
            },
        ))
    }

    fn fetch_decode(
        &mut self,
        key: RemotePartKey,
        object: &RemoteByteObject,
    ) -> Result<(Vec<u8>, RemoteObject), TransferError>
    where
        S: RemoteObjectStore,
    {
        let encoded = self.store.download(object.object_id)?;
        self.decode(key, object, &encoded)
    }

    /// Download one part named by an authenticated opened manifest. Unlike
    /// discovery, this retains the verified plaintext for the destination owner.
    pub(crate) fn download_manifest_part(
        &mut self,
        key: RemotePartKey,
    ) -> Result<Vec<u8>, TransferError>
    where
        S: RemoteObjectStore,
    {
        self.validate_key(key)?;
        if self.recovered_public.is_none() {
            return Err(TransferError::ManifestCorrupted);
        }
        let part = self
            .manifest_parts
            .get(&key.part_index.get())
            .cloned()
            .ok_or(TransferError::ManifestCorrupted)?;
        let object = RemoteByteObject {
            object_id: u64::try_from(part.remote_locator.message_id)
                .map_err(|_| TransferError::ManifestCorrupted)?,
            name: part.remote_locator.remote_name,
            encoded_size: part.encoded_length,
        };
        let (plaintext, _) = self.fetch_decode(key, &object)?;
        Ok(plaintext)
    }

    /// Publish and re-read the authenticated recovery manifest. The common
    /// caption makes manifests enumerable after local database loss, while the
    /// opaque filename binds each candidate to its authenticated package ID.
    pub fn publish_manifest(
        &mut self,
        master_key: &VaultMasterKey,
        request: ManifestPublishRequest,
    ) -> Result<RemoteByteObject, TransferError>
    where
        S: RemoteObjectStore,
    {
        for position in 0..self.part_sizes.len() {
            let part_index = u32::try_from(position)
                .map(PartIndex::new)
                .map_err(|_| TransferError::ManifestCorrupted)?;
            if !self.manifest_parts.contains_key(&part_index.get()) {
                let discovered = self.discover_remote(RemotePartKey {
                    account_id: self.account_id,
                    package_id: self.package_id,
                    part_index,
                })?;
                if discovered.len() != 1 {
                    return Err(TransferError::ManifestCorrupted);
                }
            }
        }
        if self.manifest_parts.len() != self.part_sizes.len() {
            return Err(TransferError::ManifestCorrupted);
        }
        let parts = (0..self.part_sizes.len())
            .map(|position| {
                let index =
                    u32::try_from(position).map_err(|_| TransferError::ManifestCorrupted)?;
                self.manifest_parts
                    .get(&index)
                    .cloned()
                    .ok_or(TransferError::ManifestCorrupted)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let part_count =
            u32::try_from(parts.len()).map_err(|_| TransferError::ManifestCorrupted)?;
        let application_part_target = *self
            .part_sizes
            .first()
            .ok_or(TransferError::ManifestCorrupted)?;
        let metadata = ManifestMetadata {
            logical_name: request.logical_name,
            relative_path: request.relative_path,
            mime_type: request.mime_type,
            media_kind: request.media_kind,
            whole_plaintext_blake3: request.whole_plaintext_blake3,
            parts,
            logical_timestamps: None,
            source_metadata: Vec::new(),
            format_extensions: Vec::new(),
        };
        let name = remote_manifest_name(&self.package_bytes);
        let existing = self
            .store
            .search_exact_caption(MANIFEST_CAPTION, MAX_RECONCILIATION_RESULTS)?
            .into_iter()
            .filter(|candidate| candidate.name == name)
            .collect::<Vec<_>>();
        if existing.len() > 1 {
            return Err(TransferError::ManifestCorrupted);
        }
        if let Some(object) = existing.into_iter().next() {
            let downloaded = self.store.download(object.object_id)?;
            let opened = open_manifest(&downloaded, master_key, ManifestLimits::default())
                .map_err(map_crypto_error)?;
            let public = &opened.public_header;
            if public.package_id == self.package_bytes
                && public.vault_id == request.vault_id
                && public.manifest_generation == request.manifest_generation
                && public.created_at_unix_ms == request.created_at_unix_ms
                && public.logical_file_size == self.logical_file_size
                && public.part_count == part_count
                && public.application_part_target == application_part_target
                && public.frame_plaintext_max == FRAME_PLAINTEXT_BYTES
                && public.master_key_generation == request.master_key_generation
                && public.file_key_wrap.wrap_generation == request.wrap_generation
                && opened.metadata == metadata
            {
                self.published_manifest_envelope = Some(downloaded);
                return Ok(object);
            }
            return Err(TransferError::ManifestCorrupted);
        }
        let file_key_wrap = wrap_file_key(
            master_key,
            &self.file_key,
            &request.vault_id,
            &self.package_bytes,
            request.master_key_generation,
            request.wrap_generation,
            &mut self.usage,
        )
        .map_err(map_crypto_error)?;
        let public = ManifestPublicHeader {
            package_id: self.package_bytes,
            vault_id: request.vault_id,
            manifest_generation: request.manifest_generation,
            created_at_unix_ms: request.created_at_unix_ms,
            logical_file_size: self.logical_file_size,
            part_count,
            application_part_target,
            frame_plaintext_max: FRAME_PLAINTEXT_BYTES,
            nonce_strategy_id: NONCE_STRATEGY_ID,
            crypto_suite_id: CRYPTO_SUITE_ID,
            file_key_wrap,
            master_key_generation: request.master_key_generation,
            flags: 0,
        };
        let bytes = seal_manifest(
            &public,
            &metadata,
            &self.file_key,
            ManifestLimits::default(),
            &mut self.usage,
        )
        .map_err(map_crypto_error)?;
        if bytes.len() > MAX_TRANSFER_OBJECT_BYTES {
            return Err(TransferError::ManifestCorrupted);
        }
        let result = self.store.upload(&name, MANIFEST_CAPTION, bytes);
        let object = match result {
            Ok(object) => object,
            Err(UploadError::Definite(error)) => return Err(error),
            Err(UploadError::AmbiguousSuccess) => self
                .store
                .search_exact_caption(MANIFEST_CAPTION, MAX_RECONCILIATION_RESULTS)?
                .into_iter()
                .find(|candidate| candidate.name == name)
                .ok_or(TransferError::Network)?,
        };
        let downloaded = self.store.download(object.object_id)?;
        if downloaded.len() as u64 != object.encoded_size {
            return Err(TransferError::HashMismatch);
        }
        let opened = open_manifest(&downloaded, master_key, ManifestLimits::default())
            .map_err(map_crypto_error)?;
        if opened.public_header != public || opened.metadata != metadata {
            return Err(TransferError::HashMismatch);
        }
        self.published_manifest_envelope = Some(downloaded);
        Ok(object)
    }
}

pub(crate) fn recover_manifest<S: RemoteObjectStore>(
    store: &mut S,
    master_key: &VaultMasterKey,
    object: &RemoteByteObject,
) -> Result<teleark_crypto::OpenedManifest, TransferError> {
    recover_manifest_observed(store, master_key, object, None)
}

pub(crate) fn recover_manifest_observed<S: RemoteObjectStore>(
    store: &mut S,
    master_key: &VaultMasterKey,
    object: &RemoteByteObject,
    observer: Option<&crate::ManagedScanObserver>,
) -> Result<teleark_crypto::OpenedManifest, TransferError> {
    if let Some(observer) = observer {
        observer.phase(crate::ChannelSyncPhase::ManifestReceiving);
    }
    let bytes = store.download(object.object_id)?;
    if let Some(observer) = observer {
        observer.phase(crate::ChannelSyncPhase::ManifestVerifying);
    }
    if bytes.len() as u64 != object.encoded_size || bytes.len() > MAX_TRANSFER_OBJECT_BYTES {
        return Err(TransferError::HashMismatch);
    }
    let manifest =
        open_manifest(&bytes, master_key, ManifestLimits::default()).map_err(map_crypto_error)?;
    if object.name != remote_manifest_name(&manifest.public_header.package_id) {
        return Err(TransferError::ManifestCorrupted);
    }
    Ok(manifest)
}

/// Scan the common manifest caption and independently authenticate every
/// candidate. Corrupt objects are reported without hiding valid packages.
pub fn recover_remote_manifests<S: RemoteObjectStore>(
    store: &mut S,
    master_key: &VaultMasterKey,
    limit: usize,
) -> Result<ManifestRecoveryReport, TransferError> {
    let bounded_limit = limit.min(MAX_RECONCILIATION_RESULTS);
    if bounded_limit == 0 {
        return Ok(ManifestRecoveryReport::default());
    }
    let candidates = store.search_exact_caption(MANIFEST_CAPTION, bounded_limit)?;
    let mut report = ManifestRecoveryReport::default();
    for object in candidates {
        let recovered = recover_manifest(store, master_key, &object);
        match recovered {
            Ok(manifest) => report
                .recovered
                .push(RecoveredManifest { object, manifest }),
            Err(TransferError::Cancelled) => return Err(TransferError::Cancelled),
            Err(error) => report.rejected.push(RejectedManifest { object, error }),
        }
    }
    Ok(report)
}

impl<S: RemoteObjectStore> RemoteTransport for EncryptedRemoteTransport<S> {
    fn discover_remote(&mut self, key: RemotePartKey) -> Result<Vec<RemoteObject>, TransferError> {
        self.validate_key(key)?;
        if let Some(part) = self.manifest_parts.get(&key.part_index.get()).cloned() {
            let object = RemoteByteObject {
                object_id: u64::try_from(part.remote_locator.message_id)
                    .map_err(|_| TransferError::ManifestCorrupted)?,
                name: part.remote_locator.remote_name,
                encoded_size: part.encoded_length,
            };
            let (_, decoded) = self.fetch_decode(key, &object)?;
            return Ok(vec![decoded]);
        }
        let caption = self.caption(key);
        let expected_name = self.name(key);
        let objects = self
            .store
            .search_exact_caption(&caption, MAX_RECONCILIATION_RESULTS)?;
        let mut result = Vec::new();
        for object in objects
            .into_iter()
            .filter(|object| object.name == expected_name)
        {
            let (_, decoded) = self.fetch_decode(key, &object)?;
            result.push(decoded);
        }
        Ok(result)
    }

    fn upload_remote(
        &mut self,
        key: RemotePartKey,
        bytes: &[u8],
        digest: ContentDigest,
    ) -> Result<RemoteObject, UploadError> {
        let (encoded, mut manifest_part) = self
            .encrypt(key, bytes, digest)
            .map_err(UploadError::Definite)?;
        let name = self.name(key);
        let caption = self.caption(key);
        let object = self.store.upload(&name, &caption, encoded)?;
        self.hydrated_objects.insert(object.object_id);
        manifest_part.remote_locator.message_id = i64::try_from(object.object_id)
            .map_err(|_| UploadError::Definite(TransferError::ManifestCorrupted))?;
        self.manifest_parts
            .insert(key.part_index.get(), manifest_part);
        Ok(RemoteObject {
            object_id: object.object_id,
            key,
            plaintext_size: bytes.len() as u64,
            encoded_size: object.encoded_size,
            digest,
        })
    }

    fn verify_remote(
        &mut self,
        object: &RemoteObject,
        expected_size: u64,
        expected_digest: ContentDigest,
    ) -> Result<(), TransferError> {
        let stored = RemoteByteObject {
            object_id: object.object_id,
            name: self.name(object.key),
            encoded_size: object.encoded_size,
        };
        let (_, decoded) = self.fetch_decode(object.key, &stored)?;
        if decoded.plaintext_size != expected_size || decoded.digest != expected_digest {
            return Err(TransferError::HashMismatch);
        }
        Ok(())
    }

    fn download_remote(&mut self, object: &RemoteObject) -> Result<Vec<u8>, TransferError> {
        let stored = RemoteByteObject {
            object_id: object.object_id,
            name: self.name(object.key),
            encoded_size: object.encoded_size,
        };
        let (plaintext, decoded) = self.fetch_decode(object.key, &stored)?;
        if decoded.plaintext_size != object.plaintext_size || decoded.digest != object.digest {
            return Err(TransferError::HashMismatch);
        }
        Ok(plaintext)
    }

    fn finalize_upload(
        &mut self,
        account_id: AccountId,
        package_id: PackageId,
        whole_digest: ContentDigest,
    ) -> Result<(), TransferError> {
        if account_id != self.account_id || package_id != self.package_id {
            return Err(TransferError::ManifestCorrupted);
        }
        if let Some(recovered_digest) = self.recovered_whole_digest {
            return (recovered_digest == whole_digest.0)
                .then_some(())
                .ok_or(TransferError::HashMismatch);
        }
        let publication = self
            .manifest_publication
            .take()
            .ok_or(TransferError::KeyUnavailable)?;
        if publication.request.whole_plaintext_blake3 != whole_digest.0 {
            self.manifest_publication = Some(publication);
            return Err(TransferError::HashMismatch);
        }
        let result = self.publish_manifest(&publication.master_key, publication.request.clone());
        if result.is_err() {
            self.manifest_publication = Some(publication);
        }
        result.map(|_| ())
    }
}

/// SQLite-backed implementation of the engine's atomic checkpoint port.
/// Every per-part blob uses the explicit `TARKCP01` codec below.
pub struct SqliteCheckpointStore {
    database: Database,
}

impl SqliteCheckpointStore {
    #[must_use]
    pub const fn new(database: Database) -> Self {
        Self { database }
    }

    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self, TransferError> {
        Database::open(path)
            .map(Self::new)
            .map_err(|_| TransferError::Database)
    }

    /// Creates the durable task/part rows once. Reopening the same transfer
    /// validates its immutable layout without replacing verified checkpoints.
    pub fn initialize_transfer(
        &mut self,
        task: &TransferTask,
        account_id: AccountId,
    ) -> Result<(), TransferError> {
        if let Some((stored, parts)) = self
            .database
            .transfer(task.id())
            .map_err(|_| TransferError::Database)?
        {
            if stored.logical_file_id != task.logical_file_id()
                || stored.total_bytes != task.progress().total_bytes
                || parts.len() != task.parts().len()
                || parts.iter().zip(task.parts()).any(|(stored, planned)| {
                    stored.index != planned.index()
                        || stored.offset_bytes != planned.offset_bytes()
                        || stored.size_bytes != planned.size_bytes()
                })
            {
                return Err(TransferError::Database);
            }
            return Ok(());
        }

        let now = now_unix_ms()?;
        let (mut record, parts) = TransferTaskRecord::from_core(task, now);
        record.account_id = Some(account_id);
        self.database
            .save_transfer(&record, &parts)
            .map_err(|_| TransferError::Database)
    }

    #[must_use]
    pub const fn database(&self) -> &Database {
        &self.database
    }
}

impl CheckpointPort for SqliteCheckpointStore {
    fn load_checkpoint(
        &mut self,
        transfer_id: TransferId,
    ) -> Result<Option<TransferCheckpoint>, TransferError> {
        let Some((_, stored_parts)) = self
            .database
            .transfer(transfer_id)
            .map_err(|_| TransferError::Database)?
        else {
            return Ok(None);
        };
        let mut decoded = Vec::with_capacity(stored_parts.len());
        for part in &stored_parts {
            match (part.checkpoint_version, part.checkpoint_data.as_deref()) {
                (None, None) => decoded.push(None),
                (Some(CHECKPOINT_VERSION), Some(bytes)) => {
                    decoded.push(Some(decode_checkpoint_part(bytes)?));
                }
                _ => return Err(TransferError::Database),
            }
        }
        let Some(first) = decoded.iter().flatten().next() else {
            return Ok(None);
        };
        let source_identity = first.source_identity;
        let destination_id = first.destination_id;
        let mut parts = Vec::with_capacity(stored_parts.len());
        for (stored, decoded) in stored_parts.iter().zip(decoded) {
            let part = match decoded {
                Some(decoded)
                    if decoded.source_identity == source_identity
                        && decoded.destination_id == destination_id
                        && decoded.part.part_index == stored.index =>
                {
                    decoded.part
                }
                Some(_) => return Err(TransferError::Database),
                None => PartCheckpoint {
                    part_index: stored.index,
                    attempts: stored.attempts,
                    retry_not_before_ms: 0,
                    verified: None,
                },
            };
            parts.push(part);
        }
        Ok(Some(TransferCheckpoint {
            transfer_id,
            source_identity,
            destination_id,
            parts,
        }))
    }

    fn save_checkpoint(&mut self, checkpoint: &TransferCheckpoint) -> Result<(), TransferError> {
        let Some((mut task, mut stored_parts)) = self
            .database
            .transfer(checkpoint.transfer_id)
            .map_err(|_| TransferError::Database)?
        else {
            return Err(TransferError::Database);
        };
        if checkpoint.parts.len() != stored_parts.len() {
            return Err(TransferError::Database);
        }
        let now = now_unix_ms()?;
        for (stored, part) in stored_parts.iter_mut().zip(&checkpoint.parts) {
            if stored.index != part.part_index {
                return Err(TransferError::Database);
            }
            stored.attempts = part.attempts;
            stored.checkpoint_version = Some(CHECKPOINT_VERSION);
            stored.checkpoint_data = Some(encode_checkpoint_part(
                checkpoint.source_identity,
                checkpoint.destination_id,
                part,
            ));
            stored.updated_at_unix_ms = now;
            if let Some(verified) = &part.verified {
                stored.transferred_bytes = stored.size_bytes;
                stored.state = StoredPartState::Verified;
                stored.remote_object_id = verified
                    .remote_object
                    .as_ref()
                    .map(|object| object.object_id);
            } else if part.retry_not_before_ms != 0 {
                stored.state = StoredPartState::WaitingRetry;
            } else {
                stored.state = StoredPartState::Queued;
            }
        }
        task.transferred_bytes = stored_parts
            .iter()
            .try_fold(0_u64, |total, part| {
                total.checked_add(part.transferred_bytes)
            })
            .ok_or(TransferError::Database)?;
        task.retry_count = stored_parts
            .iter()
            .map(|part| part.attempts)
            .max()
            .unwrap_or_default();
        task.state = if stored_parts
            .iter()
            .all(|part| part.state == StoredPartState::Verified)
        {
            StoredTransferState::Verifying
        } else if stored_parts
            .iter()
            .any(|part| part.state == StoredPartState::WaitingRetry)
        {
            StoredTransferState::WaitingRetry
        } else {
            StoredTransferState::Queued
        };
        task.updated_at_unix_ms = now;
        self.database
            .save_transfer(&task, &stored_parts)
            .map_err(|_| TransferError::Database)
    }

    fn save_task_state(&mut self, core: &TransferTask) -> Result<(), TransferError> {
        let Some((mut task, mut parts)) = self
            .database
            .transfer(core.id())
            .map_err(|_| TransferError::Database)?
        else {
            return Err(TransferError::Database);
        };
        if parts.len() != core.parts().len()
            || parts
                .iter()
                .zip(core.parts())
                .any(|(stored, current)| stored.index != current.index())
        {
            return Err(TransferError::Database);
        }
        let now = now_unix_ms()?;
        let progress = core.progress();
        task.state = core.state().into();
        task.transferred_bytes = progress.transferred_bytes;
        task.updated_at_unix_ms = now;
        for (stored, current) in parts.iter_mut().zip(core.parts()) {
            stored.state = current.state().into();
            stored.transferred_bytes = current.transferred_bytes();
            stored.updated_at_unix_ms = now;
        }
        self.database
            .save_transfer(&task, &parts)
            .map_err(|_| TransferError::Database)
    }
}

/// Concrete composition of filesystem, encrypted remote transport, SQLite
/// checkpoints, and BLAKE3 used by the existing transfer engine.
pub struct ProductionTransferIo<S> {
    pub files: NativeFileSystem,
    pub remote: EncryptedRemoteTransport<S>,
    pub checkpoints: SqliteCheckpointStore,
    digest: Blake3Digest,
}

impl<S> ProductionTransferIo<S> {
    #[must_use]
    pub const fn new(
        files: NativeFileSystem,
        remote: EncryptedRemoteTransport<S>,
        checkpoints: SqliteCheckpointStore,
    ) -> Self {
        Self {
            files,
            remote,
            checkpoints,
            digest: Blake3Digest,
        }
    }
}

impl<S> DigestPort for ProductionTransferIo<S> {
    fn digest(&self, bytes: &[u8]) -> ContentDigest {
        self.digest.digest(bytes)
    }
}

impl<S> SourcePort for ProductionTransferIo<S> {
    fn source_identity(&self, source_id: SourceId) -> Result<SourceIdentity, TransferError> {
        self.files.source_identity(source_id)
    }

    fn read_source_range(
        &mut self,
        source_id: SourceId,
        offset: u64,
        length: u64,
    ) -> Result<Vec<u8>, TransferError> {
        self.files.read_source_range(source_id, offset, length)
    }

    fn digest_source(&mut self, source_id: SourceId) -> Result<ContentDigest, TransferError> {
        self.files.digest_source(source_id)
    }
}

impl<S: RemoteObjectStore> RemoteTransport for ProductionTransferIo<S> {
    fn discover_remote(&mut self, key: RemotePartKey) -> Result<Vec<RemoteObject>, TransferError> {
        self.remote.discover_remote(key)
    }

    fn upload_remote(
        &mut self,
        key: RemotePartKey,
        bytes: &[u8],
        digest: ContentDigest,
    ) -> Result<RemoteObject, UploadError> {
        self.remote.upload_remote(key, bytes, digest)
    }

    fn verify_remote(
        &mut self,
        object: &RemoteObject,
        expected_size: u64,
        expected_digest: ContentDigest,
    ) -> Result<(), TransferError> {
        self.remote
            .verify_remote(object, expected_size, expected_digest)
    }

    fn download_remote(&mut self, object: &RemoteObject) -> Result<Vec<u8>, TransferError> {
        self.remote.download_remote(object)
    }

    fn finalize_upload(
        &mut self,
        account_id: AccountId,
        package_id: PackageId,
        whole_digest: ContentDigest,
    ) -> Result<(), TransferError> {
        self.remote
            .finalize_upload(account_id, package_id, whole_digest)
    }
}

impl<S> CheckpointPort for ProductionTransferIo<S> {
    fn load_checkpoint(
        &mut self,
        transfer_id: TransferId,
    ) -> Result<Option<TransferCheckpoint>, TransferError> {
        self.checkpoints.load_checkpoint(transfer_id)
    }

    fn save_checkpoint(&mut self, checkpoint: &TransferCheckpoint) -> Result<(), TransferError> {
        self.checkpoints.save_checkpoint(checkpoint)
    }

    fn save_task_state(&mut self, task: &TransferTask) -> Result<(), TransferError> {
        self.checkpoints.save_task_state(task)
    }
}

impl<S> FileSystemPort for ProductionTransferIo<S> {
    fn prepare_partial(
        &mut self,
        destination_id: DestinationId,
        total_bytes: u64,
    ) -> Result<(), TransferError> {
        self.files.prepare_partial(destination_id, total_bytes)
    }

    fn write_partial(
        &mut self,
        destination_id: DestinationId,
        offset: u64,
        bytes: &[u8],
    ) -> Result<(), TransferError> {
        self.files.write_partial(destination_id, offset, bytes)
    }

    fn read_partial(
        &mut self,
        destination_id: DestinationId,
        offset: u64,
        length: u64,
    ) -> Result<Vec<u8>, TransferError> {
        self.files.read_partial(destination_id, offset, length)
    }

    fn digest_partial(
        &mut self,
        destination_id: DestinationId,
    ) -> Result<ContentDigest, TransferError> {
        self.files.digest_partial(destination_id)
    }

    fn flush_partial(&mut self, destination_id: DestinationId) -> Result<(), TransferError> {
        self.files.flush_partial(destination_id)
    }

    fn atomic_finalize(&mut self, destination_id: DestinationId) -> Result<(), TransferError> {
        self.files.atomic_finalize(destination_id)
    }

    fn final_exists(&self, destination_id: DestinationId) -> bool {
        self.files.final_exists(destination_id)
    }
}

struct DecodedCheckpointPart {
    source_identity: Option<SourceIdentity>,
    destination_id: Option<DestinationId>,
    part: PartCheckpoint,
}

fn encode_checkpoint_part(
    source_identity: Option<SourceIdentity>,
    destination_id: Option<DestinationId>,
    part: &PartCheckpoint,
) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(192);
    bytes.extend_from_slice(CHECKPOINT_MAGIC);
    bytes.push(u8::from(source_identity.is_some()));
    bytes.push(u8::from(destination_id.is_some()));
    if let Some(identity) = source_identity {
        bytes.extend_from_slice(&identity.filesystem_id.to_be_bytes());
        bytes.extend_from_slice(&identity.size_bytes.to_be_bytes());
        bytes.extend_from_slice(&identity.modified_at_units.to_be_bytes());
        bytes.extend_from_slice(&identity.revision.to_be_bytes());
    }
    if let Some(destination) = destination_id {
        bytes.extend_from_slice(&destination.0.to_be_bytes());
    }
    bytes.extend_from_slice(&part.part_index.get().to_be_bytes());
    bytes.extend_from_slice(&part.attempts.to_be_bytes());
    bytes.extend_from_slice(&part.retry_not_before_ms.to_be_bytes());
    bytes.push(u8::from(part.verified.is_some()));
    if let Some(verified) = &part.verified {
        bytes.extend_from_slice(&verified.size_bytes.to_be_bytes());
        bytes.extend_from_slice(&verified.digest.0);
        bytes.push(u8::from(verified.remote_object.is_some()));
        if let Some(remote) = &verified.remote_object {
            bytes.extend_from_slice(&remote.object_id.to_be_bytes());
            bytes.extend_from_slice(&remote.key.account_id.get().to_be_bytes());
            bytes.extend_from_slice(&remote.key.package_id.get().to_be_bytes());
            bytes.extend_from_slice(&remote.key.part_index.get().to_be_bytes());
            bytes.extend_from_slice(&remote.plaintext_size.to_be_bytes());
            bytes.extend_from_slice(&remote.encoded_size.to_be_bytes());
            bytes.extend_from_slice(&remote.digest.0);
        }
    }
    bytes
}

fn decode_checkpoint_part(bytes: &[u8]) -> Result<DecodedCheckpointPart, TransferError> {
    let mut decoder = CheckpointDecoder::new(bytes);
    if decoder.take(8)? != CHECKPOINT_MAGIC {
        return Err(TransferError::Database);
    }
    let has_source_identity = decoder.flag()?;
    let has_destination_id = decoder.flag()?;
    let source_identity = match has_source_identity {
        true => Some(SourceIdentity {
            filesystem_id: decoder.u128()?,
            size_bytes: decoder.u64()?,
            modified_at_units: decoder.u64()?,
            revision: decoder.u64()?,
        }),
        false => None,
    };
    let destination_id = has_destination_id
        .then(|| decoder.u64())
        .transpose()?
        .map(DestinationId);
    let part_index = PartIndex::new(decoder.u32()?);
    let attempts = decoder.u32()?;
    let retry_not_before_ms = decoder.u64()?;
    let verified = if decoder.flag()? {
        let size_bytes = decoder.u64()?;
        let digest = ContentDigest(decoder.array_32()?);
        let remote_object = if decoder.flag()? {
            Some(RemoteObject {
                object_id: decoder.u64()?,
                key: RemotePartKey {
                    account_id: AccountId::new(decoder.i64()?),
                    package_id: PackageId::new(decoder.u64()?),
                    part_index: PartIndex::new(decoder.u32()?),
                },
                plaintext_size: decoder.u64()?,
                encoded_size: decoder.u64()?,
                digest: ContentDigest(decoder.array_32()?),
            })
        } else {
            None
        };
        Some(VerifiedPart {
            size_bytes,
            digest,
            remote_object,
        })
    } else {
        None
    };
    if !decoder.is_empty() {
        return Err(TransferError::Database);
    }
    Ok(DecodedCheckpointPart {
        source_identity,
        destination_id,
        part: PartCheckpoint {
            part_index,
            attempts,
            retry_not_before_ms,
            verified,
        },
    })
}

struct CheckpointDecoder<'a> {
    remaining: &'a [u8],
}

impl<'a> CheckpointDecoder<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { remaining: bytes }
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], TransferError> {
        if length > self.remaining.len() {
            return Err(TransferError::Database);
        }
        let (value, remaining) = self.remaining.split_at(length);
        self.remaining = remaining;
        Ok(value)
    }

    fn flag(&mut self) -> Result<bool, TransferError> {
        match self.take(1)? {
            [0] => Ok(false),
            [1] => Ok(true),
            _ => Err(TransferError::Database),
        }
    }

    fn u32(&mut self) -> Result<u32, TransferError> {
        self.take(4)?
            .try_into()
            .map(u32::from_be_bytes)
            .map_err(|_| TransferError::Database)
    }

    fn u64(&mut self) -> Result<u64, TransferError> {
        self.take(8)?
            .try_into()
            .map(u64::from_be_bytes)
            .map_err(|_| TransferError::Database)
    }

    fn i64(&mut self) -> Result<i64, TransferError> {
        self.take(8)?
            .try_into()
            .map(i64::from_be_bytes)
            .map_err(|_| TransferError::Database)
    }

    fn u128(&mut self) -> Result<u128, TransferError> {
        self.take(16)?
            .try_into()
            .map(u128::from_be_bytes)
            .map_err(|_| TransferError::Database)
    }

    fn array_32(&mut self) -> Result<[u8; 32], TransferError> {
        self.take(32)?
            .try_into()
            .map_err(|_| TransferError::Database)
    }

    const fn is_empty(&self) -> bool {
        self.remaining.is_empty()
    }
}

fn now_unix_ms() -> Result<i64, TransferError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| TransferError::Database)?;
    i64::try_from(duration.as_millis()).map_err(|_| TransferError::Database)
}

fn validate_part_sizes(total: u64, parts: &[u64]) -> Result<(), TransferError> {
    if parts.is_empty()
        || parts
            .iter()
            .any(|size| *size == 0 || *size > ENCRYPTED_PART_PLAINTEXT_BYTES)
    {
        return Err(TransferError::ManifestCorrupted);
    }
    let sum = parts
        .iter()
        .try_fold(0_u64, |total, size| total.checked_add(*size))
        .ok_or(TransferError::ManifestCorrupted)?;
    if sum != total {
        return Err(TransferError::ManifestCorrupted);
    }
    Ok(())
}

pub(crate) fn package_bytes(package_id: PackageId) -> [u8; 16] {
    let mut bytes = *b"TARKPKG1\0\0\0\0\0\0\0\0";
    bytes[8..].copy_from_slice(&package_id.get().to_be_bytes());
    bytes
}

pub(crate) fn package_id_from_bytes(bytes: [u8; 16]) -> Result<PackageId, TransferError> {
    if bytes[..8] != *b"TARKPKG1" {
        return Err(TransferError::ManifestCorrupted);
    }
    let value = bytes[8..]
        .try_into()
        .map(u64::from_be_bytes)
        .map_err(|_| TransferError::ManifestCorrupted)?;
    Ok(PackageId::new(value))
}

pub(crate) fn hex_id(bytes: &[u8; 16]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(32);
    for byte in bytes {
        output.push(char::from(HEX[(byte >> 4) as usize]));
        output.push(char::from(HEX[(byte & 0x0f) as usize]));
    }
    output
}

fn map_application_error(error: ApplicationError) -> TransferError {
    match error.kind() {
        ApplicationErrorKind::Authorization => TransferError::Authorization,
        ApplicationErrorKind::NotFound | ApplicationErrorKind::SourceMissing => {
            TransferError::RemoteMissing
        }
        ApplicationErrorKind::PermissionDenied => TransferError::PermissionDenied,
        ApplicationErrorKind::Cancelled => TransferError::Cancelled,
        ApplicationErrorKind::Persistence => TransferError::Database,
        ApplicationErrorKind::Network | ApplicationErrorKind::Server => TransferError::Network,
        ApplicationErrorKind::InvalidRequest
        | ApplicationErrorKind::Conflict
        | ApplicationErrorKind::SourceChanged
        | ApplicationErrorKind::Capacity => TransferError::ManifestCorrupted,
        _ => TransferError::Network,
    }
}

fn map_crypto_error(error: CryptoError) -> TransferError {
    match error {
        CryptoError::Cancelled => TransferError::Cancelled,
        CryptoError::AuthenticationFailed => TransferError::AuthenticationFailed,
        CryptoError::UnsupportedVersion { major, .. } => {
            TransferError::UnsupportedManifestVersion {
                version: u32::from(major),
            }
        }
        CryptoError::RandomSourceFailed | CryptoError::AeadIdentityAlreadyUsed => {
            TransferError::KeyUnavailable
        }
        _ => TransferError::ManifestCorrupted,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        sync::{Arc, Mutex},
    };

    use teleark_core::{
        FileKind, LogicalFileId, PartIndex, TransferDirection, TransferPriority, TransferState,
    };
    use teleark_storage::{AccountRecord, NewLogicalFileRecord};
    use teleark_transfer::{
        Clock, DownloadSpec, EncryptionPipelineConfig, JitterSource, PipelinePart, ProgressConfig,
        RetryPolicy, SchedulerConfig, TransferEngine, TransferEngineConfig, UploadSpec,
        run_encryption_upload_pipeline,
    };

    use super::*;

    #[derive(Clone, Default)]
    struct FakeStore {
        inner: Arc<Mutex<FakeStoreState>>,
    }

    #[derive(Default)]
    struct FakeStoreState {
        next_id: u64,
        objects: BTreeMap<u64, (String, String, Vec<u8>)>,
    }

    impl FakeStore {
        fn encoded(&self, id: u64) -> Vec<u8> {
            self.inner
                .lock()
                .ok()
                .and_then(|state| state.objects.get(&id).map(|object| object.2.clone()))
                .unwrap_or_default()
        }

        fn object_count(&self) -> usize {
            self.inner
                .lock()
                .map(|state| state.objects.len())
                .unwrap_or_default()
        }
    }

    impl RemoteObjectStore for FakeStore {
        fn search_exact_caption(
            &mut self,
            caption: &str,
            limit: usize,
        ) -> Result<Vec<RemoteByteObject>, TransferError> {
            let state = self.inner.lock().map_err(|_| TransferError::Network)?;
            Ok(state
                .objects
                .iter()
                .filter(|(_, object)| object.1 == caption)
                .take(limit)
                .map(|(id, object)| RemoteByteObject {
                    object_id: *id,
                    name: object.0.clone(),
                    encoded_size: object.2.len() as u64,
                })
                .collect())
        }

        fn upload(
            &mut self,
            name: &str,
            caption: &str,
            bytes: Vec<u8>,
        ) -> Result<RemoteByteObject, UploadError> {
            let mut state = self
                .inner
                .lock()
                .map_err(|_| UploadError::Definite(TransferError::Network))?;
            state.next_id = state.next_id.saturating_add(1);
            let id = state.next_id;
            let encoded_size = bytes.len() as u64;
            state
                .objects
                .insert(id, (name.to_owned(), caption.to_owned(), bytes));
            Ok(RemoteByteObject {
                object_id: id,
                name: name.to_owned(),
                encoded_size,
            })
        }

        fn download(&mut self, object_id: u64) -> Result<Vec<u8>, TransferError> {
            self.encoded(object_id)
                .is_empty()
                .then_some(())
                .map_or_else(
                    || Ok(self.encoded(object_id)),
                    |_| Err(TransferError::RemoteMissing),
                )
        }
    }

    fn key() -> RemotePartKey {
        RemotePartKey {
            account_id: AccountId::new(9),
            package_id: PackageId::new(11),
            part_index: PartIndex::new(0),
        }
    }

    #[test]
    fn cancelling_manifest_recovery_stops_before_the_next_candidate() {
        struct CancelledStore {
            downloads: usize,
        }
        impl RemoteObjectStore for CancelledStore {
            fn search_exact_caption(
                &mut self,
                _: &str,
                _: usize,
            ) -> Result<Vec<RemoteByteObject>, TransferError> {
                Ok((1..=3)
                    .map(|object_id| RemoteByteObject {
                        object_id,
                        name: "candidate".into(),
                        encoded_size: 0,
                    })
                    .collect())
            }
            fn upload(
                &mut self,
                _: &str,
                _: &str,
                _: Vec<u8>,
            ) -> Result<RemoteByteObject, UploadError> {
                unreachable!("read only recovery")
            }
            fn download(&mut self, _: u64) -> Result<Vec<u8>, TransferError> {
                self.downloads += 1;
                Err(TransferError::Cancelled)
            }
        }
        let mut store = CancelledStore { downloads: 0 };
        let master = VaultMasterKey::from_bytes([8; 32]);
        assert!(matches!(
            recover_remote_manifests(&mut store, &master, 3),
            Err(TransferError::Cancelled)
        ));
        assert_eq!(store.downloads, 1);
    }

    #[test]
    fn prepared_parts_use_the_bounded_parallel_encryption_pipeline() -> Result<(), TransferError> {
        let store = FakeStore::default();
        let part_bytes = [vec![1_u8; 32 * 1024], vec![2_u8; 24 * 1024]];
        let part_sizes = part_bytes
            .iter()
            .map(|bytes| bytes.len() as u64)
            .collect::<Vec<_>>();
        let total_bytes = part_sizes.iter().sum();
        let mut remote = EncryptedRemoteTransport::new(
            store.clone(),
            AccountId::new(9),
            99,
            PackageId::new(11),
            FileKey::from_bytes([7; 32]),
            total_bytes,
            part_sizes.clone(),
        )?;
        let mut offset = 0_u64;
        let mut descriptors = Vec::new();
        let mut plans = Vec::new();
        for (position, plaintext_length) in part_sizes.iter().copied().enumerate() {
            let part_index = u32::try_from(position).map_err(|_| TransferError::SourceChanged)?;
            let key = RemotePartKey {
                account_id: AccountId::new(9),
                package_id: PackageId::new(11),
                part_index: PartIndex::new(part_index),
            };
            descriptors.push(PipelinePart {
                part_index,
                plaintext_offset: offset,
                plaintext_length,
            });
            plans.push(remote.plan_part_encryption(key, None)?);
            offset = offset.saturating_add(plaintext_length);
        }
        let context = remote.encryption_context();
        let report = run_encryption_upload_pipeline(
            EncryptionPipelineConfig::new(2, 1, 2).map_err(|_| TransferError::SourceChanged)?,
            &descriptors,
            |descriptor| {
                part_bytes
                    .get(descriptor.part_index as usize)
                    .cloned()
                    .ok_or(TransferError::SourceChanged)
            },
            |descriptor, plaintext| {
                let plan = plans
                    .get(descriptor.part_index as usize)
                    .cloned()
                    .ok_or(TransferError::SourceChanged)?;
                EncryptedRemoteTransport::<FakeStore>::encrypt_planned_part(
                    &context, plan, plaintext,
                )
            },
            |_, prepared| remote.upload_prepared_part(prepared).map(|_| ()),
        )
        .map_err(|error| match error {
            teleark_transfer::EncryptionPipelineError::Read { source, .. }
            | teleark_transfer::EncryptionPipelineError::Encrypt { source, .. }
            | teleark_transfer::EncryptionPipelineError::Upload { source, .. } => source,
            _ => TransferError::Network,
        })?;
        assert_eq!(report.completed_parts, 2);
        assert_eq!(store.object_count(), 2);
        Ok(())
    }

    #[test]
    fn encrypted_manifest_recovers_key_layout_and_remote_locator() -> Result<(), TransferError> {
        let store = FakeStore::default();
        let plaintext = b"bounded encrypted Telegram object";
        let digest = Blake3Digest.digest(plaintext);
        let master_key = VaultMasterKey::from_bytes([8; 32]);
        let mut first = EncryptedRemoteTransport::new(
            store.clone(),
            AccountId::new(9),
            99,
            PackageId::new(11),
            FileKey::from_bytes([7; 32]),
            plaintext.len() as u64,
            vec![plaintext.len() as u64],
        )?;
        let uploaded =
            first
                .upload_remote(key(), plaintext, digest)
                .map_err(|error| match error {
                    UploadError::Definite(error) => error,
                    UploadError::AmbiguousSuccess => TransferError::Network,
                })?;
        let encoded = store.encoded(uploaded.object_id);
        assert!(
            !encoded
                .windows(plaintext.len())
                .any(|window| window == plaintext)
        );
        first.verify_remote(&uploaded, plaintext.len() as u64, digest)?;
        assert_eq!(
            first.download_manifest_part(key()),
            Err(TransferError::ManifestCorrupted)
        );
        let request = manifest_request(digest);
        first.publish_manifest(&master_key, request.clone())?;
        first.publish_manifest(&master_key, request)?;
        assert_eq!(store.object_count(), 2);

        let mut recovery_store = store.clone();
        let mut report = recover_remote_manifests(&mut recovery_store, &master_key, 100)?;
        assert!(report.rejected.is_empty());
        assert_eq!(report.recovered.len(), 1);
        let opened = report
            .recovered
            .pop()
            .ok_or(TransferError::ManifestCorrupted)?
            .manifest;
        let mut recovered = EncryptedRemoteTransport::from_opened_manifest(store.clone(), opened)?;
        let discovered = recovered.discover_remote(key())?;
        assert_eq!(discovered.len(), 1);
        assert_eq!(recovered.download_remote(&discovered[0])?, plaintext);
        assert_eq!(recovered.download_manifest_part(key())?, plaintext);
        {
            let mut state = store.inner.lock().map_err(|_| TransferError::Network)?;
            let object = state
                .objects
                .get_mut(&uploaded.object_id)
                .ok_or(TransferError::RemoteMissing)?;
            let last = object
                .2
                .last_mut()
                .ok_or(TransferError::ManifestCorrupted)?;
            *last ^= 1;
        }
        assert!(
            recovered.download_manifest_part(key()).is_err(),
            "authenticated manifest does not authorize corrupted part bytes"
        );
        Ok(())
    }

    #[test]
    fn part_planner_is_bounded_and_gap_free() -> Result<(), TransferError> {
        let total = ENCRYPTED_PART_PLAINTEXT_BYTES * 2 + 17;
        let parts = encrypted_part_sizes(total)?;
        assert_eq!(
            parts,
            vec![
                ENCRYPTED_PART_PLAINTEXT_BYTES,
                ENCRYPTED_PART_PLAINTEXT_BYTES,
                17
            ]
        );
        assert_eq!(parts.iter().sum::<u64>(), total);
        Ok(())
    }

    #[test]
    fn checkpoint_codec_rejects_trailing_and_round_trips_verified_evidence()
    -> Result<(), TransferError> {
        let part = PartCheckpoint {
            part_index: PartIndex::new(2),
            attempts: 3,
            retry_not_before_ms: 99,
            verified: Some(VerifiedPart {
                size_bytes: 7,
                digest: ContentDigest([4; 32]),
                remote_object: Some(RemoteObject {
                    object_id: 44,
                    key: RemotePartKey {
                        account_id: AccountId::new(-9),
                        package_id: PackageId::new(10),
                        part_index: PartIndex::new(2),
                    },
                    plaintext_size: 7,
                    encoded_size: 135,
                    digest: ContentDigest([4; 32]),
                }),
            }),
        };
        let source = SourceIdentity {
            filesystem_id: 5,
            size_bytes: 21,
            modified_at_units: 6,
            revision: 7,
        };
        let encoded = encode_checkpoint_part(Some(source), None, &part);
        let decoded = decode_checkpoint_part(&encoded)?;
        assert_eq!(decoded.source_identity, Some(source));
        assert_eq!(decoded.destination_id, None);
        assert_eq!(decoded.part, part);

        let mut trailing = encoded;
        trailing.push(0);
        assert_eq!(
            decode_checkpoint_part(&trailing).map(|_| ()),
            Err(TransferError::Database)
        );
        Ok(())
    }

    struct TestClock;

    impl Clock for TestClock {
        fn now_millis(&self) -> u64 {
            1
        }
    }

    struct NoJitter;

    impl JitterSource for NoJitter {
        fn jitter_millis(&mut self, _maximum_millis: u64) -> u64 {
            0
        }
    }

    fn engine_config() -> Result<TransferEngineConfig, teleark_transfer::ConfigurationError> {
        Ok(TransferEngineConfig::new(
            SchedulerConfig::new(1, 1, 1, 1, 1, 16)?,
            RetryPolicy::new(3, 1, 10, 0)?,
            ProgressConfig::new(1, 32, 8)?,
        ))
    }

    fn manifest_request(whole_digest: ContentDigest) -> ManifestPublishRequest {
        ManifestPublishRequest {
            vault_id: [6; 16],
            manifest_generation: 1,
            master_key_generation: 1,
            wrap_generation: 1,
            created_at_unix_ms: 1,
            logical_name: "source.bin".to_owned(),
            relative_path: None,
            mime_type: Some("application/octet-stream".to_owned()),
            media_kind: MediaKind::Other,
            whole_plaintext_blake3: whole_digest.0,
        }
    }

    fn prepare_database(
        path: &std::path::Path,
        source: &std::path::Path,
        size: u64,
    ) -> Result<LogicalFileId, Box<dyn std::error::Error>> {
        let mut database = Database::open(path)?;
        database.upsert_account(&AccountRecord {
            id: AccountId::new(9),
            display_name: "test".to_owned(),
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
        })?;
        let file = database.insert_logical_file(&NewLogicalFileRecord::local_import(
            source.to_owned(),
            "source.bin".to_owned(),
            size,
            FileKind::Other,
            Some(1),
        ))?;
        Ok(file.id)
    }

    #[test]
    fn engine_uses_real_files_sqlite_crypto_resume_and_remote_recovery()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let source_path = directory.path().join("source.bin");
        let destination_path = directory.path().join("restored.bin");
        let database_path = directory.path().join("library.sqlite3");
        let recovery_database_path = directory.path().join("recovered.sqlite3");
        let plaintext = b"abcdefgh";
        std::fs::write(&source_path, plaintext)?;
        let logical_file_id =
            prepare_database(&database_path, &source_path, plaintext.len() as u64)?;
        let part_sizes = vec![4, 4];
        let store = FakeStore::default();
        let digest = Blake3Digest;
        let part_digests = vec![
            digest.digest(&plaintext[..4]),
            digest.digest(&plaintext[4..]),
        ];
        let whole_digest = digest.digest(plaintext);
        let master_key = VaultMasterKey::from_bytes([8; 32]);

        let upload_task = TransferTask::try_new(
            TransferId::new(1),
            logical_file_id,
            TransferDirection::Upload,
            plaintext.len() as u64,
            TransferPriority::NORMAL,
            &part_sizes,
        )?;
        let mut checkpoints = SqliteCheckpointStore::open(&database_path)?;
        checkpoints.initialize_transfer(&upload_task, AccountId::new(9))?;
        let mut files = NativeFileSystem::new();
        files.register_source(SourceId(1), &source_path)?;
        let source_identity = files.source_identity(SourceId(1))?;
        let mut remote = EncryptedRemoteTransport::new(
            store.clone(),
            AccountId::new(9),
            99,
            PackageId::new(11),
            FileKey::from_bytes([7; 32]),
            plaintext.len() as u64,
            part_sizes.clone(),
        )?;
        remote.configure_manifest_publication(
            VaultMasterKey::from_bytes([8; 32]),
            manifest_request(whole_digest),
        );
        let mut io = ProductionTransferIo::new(files, remote, checkpoints);
        let upload_spec = UploadSpec {
            account_id: AccountId::new(9),
            package_id: PackageId::new(11),
            source_id: SourceId(1),
            source_identity,
            part_digests: part_digests.clone(),
            whole_digest,
        };
        let mut first = TransferEngine::new(engine_config()?);
        first.enqueue_upload(&mut io, upload_task, upload_spec.clone(), &TestClock)?;
        first.step(&mut io, &TestClock, &mut NoJitter)?;
        assert_eq!(store.object_count(), 1);
        drop(first);
        drop(io);

        let restarted_task = TransferTask::try_new(
            TransferId::new(1),
            logical_file_id,
            TransferDirection::Upload,
            plaintext.len() as u64,
            TransferPriority::NORMAL,
            &part_sizes,
        )?;
        let mut checkpoints = SqliteCheckpointStore::open(&database_path)?;
        checkpoints.initialize_transfer(&restarted_task, AccountId::new(9))?;
        let mut files = NativeFileSystem::new();
        files.register_source(SourceId(1), &source_path)?;
        let mut remote = EncryptedRemoteTransport::new(
            store.clone(),
            AccountId::new(9),
            99,
            PackageId::new(11),
            FileKey::from_bytes([7; 32]),
            plaintext.len() as u64,
            part_sizes.clone(),
        )?;
        remote.configure_manifest_publication(
            VaultMasterKey::from_bytes([8; 32]),
            manifest_request(whole_digest),
        );
        let mut io = ProductionTransferIo::new(files, remote, checkpoints);
        let mut restarted = TransferEngine::new(engine_config()?);
        restarted.enqueue_upload(&mut io, restarted_task, upload_spec.clone(), &TestClock)?;
        restarted.step(&mut io, &TestClock, &mut NoJitter)?;
        assert_eq!(
            restarted.task(TransferId::new(1)).map(TransferTask::state),
            Some(TransferState::Completed)
        );
        assert_eq!(
            io.checkpoints
                .database()
                .transfer(TransferId::new(1))?
                .map(|value| value.0.state),
            Some(StoredTransferState::Completed)
        );
        assert_eq!(store.object_count(), 3);
        drop(restarted);
        drop(io);

        let mut remote_recovery_store = store.clone();
        let mut report = recover_remote_manifests(&mut remote_recovery_store, &master_key, 100)?;
        assert!(report.rejected.is_empty());
        let opened = report
            .recovered
            .pop()
            .ok_or(TransferError::ManifestCorrupted)?
            .manifest;
        let recovered_part_sizes = opened
            .metadata
            .parts
            .iter()
            .map(|part| part.plaintext_length)
            .collect::<Vec<_>>();
        let recovered_part_digests = opened
            .metadata
            .parts
            .iter()
            .map(|part| ContentDigest(part.plaintext_blake3))
            .collect::<Vec<_>>();
        let recovered_whole_digest = ContentDigest(opened.metadata.whole_plaintext_blake3);
        let remote = EncryptedRemoteTransport::from_opened_manifest(store.clone(), opened)?;

        let download_task = TransferTask::try_new(
            TransferId::new(2),
            logical_file_id,
            TransferDirection::Download,
            plaintext.len() as u64,
            TransferPriority::NORMAL,
            &recovered_part_sizes,
        )?;
        let mut checkpoints = SqliteCheckpointStore::open(&database_path)?;
        checkpoints.initialize_transfer(&download_task, AccountId::new(9))?;
        let mut files = NativeFileSystem::new();
        files.register_destination(DestinationId(2), &destination_path)?;
        let mut io = ProductionTransferIo::new(files, remote, checkpoints);
        let mut download = TransferEngine::new(engine_config()?);
        download.enqueue_download(
            &mut io,
            download_task,
            DownloadSpec {
                account_id: AccountId::new(9),
                package_id: PackageId::new(11),
                destination_id: DestinationId(2),
                part_digests: recovered_part_digests,
                whole_digest: recovered_whole_digest,
            },
            &TestClock,
        )?;
        download.step(&mut io, &TestClock, &mut NoJitter)?;
        download.step(&mut io, &TestClock, &mut NoJitter)?;
        assert_eq!(std::fs::read(&destination_path)?, plaintext);
        assert_eq!(
            io.checkpoints
                .database()
                .transfer(TransferId::new(2))?
                .map(|value| value.0.state),
            Some(StoredTransferState::Completed)
        );
        drop(download);
        drop(io);

        let recovered_logical_id = prepare_database(
            &recovery_database_path,
            &source_path,
            plaintext.len() as u64,
        )?;
        let recovery_task = TransferTask::try_new(
            TransferId::new(3),
            recovered_logical_id,
            TransferDirection::Upload,
            plaintext.len() as u64,
            TransferPriority::NORMAL,
            &recovered_part_sizes,
        )?;
        let mut checkpoints = SqliteCheckpointStore::open(&recovery_database_path)?;
        checkpoints.initialize_transfer(&recovery_task, AccountId::new(9))?;
        let mut files = NativeFileSystem::new();
        files.register_source(SourceId(3), &source_path)?;
        let recovered_identity = files.source_identity(SourceId(3))?;
        let mut recovery_store = store.clone();
        let mut report = recover_remote_manifests(&mut recovery_store, &master_key, 100)?;
        let opened = report
            .recovered
            .pop()
            .ok_or(TransferError::ManifestCorrupted)?
            .manifest;
        let remote = EncryptedRemoteTransport::from_opened_manifest(store.clone(), opened)?;
        let mut io = ProductionTransferIo::new(files, remote, checkpoints);
        let mut recovery = TransferEngine::new(engine_config()?);
        recovery.enqueue_upload(
            &mut io,
            recovery_task,
            UploadSpec {
                source_id: SourceId(3),
                source_identity: recovered_identity,
                ..upload_spec
            },
            &TestClock,
        )?;
        recovery.step(&mut io, &TestClock, &mut NoJitter)?;
        recovery.step(&mut io, &TestClock, &mut NoJitter)?;
        assert_eq!(store.object_count(), 3);
        assert_eq!(
            recovery.task(TransferId::new(3)).map(TransferTask::state),
            Some(TransferState::Completed)
        );
        Ok(())
    }
}
