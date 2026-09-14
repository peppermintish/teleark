//! Resumes an immutable, persisted part identity across process lifetimes.
use super::*;
use crate::VaultPartRecovery;
use std::num::NonZeroI64;

/// Store capability required by durable uploads. There is deliberately no
/// fallback to ordinary upload, which could allocate a different message ID.
pub trait ReservedPublicationStore: RemoteObjectStore {
    #[cfg(not(test))]
    fn upload_stream_reserved(
        &mut self,
        name: &str,
        caption: &str,
        stream: teleark_telegram::UploadStream,
        random_id: NonZeroI64,
        tuning: teleark_telegram::TransferTuning,
    ) -> Result<RemoteByteObject, UploadError>;
    #[cfg(test)]
    fn upload_stream_reserved(
        &mut self,
        name: &str,
        caption: &str,
        mut stream: teleark_telegram::UploadStream,
        random_id: NonZeroI64,
        _tuning: teleark_telegram::TransferTuning,
    ) -> Result<RemoteByteObject, UploadError> {
        let mut bytes = Vec::new();
        while let Some(block) = stream.blocks.blocking_recv() {
            bytes.extend_from_slice(&block);
            let _ = stream.recycled.try_send(block);
        }
        if stream.sealed.blocking_recv() != Ok(true) {
            return Err(UploadError::Definite(TransferError::SourceChanged));
        }
        self.upload_reserved(name, caption, bytes, random_id)
    }
    fn upload_reserved(
        &mut self,
        name: &str,
        caption: &str,
        bytes: Vec<u8>,
        random_id: NonZeroI64,
    ) -> Result<RemoteByteObject, UploadError>;
}

impl ReservedPublicationStore for TelegramObjectStore {
    fn upload_stream_reserved(
        &mut self,
        name: &str,
        caption: &str,
        stream: teleark_telegram::UploadStream,
        random_id: NonZeroI64,
        tuning: teleark_telegram::TransferTuning,
    ) -> Result<RemoteByteObject, UploadError> {
        let encoded_size = stream.total_bytes;
        self.telegram
            .upload_stream_observed(
                self.account_id,
                self.chat_id,
                name.into(),
                caption.into(),
                stream,
                teleark_telegram::StreamUploadOptions {
                    observer: self.observer.clone(),
                    random_id: random_id.get(),
                    tuning,
                },
                self.cancellation.clone(),
            )
            .map(|object_id| RemoteByteObject {
                object_id: object_id as u64,
                name: name.into(),
                encoded_size,
            })
            .map_err(|error| UploadError::Definite(map_application_error(error)))
    }
    fn upload_reserved(
        &mut self,
        name: &str,
        caption: &str,
        bytes: Vec<u8>,
        random_id: NonZeroI64,
    ) -> Result<RemoteByteObject, UploadError> {
        TelegramObjectStore::upload_reserved(self, name, caption, bytes, random_id)
    }
}

impl<S> EncryptedRemoteTransport<S> {
    /// Reserve once, then commit the returned identity to durable storage
    /// before starting its one encryption attempt. Retirement allocates a new
    /// instance and is committed separately before another attempt.
    pub fn reserve_part_identity(
        &mut self,
        key: RemotePartKey,
        plaintext_digest: ContentDigest,
        random_id: NonZeroI64,
    ) -> Result<VaultPartRecovery, TransferError> {
        let plan = self.plan_part_encryption(key, Some(plaintext_digest))?;
        Ok(VaultPartRecovery {
            header: plan.header,
            plaintext_blake3: plaintext_digest.0,
            publication_random_id: random_id.get(),
        })
    }

    /// Advance a retired instance monotonically. Even an RNG collision cannot
    /// select any earlier instance for this part; overflow fails closed.
    pub(crate) fn replacement_part_identity(
        &self,
        key: RemotePartKey,
        previous: &VaultPartRecovery,
        digest: ContentDigest,
        random_id: NonZeroI64,
    ) -> Result<VaultPartRecovery, TransferError> {
        self.restore_reserved_plan(key, previous)?;
        if previous.plaintext_blake3 != digest.0 {
            return Err(TransferError::SourceChanged);
        }
        if random_id.get() == previous.publication_random_id {
            return Err(TransferError::KeyUnavailable);
        }
        let next = u128::from_be_bytes(previous.header.part_instance_id.0)
            .checked_add(1)
            .ok_or(TransferError::KeyUnavailable)?;
        let mut replacement = previous.clone();
        replacement.header.part_instance_id = teleark_crypto::PartInstanceId(next.to_be_bytes());
        replacement.publication_random_id = random_id.get();
        Ok(replacement)
    }

    /// Reconstruct the exact ciphertext, reconcile remote publication, and
    /// verify the remote bytes before returning a receipt for durable commit.
    #[cfg(test)]
    pub(crate) fn resume_reserved_part(
        &mut self,
        key: RemotePartKey,
        reservation: &VaultPartRecovery,
        plaintext: Vec<u8>,
    ) -> Result<RemoteObject, TransferError>
    where
        S: ReservedPublicationStore,
    {
        self.resume_reserved_part_with_receipt(key, reservation, plaintext, None)
    }

