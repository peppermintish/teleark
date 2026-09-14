use super::*;
pub(super) fn fixture() -> VaultRecoveryContext {
    VaultRecoveryContext {
        account_id: 7,
        task_id: 9,
        chat_id: 11,
        package_id: [2; 16],
        vault_id: [3; 16],
        master_key_generation: 1,
        file_key_wrap: FileKeyWrap {
            algorithm_id: CRYPTO_SUITE_ID,
            wrap_generation: 1,
            ciphertext: [4; 32],
            tag: [5; 16],
        },
        file_name: "fixture.txt".into(),
        created_at_unix_ms: 100,
        size_bytes: 123,
        direction: VaultRecoveryDirection::Upload {
            source: std::env::temp_dir().join("synthetic-source"),
            identity: SourceIdentity {
                filesystem_id: 1234,
                size_bytes: 123,
                modified_at_units: 99,
                revision: 1,
            },
            source_blake3: [6; 32],
        },
    }
}
#[test]
fn both_directions_preserve_wrapped_key_scope_and_file_identity() {
    let mut context = fixture();
    assert_eq!(
        VaultRecoveryContext::decode(&context.encode().expect("encode")).expect("decode"),
        context
    );
    context.direction = VaultRecoveryDirection::Download {
        destination: std::env::temp_dir().join("synthetic-target"),
        manifest_message_id: 45,
        manifest_blake3: [8; 32],
        whole_plaintext_blake3: [9; 32],
    };
    assert_eq!(
        VaultRecoveryContext::decode(&context.encode().expect("encode")).expect("decode"),
        context
    );
}
#[test]
fn rejects_every_truncation_corruption_and_future_version_without_reinterpretation() {
    let bytes = fixture().encode().expect("fixture");
    for n in 0..bytes.len() {
        assert!(
            VaultRecoveryContext::decode(&bytes[..n]).is_err(),
            "truncation {n}"
        );
    }
    for n in 0..bytes.len() {
        let mut changed = bytes.clone();
        changed[n] ^= 1;
        assert!(
            VaultRecoveryContext::decode(&changed).is_err(),
            "mutation {n}"
        );
    }
    let mut newer = bytes;
    newer[8..12].copy_from_slice(&2u32.to_le_bytes());
    assert_eq!(
        VaultRecoveryContext::decode(&newer),
        Err(RecoveryContextError::UnsupportedVersion)
    );
}
#[test]
fn rejects_invalid_scopes_and_source_size_instead_of_normalizing_them() {
    let mut context = fixture();
    context.account_id = 0;
    assert!(context.encode().is_err());
    context = fixture();
    context.size_bytes += 1;
    assert!(context.encode().is_err());
    context = fixture();
    context.file_key_wrap.wrap_generation = 0;
    assert!(context.encode().is_err());
    context = fixture();
    if let VaultRecoveryDirection::Upload { source, .. } = &mut context.direction {
        *source = "relative-path".into();
    }
    assert!(context.encode().is_err());
}
#[cfg(unix)]
#[test]
fn native_non_utf8_source_paths_round_trip_without_loss() {
    use std::os::unix::ffi::OsStringExt as _;
    let mut context = fixture();
    if let VaultRecoveryDirection::Upload { source, .. } = &mut context.direction {
        *source = std::ffi::OsString::from_vec(b"/tmp/synthetic-\xff".to_vec()).into();
    }
    assert_eq!(
        VaultRecoveryContext::decode(&context.encode().expect("encode")).expect("decode"),
        context
    );
}
#[test]
fn malformed_length_is_rejected_even_with_a_valid_checksum() {
    let mut bytes = fixture().encode().expect("encode");
    // Name length follows the 143-byte fixed header, including time and size.
    let offset = 8 + 4 + 1 + 8 + 8 + 8 + 16 + 16 + 4 + 2 + 4 + 32 + 16 + 8 + 8;
    bytes[offset..offset + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    let end = bytes.len() - 32;
    let hash = blake3::hash(&bytes[..end]);
    bytes[end..].copy_from_slice(hash.as_bytes());
    assert_eq!(
        VaultRecoveryContext::decode(&bytes),
        Err(RecoveryContextError::Invalid)
    );
}

#[test]
fn persisted_context_restores_the_same_key_and_rejects_foreign_scope() {
    use teleark_crypto::{
        AeadUsageRegistry, OsRandom, generate_file_key, generate_vault_master_key, wrap_file_key,
    };
    let master = generate_vault_master_key(&mut OsRandom).expect("synthetic master");
    let original = generate_file_key(&mut OsRandom).expect("synthetic file key");
    let mut context = fixture();
    context.file_key_wrap = wrap_file_key(
        &master,
        &original,
        &context.vault_id,
        &context.package_id,
        1,
        1,
        &mut AeadUsageRegistry::new(),
    )
    .expect("wrapped file key");
    let temp = tempfile::tempdir().expect("temporary database");
    let path = temp.path().join("context.sqlite");
    {
        let mut database = teleark_storage::Database::open(&path).expect("database");
        database
            .admit_vault_job(&context.admission_record().expect("record"))
            .expect("admit");
    }
    let database = teleark_storage::Database::open(&path).expect("reopened database");
    let record = database.vault_job(7, 9).expect("read").expect("admitted");
    let restored = VaultRecoveryContext::from_record(&record).expect("bound context");
    let recovered = restored.file_key(&master).expect("authenticated key");
    let expected = wrap_file_key(
        &master,
        &original,
        &context.vault_id,
        &context.package_id,
        1,
        2,
        &mut AeadUsageRegistry::new(),
    )
    .expect("expected wrap");
    let actual = wrap_file_key(
        &master,
        &recovered,
        &context.vault_id,
        &context.package_id,
        1,
        2,
        &mut AeadUsageRegistry::new(),
    )
    .expect("restored wrap");
    assert_eq!(
        actual, expected,
        "restored key has identical cryptographic behavior"
    );
    let wrong = generate_vault_master_key(&mut OsRandom).expect("another synthetic key");
    assert!(matches!(
        restored.file_key(&wrong),
        Err(RecoveryContextError::Authentication)
    ));
    for field in 0..5 {
        let mut other = record.clone();
        match field {
            0 => other.account_id += 1,
            1 => other.id += 1,
            2 => other.chat_id += 1,
            3 => other.package_id[0] ^= 1,
            _ => other.direction = teleark_storage::VaultJobDirection::Download,
        };
        assert_eq!(
            VaultRecoveryContext::from_record(&other),
            Err(RecoveryContextError::Corrupt)
        );
    }
}

#[cfg(unix)]
#[test]
fn version_one_context_matches_the_frozen_binary_fixture() {
    let mut context = fixture();
    if let VaultRecoveryDirection::Upload { source, .. } = &mut context.direction {
        *source = "/tmp/teleark-context-fixture".into();
    }
    let bytes = include_bytes!("fixtures/upload-context-v1.bin");
    assert_eq!(context.encode().expect("encode").as_slice(), bytes);
    assert_eq!(
        VaultRecoveryContext::decode(bytes).expect("historical fixture"),
        context
    );
}

#[test]
fn reserved_header_recovers_identical_ciphertext_and_rejects_invalid_bytes() {
    use teleark_crypto::{
        AeadUsageRegistry, FileKey, PartHeader, PartInstanceId, PartLimits, encrypt_part,
    };
    let reservation = VaultPartRecovery {
        header: PartHeader::new(
            [2; 16],
            PartInstanceId([7; 16]),
            0,
            1,
            4,
            0,
            4,
            8 * 1024 * 1024,
            PartLimits::default(),
        )
        .expect("valid header"),
        plaintext_blake3: *blake3::hash(b"test").as_bytes(),
        publication_random_id: 12345,
    };
    let bytes = reservation.encode().expect("encoded reservation");
    let restored = VaultPartRecovery::decode(&bytes).expect("restored reservation");
    assert_eq!(restored, reservation);
    let key = FileKey::from_bytes([88; 32]);
    let encrypt = |header: &PartHeader| {
        let mut out = Vec::new();
        encrypt_part(
            &mut std::io::Cursor::new(b"test"),
            &mut out,
            header,
            &key,
            PartLimits::default(),
            &mut AeadUsageRegistry::new(),
        )
        .expect("encrypt synthetic fixed input");
        out
    };
    assert_eq!(encrypt(&reservation.header), encrypt(&restored.header));
    for n in 0..bytes.len() {
        assert!(VaultPartRecovery::decode(&bytes[..n]).is_err());
    }
    for n in 0..bytes.len() {
        let mut changed = bytes.clone();
        changed[n] ^= 1;
        assert!(VaultPartRecovery::decode(&changed).is_err());
    }
}

#[test]
fn resumed_source_requires_unchanged_identity_path_and_full_content() {
    let context = fixture();
    let VaultRecoveryDirection::Upload {
        source,
        identity,
        source_blake3,
    } = &context.direction
    else {
        panic!("upload fixture")
    };
    assert_eq!(
        context.verify_upload_source(source, *identity, *source_blake3),
        Ok(())
    );
    let mut changed_identity = *identity;
    changed_identity.revision += 1;
    assert_eq!(
        context.verify_upload_source(source, changed_identity, *source_blake3),
        Err(teleark_core::TransferError::SourceChanged)
    );
    let mut changed_hash = *source_blake3;
    changed_hash[0] ^= 1;
    assert_eq!(
        context.verify_upload_source(source, *identity, changed_hash),
        Err(teleark_core::TransferError::SourceChanged)
    );
    assert_eq!(
        context.verify_upload_source(
            &source.with_file_name("replacement"),
            *identity,
            *source_blake3
        ),
        Err(teleark_core::TransferError::SourceChanged)
    );
    assert_eq!(
        VaultRecoveryContext::decode(
            &context
                .encode()
                .expect("original context remains encodable")
        )
        .expect("unchanged context"),
        context
    );
}