    #[cfg(test)]
    pub(crate) fn resume_reserved_part_with_receipt(
        &mut self,
        key: RemotePartKey,
        reservation: &VaultPartRecovery,
        plaintext: Vec<u8>,
        receipt_id: Option<u64>,
    ) -> Result<RemoteObject, TransferError>
    where
        S: ReservedPublicationStore,
    {
        let plan = self.restore_reserved_plan(key, reservation)?;
        let prepared = Self::encrypt_planned_part(&self.encryption_context(), plan, plaintext)?;
        self.publish_reserved_prepared(prepared, reservation, receipt_id)
    }

    pub(crate) fn restore_reserved_plan(
        &self,
        key: RemotePartKey,
        reservation: &VaultPartRecovery,
    ) -> Result<PartEncryptionPlan, TransferError> {
        let position = self.validate_key(key)?;
        NonZeroI64::new(reservation.publication_random_id)
            .ok_or(TransferError::ManifestCorrupted)?;
        let expected_header = PartHeader::new(
            self.package_bytes,
            reservation.header.part_instance_id,
            key.part_index.get(),
            u32::try_from(self.part_sizes.len()).map_err(|_| TransferError::ManifestCorrupted)?,
            self.logical_file_size,
            self.plaintext_offset(position)?,
            self.part_sizes[position],
            reservation.header.frame_plaintext_max,
            self.limits,
        )
        .map_err(map_crypto_error)?;
        let expected_header = if reservation.header.format_major == 2 {
            expected_header
                .aligned(self.limits)
                .map_err(map_crypto_error)?
        } else {
            expected_header
        };
        if reservation.header != expected_header {
            return Err(TransferError::ManifestCorrupted);
        }
        let expected_encoded_size = expected_header
            .expected_encoded_length()
            .map_err(map_crypto_error)?;
        if expected_encoded_size > teleark_telegram::MAX_STREAM_OBJECT_BYTES {
            return Err(TransferError::ManifestCorrupted);
        }
        Ok(PartEncryptionPlan {
            key,
            chat_id: self.chat_id,
            header: expected_header,
            expected_digest: Some(ContentDigest(reservation.plaintext_blake3)),
            expected_encoded_size,
            remote_name: self.name(key),
        })
    }

    #[cfg(test)]
    pub(crate) fn publish_reserved_prepared(
        &mut self,
        mut prepared: PreparedEncryptedPart,
        reservation: &VaultPartRecovery,
        receipt_id: Option<u64>,
    ) -> Result<RemoteObject, TransferError>
    where
        S: ReservedPublicationStore,
    {
        let key = prepared.key;
        let plan = self.restore_reserved_plan(key, reservation)?;
        let expected_encoded_size = plan.expected_encoded_size;
        let random_id = NonZeroI64::new(reservation.publication_random_id)
            .ok_or(TransferError::ManifestCorrupted)?;
        if prepared.plaintext_digest.0 != reservation.plaintext_blake3
            || prepared.manifest_part.part_instance_id != reservation.header.part_instance_id.0
            || prepared.encoded.len() as u64 != expected_encoded_size
        {
            return Err(TransferError::ManifestCorrupted);
        }
        if let Some(object_id) = receipt_id {
            return self.finish_prepared_part(
                prepared,
                RemoteByteObject {
                    object_id,
                    name: self.name(key),
                    encoded_size: expected_encoded_size,
                },
            );
        }
        let caption = self.caption(key);
        let candidates = self
            .store
            .search_exact_caption(&caption, MAX_RECONCILIATION_RESULTS)?;
        if candidates.len() >= MAX_RECONCILIATION_RESULTS {
            return Err(TransferError::RemoteMissing);
        }
        for object in candidates {
            if object.name != self.name(key) || object.encoded_size != expected_encoded_size {
                continue;
            }
            let encoded = self.store.download(object.object_id)?;
            // A caption/name match alone cannot authenticate a receipt. The
            // deterministic expected ciphertext binds every header and byte.
            if encoded == prepared.encoded {
                return self.finish_prepared_part(prepared, object);
            }
        }
        let object = self
            .store
            .upload_reserved(
                &prepared.manifest_part.remote_locator.remote_name,
                &caption,
                std::mem::take(&mut prepared.encoded),
                random_id,
            )
            .map_err(|error| match error {
                UploadError::Definite(error) => error,
                UploadError::AmbiguousSuccess => TransferError::Network,
            })?;
        self.finish_prepared_part(prepared, object)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, rc::Rc};

    #[derive(Clone, Default)]
    struct Store(Rc<RefCell<State>>);
    #[derive(Default)]
    struct State {
        objects: BTreeMap<i64, (RemoteByteObject, String, Vec<u8>)>,
        uploads: usize,
        reads: usize,
        lose_ack: bool,
        hide_search: bool,
        ledger: Option<std::path::PathBuf>,
        after_send: Option<Box<dyn FnOnce()>>,
    }
    impl RemoteObjectStore for Store {
        fn search_exact_caption(
            &mut self,
            caption: &str,
            limit: usize,
        ) -> Result<Vec<RemoteByteObject>, TransferError> {
            let mut state = self.0.borrow_mut();
            state.reads += 1;
            Ok(if state.hide_search {
                vec![]
            } else {
                state
                    .objects
                    .values()
                    .filter(|(_, saved, _)| saved == caption)
                    .take(limit)
                    .map(|(object, _, _)| object.clone())
                    .collect()
            })
        }
        fn upload(
            &mut self,
            _: &str,
            _: &str,
            _: Vec<u8>,
        ) -> Result<RemoteByteObject, UploadError> {
            panic!("durable path must not allocate a fresh publication ID")
        }
        fn download(&mut self, id: u64) -> Result<Vec<u8>, TransferError> {
            let mut state = self.0.borrow_mut();
            state.reads += 1;
            state
                .objects
                .values()
                .find(|(object, _, _)| object.object_id == id)
                .map(|(_, _, bytes)| bytes.clone())
                .ok_or(TransferError::RemoteMissing)
        }
    }
    impl ReservedPublicationStore for Store {
        fn upload_reserved(
            &mut self,
            name: &str,
            caption: &str,
            bytes: Vec<u8>,
            id: NonZeroI64,
        ) -> Result<RemoteByteObject, UploadError> {
            let mut state = self.0.borrow_mut();
            if let Some(path) = &state.ledger {
                let db =
                    teleark_storage::Database::open(path).expect("independent ledger connection");
                if caption == MANIFEST_CAPTION {
                    let saved = db
                        .vault_manifest_outbox(1, 9)
                        .expect("outbox read")
                        .expect("reservation before send");
                    assert_eq!(saved.random_id, id.get());
                    assert_eq!(saved.envelope.as_deref(), Some(bytes.as_slice()));
                } else {
                    let parts = db
                        .vault_parts(1, 9, None, 1)
                        .expect("committed reservation visible before send");
                    assert_eq!(parts.len(), 1);
                    assert_eq!(
                        VaultPartRecovery::decode(&parts[0].identity)
                            .expect("reserved identity")
                            .publication_random_id,
                        id.get()
                    );
                }
            }
            state.uploads += 1;
            let object = RemoteByteObject {
                object_id: 10 + state.objects.len() as u64,
                name: name.into(),
                encoded_size: bytes.len() as u64,
            };
            let stored =
                state
                    .objects
                    .entry(id.get())
                    .or_insert((object, caption.into(), bytes.clone()));
            assert_eq!(
                stored.2, bytes,
                "reserved ID must retain identical ciphertext"
            );
            let object = stored.0.clone();
            if let Some(after_send) = state.after_send.take() {
                after_send();
            }
            if std::mem::take(&mut state.lose_ack) {
                Err(UploadError::AmbiguousSuccess)
            } else {
                Ok(object)
            }
        }
    }
    fn key() -> RemotePartKey {
        RemotePartKey {
            account_id: AccountId::new(1),
            package_id: PackageId::new(2),
            part_index: PartIndex::new(0),
        }
    }
    fn transport(store: Store) -> EncryptedRemoteTransport<Store> {
        EncryptedRemoteTransport::new(
            store,
            key().account_id,
            3,
            key().package_id,
            FileKey::from_bytes([42; 32]),
            4,
            vec![4],
        )
        .expect("valid fixture operation")
    }
    fn reservation(transport: &mut EncryptedRemoteTransport<Store>) -> VaultPartRecovery {
        transport
            .reserve_part_identity(
                key(),
                Blake3Digest.digest(b"data"),
                NonZeroI64::new(-100).expect("nonzero fixture publication ID"),
            )
            .expect("valid fixture operation")
    }

    #[test]
    fn lost_receipt_is_reconciled_after_transport_reconstruction() {
        let store = Store::default();
        store.0.borrow_mut().lose_ack = true;
        let mut original = transport(store.clone());
        let reserved = reservation(&mut original);
        let saved = reserved.encode().expect("encode reservation");
        assert_eq!(
            original.resume_reserved_part(key(), &reserved, b"data".to_vec()),
            Err(TransferError::Network)
        );
        drop(original);
        let mut restarted = transport(store.clone());
        let restored = VaultPartRecovery::decode(&saved).expect("restore reservation");
        let receipt = restarted
            .resume_reserved_part(key(), &restored, b"data".to_vec())
            .expect("valid fixture operation");
        assert_eq!(receipt.object_id, 10);
        assert_eq!(store.0.borrow().uploads, 1);
        assert_eq!(store.0.borrow().objects.len(), 1);
        assert_eq!(restarted.manifest_parts[&0].remote_locator.message_id, 10);
    }

    #[test]
    fn lagging_search_reuses_publication_id_and_identical_ciphertext() {
        let store = Store::default();
        store.0.borrow_mut().lose_ack = true;
        store.0.borrow_mut().hide_search = true;
        let mut original = transport(store.clone());
        let saved = reservation(&mut original);
        assert_eq!(
            original.resume_reserved_part(key(), &saved, b"data".to_vec()),
            Err(TransferError::Network)
        );
        drop(original);
        let receipt = transport(store.clone())
            .resume_reserved_part(key(), &saved, b"data".to_vec())
            .expect("valid fixture operation");
        assert_eq!(receipt.object_id, 10);
        assert_eq!(store.0.borrow().uploads, 2);
        assert_eq!(store.0.borrow().objects.len(), 1);
    }

    #[test]
    fn corrupt_remote_object_cannot_be_committed_as_a_verified_part() {
        let store = Store::default();
        store.0.borrow_mut().lose_ack = true;
        let mut original = transport(store.clone());
        let saved = reservation(&mut original);
        assert_eq!(
            original.resume_reserved_part(key(), &saved, b"data".to_vec()),
            Err(TransferError::Network)
        );
        // Simulate a server returning different stored bytes for the receipt.
        // The fake's identity dedup still returns its original object ID.
        store
            .0
            .borrow_mut()
            .objects
            .get_mut(&-100)
            .expect("published fixture")
            .2[100] ^= 1;
        let mut restarted = transport(store.clone());
        // A matching caption with different ciphertext must never be accepted.
        // Exercise the receipt-verification boundary directly.
        let object = store.0.borrow().objects[&-100].0.clone();
        let plan = PartEncryptionPlan {
            key: key(),
            chat_id: 3,
            header: saved.header.clone(),
            expected_digest: Some(ContentDigest(saved.plaintext_blake3)),
            expected_encoded_size: saved
                .header
                .expected_encoded_length()
                .expect("valid fixture layout"),
            remote_name: restarted.name(key()),
        };
        let prepared = EncryptedRemoteTransport::<Store>::encrypt_planned_part(
            &restarted.encryption_context(),
            plan,
            b"data".to_vec(),
        )
        .expect("valid fixture operation");
        assert_eq!(
            restarted.finish_prepared_part(prepared, object),
            Err(TransferError::HashMismatch)
        );
        assert!(restarted.manifest_parts.is_empty());
    }

    #[test]
    fn changed_plaintext_and_foreign_layout_fail_before_remote_work() {
        let store = Store::default();
        let mut worker = transport(store.clone());
        let mut saved = reservation(&mut worker);
        assert_eq!(
            worker.resume_reserved_part(key(), &saved, b"edit".to_vec()),
            Err(TransferError::SourceChanged)
        );
        saved.header.package_id[0] ^= 1;
        assert_eq!(
            worker.resume_reserved_part(key(), &saved, b"data".to_vec()),
            Err(TransferError::ManifestCorrupted)
        );
        assert_eq!(store.0.borrow().uploads, 0);
        assert_eq!(store.0.borrow().reads, 0);
    }
    fn admitted(
        path: &std::path::Path,
    ) -> (
        teleark_storage::Database,
        teleark_storage::VaultJobLease,
        VaultMasterKey,
    ) {
        use teleark_storage::{VaultJobLease, VaultJobState, VaultJobTransition};
        let master = VaultMasterKey::from_bytes([17; 32]);
        let package = package_bytes(key().package_id);
        let wrap = wrap_file_key(
            &master,
            &FileKey::from_bytes([42; 32]),
            &[3; 16],
            &package,
            1,
            1,
            &mut AeadUsageRegistry::new(),
        )
        .expect("wrap synthetic key");
        let context = crate::VaultRecoveryContext {
            container_plaintext_limit: crate::encrypted_part_plaintext_limit(),
            account_id: 1,
            task_id: 9,
            chat_id: 3,
            package_id: package,
            vault_id: [3; 16],
            master_key_generation: 1,
            file_key_wrap: wrap,
            file_name: "fixture.bin".into(),
            created_at_unix_ms: 100,
            size_bytes: 4,
            direction: crate::VaultRecoveryDirection::Upload {
                source: path.with_file_name("source.bin"),
                identity: SourceIdentity {
                    filesystem_id: 1,
                    size_bytes: 4,
                    modified_at_units: 1,
                    revision: 0,
                },
                source_blake3: *blake3::hash(b"data").as_bytes(),
            },
        };
        let mut db = teleark_storage::Database::open(path).expect("open ledger");
        db.admit_vault_job(&context.admission_record().expect("admission"))
            .expect("durable admission");
        let mut lease = VaultJobLease {
            account_id: 1,
            id: 9,
            generation: 0,
        };
        assert!(
            db.transition_vault_job(
                lease,
                VaultJobState::Queued,
                VaultJobTransition::Start,
                101,
                None
            )
            .expect("start")
        );
        lease.generation = 1;
        (db, lease, master)
    }

    #[test]
    fn cancelled_crypto_job_keeps_reservation_for_resume_without_remote_publication() {
        use teleark_storage::{VaultJobState, VaultJobTransition};
        let dir = tempfile::tempdir().expect("temporary ledger");
        let path = dir.path().join("jobs.sqlite");
        let (mut db, mut lease, master) = admitted(&path);
        let store = Store::default();
        let cancellation = crate::TelegramScanCancellation::new();
        let mut worker = crate::DurableUploadParts::open(&db, lease, store.clone(), &master)
            .expect("worker")
            .with_cancellation(cancellation.clone());
        let encryption = worker
            .prepare_part(&mut db, 0, b"data")
            .expect("reserved before crypto");
        let saved = db.vault_parts(1, 9, None, 1).expect("reservation")[0]
            .identity
            .clone();
        cancellation.cancel();
        assert!(matches!(
            encryption.encrypt(b"data".to_vec()),
            Err(TransferError::Cancelled)
        ));
        assert_eq!(store.0.borrow().uploads, 0);
        assert!(
            db.vault_parts(1, 9, None, 1).expect("unreceipted")[0]
                .receipt
                .is_none()
        );
        drop(worker);
        for (state, transition) in [
            (VaultJobState::Running, VaultJobTransition::RequestPause),
            (VaultJobState::Pausing, VaultJobTransition::AcknowledgePause),
            (VaultJobState::Paused, VaultJobTransition::Resume),
            (VaultJobState::Queued, VaultJobTransition::Start),
        ] {
            assert!(
                db.transition_vault_job(lease, state, transition, 102, None)
                    .expect("control")
            );
        }
        lease.generation += 1;
        let mut resumed =
            crate::DurableUploadParts::open(&db, lease, store.clone(), &master).expect("resumed");
        resumed
            .upload_part(&mut db, 0, b"data".to_vec())
            .expect("same reservation remains usable");
        assert_eq!(
            db.vault_parts(1, 9, None, 1).expect("receipt")[0].identity,
            saved
        );
        assert_eq!(store.0.borrow().uploads, 1);
    }

    fn durable_manifest_request() -> ManifestPublishRequest {
        ManifestPublishRequest {
            vault_id: [3; 16],
            manifest_generation: 1,
            master_key_generation: 1,
            wrap_generation: 1,
            created_at_unix_ms: 100,
            logical_name: "fixture.bin".into(),
            relative_path: None,
            mime_type: None,
            media_kind: MediaKind::Document,
            whole_plaintext_blake3: *blake3::hash(b"data").as_bytes(),
        }
    }

    #[test]
    fn sealed_v1_manifest_replays_original_bytes_after_upgrade() {
        let dir = tempfile::tempdir().expect("directory");
        let path = dir.path().join("jobs.sqlite");
        let (mut db, lease, master) = admitted(&path);
        let context = crate::VaultRecoveryContext::from_record(
            &db.vault_job(1, 9).expect("job").expect("saved"),
        )
        .expect("context");
        let key = context.file_key(&master).expect("file key");
        let header = PartHeader::new(
            context.package_id,
            teleark_crypto::PartInstanceId([61; 16]),
            0,
            1,
            4,
            0,
            4,
            8 * 1024 * 1024,
            PartLimits::default(),
        )
        .expect("v1 header");
        let mut bytes = Vec::new();
        let summary = teleark_crypto::encrypt_part(
            &mut &b"data"[..],
            &mut bytes,
            &header,
            &key,
            PartLimits::default(),
            &mut AeadUsageRegistry::new(),
        )
        .expect("old ciphertext");
        let mut store = Store::default();
        let name = remote_part_name(&context.package_id, 0);
        let object = store
            .upload_reserved(
                &name,
                "synthetic v1 part",
                bytes,
                NonZeroI64::new(51).expect("id"),
            )
            .expect("old object");
        let public = ManifestPublicHeader {
            package_id: context.package_id,
            vault_id: context.vault_id,
            manifest_generation: 1,
            created_at_unix_ms: 100,
            logical_file_size: 4,
            part_count: 1,
            application_part_target: 4,
            frame_plaintext_max: 8 * 1024 * 1024,
            nonce_strategy_id: NONCE_STRATEGY_ID,
            crypto_suite_id: CRYPTO_SUITE_ID,
            file_key_wrap: context.file_key_wrap.clone(),
            master_key_generation: 1,
            flags: 0,
        };
        let metadata = ManifestMetadata {
            logical_name: "fixture.bin".into(),
            relative_path: None,
            mime_type: None,
            media_kind: MediaKind::Document,
            whole_plaintext_blake3: *blake3::hash(b"data").as_bytes(),
            parts: vec![ManifestPart {
                part_index: 0,
                part_instance_id: header.part_instance_id.0,
                plaintext_offset: 0,
                plaintext_length: 4,
                encoded_length: object.encoded_size,
                frame_count: 1,
                plaintext_blake3: summary.plaintext_blake3,
                encoded_ciphertext_blake3: summary.encoded_blake3,
                remote_locator: RemoteLocator {
                    account_id: 1,
                    chat_id: 3,
                    message_id: object.object_id as i64,
                    remote_name: name,
                    locator_version: 1,
                    locator_extension: None,
                },
            }],
            logical_timestamps: None,
            source_metadata: vec![],
            format_extensions: vec![],
        };
        let limits = ManifestLimits::default();
        let commitment = teleark_crypto::manifest_content_commitment(&public, &metadata, limits)
            .expect("commitment");
        assert!(
            db.reserve_vault_manifest(
                lease,
                &teleark_storage::VaultManifestOutbox {
                    codec_version: 1,
                    commitment,
                    random_id: 52,
                    envelope: None,
                    message_id: None,
                }
            )
            .expect("reserve")
        );
        let envelope = seal_manifest(
            &public,
            &metadata,
            &key,
            limits,
            &mut AeadUsageRegistry::new(),
        )
        .expect("v1 manifest");
        assert!(
            db.save_vault_manifest_envelope(lease, &envelope)
                .expect("seal")
        );
        drop(db);
        let mut db = teleark_storage::Database::open(&path).expect("reopen");
        let mut worker = crate::DurableUploadParts::open(&db, lease, store.clone(), &master)
            .expect("upgraded worker");
        assert!(
            worker
                .resume_saved_manifest(&mut db, &master)
                .expect("resume old bytes")
                .is_some()
        );
        assert_eq!(
            db.vault_manifest_outbox(1, 9)
                .expect("outbox")
                .expect("saved")
                .envelope,
            Some(envelope)
        );
        assert_eq!(
            store.0.borrow().uploads,
            2,
            "one old part, one replayed manifest"
        );
    }

    #[test]
    fn manifest_outbox_replays_lost_ack_and_verifies_receipts_after_restart() {
        use teleark_storage::{VaultJobState, VaultJobTransition};
        let dir = tempfile::tempdir().expect("temporary ledger");
        let path = dir.path().join("jobs.sqlite");
        let (mut db, mut lease, master) = admitted(&path);
        let store = Store::default();
        store.0.borrow_mut().ledger = Some(path.clone());
        let mut worker =
            crate::DurableUploadParts::open(&db, lease, store.clone(), &master).expect("worker");
        assert_eq!(
            worker
                .resume_saved_manifest(&mut db, &master)
                .expect("no outbox"),
            None
        );
        assert_eq!(store.0.borrow().uploads, 0);
        worker
            .upload_part(&mut db, 0, b"data".to_vec())
            .expect("part");
        store.0.borrow_mut().lose_ack = true;
        assert_eq!(
            worker.publish_manifest(&mut db, &master, durable_manifest_request()),
            Err(TransferError::Network)
        );
        let reserved = db
            .vault_manifest_outbox(1, 9)
            .expect("outbox")
            .expect("saved");
        assert!(reserved.envelope.is_some());
        assert!(reserved.message_id.is_none());
        drop(worker);
        drop(db);
        let mut db = teleark_storage::Database::open(&path).expect("cold restart");
        db.recover_vault_jobs(1, 102).expect("recovery");
        lease.generation = 2;
        assert!(
            db.transition_vault_job(
                lease,
                VaultJobState::Queued,
                VaultJobTransition::Start,
                103,
                None
            )
            .expect("start")
        );
        lease.generation = 3;
        store.0.borrow_mut().hide_search = true;
        let mut worker =
            crate::DurableUploadParts::open(&db, lease, store.clone(), &master).expect("resumed");
        worker
            .upload_part(&mut db, 0, b"data".to_vec())
            .expect("saved part receipt");
        let mut changed = durable_manifest_request();
        changed.mime_type = Some("application/octet-stream".into());
        let before = store.0.borrow().uploads;
        assert_eq!(
            worker.publish_manifest(&mut db, &master, changed),
            Err(TransferError::ManifestCorrupted)
        );
        assert_eq!(
            store.0.borrow().uploads,
            before,
            "changed AEAD input never publishes"
        );
        drop(worker);
        let mut worker = crate::DurableUploadParts::open(&db, lease, store.clone(), &master)
            .expect("source-independent worker");
        let object = worker
            .resume_saved_manifest(&mut db, &master)
            .expect("stable replay despite hidden search without plaintext")
            .expect("persisted envelope");
        assert_eq!(
            store.0.borrow().objects.len(),
            2,
            "one part and one manifest"
        );
        let saved = db
            .vault_manifest_outbox(1, 9)
            .expect("outbox")
            .expect("receipt");
        assert_eq!(saved.random_id, reserved.random_id);
        assert_eq!(saved.envelope, reserved.envelope);
        assert_eq!(saved.message_id, Some(object.object_id as i64));
        let uploads = store.0.borrow().uploads;
        worker
            .publish_manifest(&mut db, &master, durable_manifest_request())
            .expect("direct receipt verification");
        assert_eq!(store.0.borrow().uploads, uploads);
        store
            .0
            .borrow_mut()
            .objects
            .get_mut(&saved.random_id)
            .expect("remote")
            .2[0] ^= 1;
        assert_eq!(
            worker.publish_manifest(&mut db, &master, durable_manifest_request()),
            Err(TransferError::HashMismatch)
        );
    }

    #[test]
    fn ledger_reopen_recovers_remote_success_without_replacing_identity() {
        use teleark_storage::{VaultJobState, VaultJobTransition};
        let dir = tempfile::tempdir().expect("temporary ledger");
        let path = dir.path().join("jobs.sqlite");
        let (mut db, mut lease, master) = admitted(&path);
        let store = Store::default();
        store.0.borrow_mut().ledger = Some(path.clone());
        store.0.borrow_mut().lose_ack = true;
        let mut worker =
            crate::DurableUploadParts::open(&db, lease, store.clone(), &master).expect("worker");
        assert_eq!(
            worker.upload_part(&mut db, 0, b"data".to_vec()),
            Err(TransferError::Network)
        );
        let reservation = db.vault_parts(1, 9, None, 1).expect("reserved")[0]
            .identity
            .clone();
        drop(worker);
        drop(db);
        let mut db = teleark_storage::Database::open(&path).expect("cold reopen");
        assert_eq!(
            db.recover_vault_jobs(1, 102)
                .expect("recover interrupted job"),
            1
        );
        lease.generation = 2;
        assert!(
            db.transition_vault_job(
                lease,
                VaultJobState::Queued,
                VaultJobTransition::Start,
                103,
                None
            )
            .expect("restart")
        );
        lease.generation = 3;
        let mut worker = crate::DurableUploadParts::open(&db, lease, store.clone(), &master)
            .expect("restored worker");
        assert_eq!(
            worker
                .upload_part(&mut db, 0, b"data".to_vec())
                .expect("reconcile")
                .object_id,
            10
        );
        let saved = &db.vault_parts(1, 9, None, 1).expect("receipted")[0];
        assert_eq!(saved.identity, reservation);
        assert!(saved.receipt.is_some());
        assert_eq!(store.0.borrow().uploads, 1);
        // A saved receipt is useful even while search remains unavailable.
        store.0.borrow_mut().hide_search = true;
        worker
            .upload_part(&mut db, 0, b"data".to_vec())
            .expect("verify saved receipt directly");
        assert_eq!(store.0.borrow().uploads, 1);
    }

    #[test]
    fn stale_ledger_worker_cannot_start_remote_work() {
        let dir = tempfile::tempdir().expect("temporary ledger");
        let (mut db, lease, master) = admitted(&dir.path().join("jobs.sqlite"));
        let store = Store::default();
        let mut worker =
            crate::DurableUploadParts::open(&db, lease, store.clone(), &master).expect("worker");
        db.recover_vault_jobs(1, 102)
            .expect("invalidate old generation");
        assert_eq!(
            worker.upload_part(&mut db, 0, b"data".to_vec()),
            Err(TransferError::Cancelled)
        );
        assert_eq!(store.0.borrow().uploads, 0);
        assert_eq!(store.0.borrow().reads, 0);
        assert!(
            db.vault_parts(1, 9, None, 1)
                .expect("no reservation")
                .is_empty()
        );
    }
    #[test]
    fn successful_send_can_be_receipted_while_pausing_but_not_after_cancel_ack() {
        use teleark_storage::{VaultJobState, VaultJobTransition};
        for acknowledge_cancel in [false, true] {
            let dir = tempfile::tempdir().expect("temporary ledger");
            let path = dir.path().join("jobs.sqlite");
            let (mut db, lease, master) = admitted(&path);
            let store = Store::default();
            let mut worker = crate::DurableUploadParts::open(&db, lease, store.clone(), &master)
                .expect("worker");
            store.0.borrow_mut().after_send = Some(Box::new(move || {
                let mut control =
                    teleark_storage::Database::open(&path).expect("independent control owner");
                if acknowledge_cancel {
                    assert!(
                        control
                            .transition_vault_job(
                                lease,
                                VaultJobState::Running,
                                VaultJobTransition::RequestCancel,
                                102,
                                None
                            )
                            .expect("cancel intent")
                    );
                    assert!(
                        control
                            .transition_vault_job(
                                lease,
                                VaultJobState::Cancelling,
                                VaultJobTransition::AcknowledgeCancel,
                                103,
                                None
                            )
                            .expect("cancel acknowledged")
                    );
                } else {
                    assert!(
                        control
                            .transition_vault_job(
                                lease,
                                VaultJobState::Running,
                                VaultJobTransition::RequestPause,
                                102,
                                None
                            )
                            .expect("pause intent")
                    );
                }
            }));
            let result = worker.upload_part(&mut db, 0, b"data".to_vec());
            let saved = db.vault_parts(1, 9, None, 1).expect("saved identity");
            if acknowledge_cancel {
                assert_eq!(result, Err(TransferError::Cancelled));
                assert!(saved[0].receipt.is_none());
            } else {
                assert!(result.is_ok());
                assert!(saved[0].receipt.is_some());
                assert_eq!(
                    worker.upload_part(&mut db, 0, b"data".to_vec()),
                    Err(TransferError::Cancelled)
                );
            }
            assert_eq!(store.0.borrow().uploads, 1);
        }
    }
    #[test]
    fn encrypted_results_waiting_in_the_queue_cannot_bypass_pause() {
        use teleark_storage::{VaultJobState, VaultJobTransition};
        let dir = tempfile::tempdir().expect("temporary ledger");
        let (mut db, lease, master) = admitted(&dir.path().join("jobs.sqlite"));
        let store = Store::default();
        let mut worker =
            crate::DurableUploadParts::open(&db, lease, store.clone(), &master).expect("worker");
        let job = worker
            .prepare_part(&mut db, 0, b"data")
            .expect("committed job");
        let prepared = job.encrypt(b"data".to_vec()).expect("parallel result");
        assert!(
            db.transition_vault_job(
                lease,
                VaultJobState::Running,
                VaultJobTransition::RequestPause,
                102,
                None
            )
            .expect("pause")
        );
        assert_eq!(
            worker.publish_prepared(&mut db, prepared),
            Err(TransferError::Cancelled)
        );
        assert_eq!(store.0.borrow().uploads, 0);
        assert_eq!(store.0.borrow().reads, 0);
        let saved = db.vault_parts(1, 9, None, 1).expect("retained reservation");
        assert!(saved[0].receipt.is_none());
    }

    #[test]
    fn blocked_encryption_job_does_not_hold_the_ledger_or_other_crypto_jobs() {
        use std::sync::mpsc;
        use std::time::Duration;
        use teleark_storage::{VaultJobState, VaultJobTransition};
        let dir = tempfile::tempdir().expect("temporary ledger");
        let (mut db, lease, master) = admitted(&dir.path().join("jobs.sqlite"));
        let store = Store::default();
        let mut worker =
            crate::DurableUploadParts::open(&db, lease, store, &master).expect("worker");
        let first = worker
            .prepare_part(&mut db, 0, b"data")
            .expect("first crypto job");
        let second = worker
            .prepare_part(&mut db, 0, b"data")
            .expect("second independent crypto job");
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        std::thread::scope(|scope| {
            scope.spawn(move || {
                entered_tx.send(()).expect("blocked worker entered");
                if release_rx.recv().is_ok() {
                    first.encrypt(b"data".to_vec()).expect("released crypto");
                }
            });
            entered_rx.recv().expect("wait for blocked worker");
            scope.spawn(move || {
                let _ = finished_tx.send(second.encrypt(b"data".to_vec()));
            });
            let result = finished_rx.recv_timeout(Duration::from_secs(5));
            let control = db.transition_vault_job(
                lease,
                VaultJobState::Running,
                VaultJobTransition::RequestPause,
                102,
                None,
            );
            // Release even on a failed assertion so scoped joins cannot hang.
            let _ = release_tx.send(());
            result
                .expect("independent crypto completes")
                .expect("valid encryption");
            assert!(control.expect("control database remains available"));
        });
    }
    #[test]
    fn bounded_encryption_pipeline_publishes_only_committed_jobs() {
        use std::sync::{Arc, Mutex};
        use teleark_transfer::{
            EncryptionPipelineConfig, PipelinePart, run_encryption_upload_pipeline,
        };
        let dir = tempfile::tempdir().expect("temporary ledger");
        let path = dir.path().join("jobs.sqlite");
        let (mut db, lease, master) = admitted(&path);
        let store = Store::default();
        store.0.borrow_mut().ledger = Some(path);
        let mut worker =
            crate::DurableUploadParts::open(&db, lease, store.clone(), &master).expect("worker");
        let job = worker
            .prepare_part_digest(&mut db, 0, 4, Blake3Digest.digest(b"data"))
            .expect("admitted digest");
        let jobs = Arc::new(Mutex::new(Some(job)));
        let report = run_encryption_upload_pipeline(
            EncryptionPipelineConfig::new(2, 1, 1).expect("bounded workers"),
            &[PipelinePart {
                part_index: 0,
                plaintext_offset: 0,
                plaintext_length: 4,
            }],
            |_| Ok(b"data".to_vec()),
            move |_, bytes| {
                let job = jobs
                    .lock()
                    .expect("job queue")
                    .take()
                    .expect("consume once");
                job.encrypt(bytes)
            },
            |_, prepared| worker.publish_prepared(&mut db, prepared).map(|_| ()),
        )
        .expect("pipeline");
        assert_eq!(report.completed_parts, 1);
        assert_eq!(store.0.borrow().uploads, 1);
        assert!(
            db.vault_parts(1, 9, None, 1).expect("receipt")[0]
                .receipt
                .is_some()
        );
    }
}
