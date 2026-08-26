use argon2::{Algorithm, Argon2, Params, Version};
use zeroize::Zeroizing;

use crate::aead::{ZERO_NONCE, decrypt_detached, encrypt_detached};
use crate::kdf::{
    FILE_KEY_WRAP_DOMAIN, PASSWORD_WRAP_DOMAIN, RECOVERY_WRAP_DOMAIN, package_wrap_key,
    recovery_kek,
};
use crate::{
    AeadUsageRegistry, CRYPTO_SUITE_ID, CryptoError, FORMAT_MAJOR, FORMAT_MINOR, FileKey,
    FormatKind, Password, RandomSource, RecoveryKey, VaultMasterKey,
};

const PASSWORD_WRAP_MAGIC: &[u8; 8] = b"TARKPWK\0";
const RECOVERY_WRAP_MAGIC: &[u8; 8] = b"TARKRWK\0";
const ARGON2_VERSION_13: u32 = 0x13;
const MIN_SALT_LENGTH: usize = 16;
const MAX_SALT_LENGTH: usize = 64;
const MAX_ARGON_MEMORY_KIB: u32 = 1024 * 1024;
const MAX_ARGON_ITERATIONS: u32 = 64;
const MAX_ARGON_PARALLELISM: u32 = 64;

/// Reviewed parameters for a newly created password wrap.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Argon2Parameters {
    pub memory_kib: u32,
    pub iterations: u32,
    pub parallelism: u32,
}

impl Argon2Parameters {
    /// Conservative starting point; applications should benchmark and may use
    /// stronger values through [`Self::new`].
    pub const RECOMMENDED: Self = Self {
        memory_kib: 64 * 1024,
        iterations: 3,
        parallelism: 1,
    };

    /// Construct parameters suitable for creating a production wrap.
    pub fn new(memory_kib: u32, iterations: u32, parallelism: u32) -> Result<Self, CryptoError> {
        let value = Self {
            memory_kib,
            iterations,
            parallelism,
        };
        value.validate_parser_bounds()?;
        if memory_kib < 19 * 1024 || iterations < 2 {
            return Err(CryptoError::PasswordParametersRejected);
        }
        Ok(value)
    }

    fn validate_parser_bounds(self) -> Result<(), CryptoError> {
        if self.memory_kib > MAX_ARGON_MEMORY_KIB
            || self.iterations > MAX_ARGON_ITERATIONS
            || self.parallelism > MAX_ARGON_PARALLELISM
        {
            return Err(CryptoError::PasswordParametersRejected);
        }
        Params::new(self.memory_kib, self.iterations, self.parallelism, Some(32))
            .map_err(|_| CryptoError::PasswordParametersRejected)?;
        Ok(())
    }

    #[cfg(test)]
    const fn testing() -> Self {
        Self {
            memory_kib: 32,
            iterations: 1,
            parallelism: 1,
        }
    }
}

/// Immutable password-based Vault Master Key wrap record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PasswordWrap {
    pub vault_id: [u8; 16],
    pub wrap_generation: u32,
    pub parameters: Argon2Parameters,
    pub salt: Vec<u8>,
    pub ciphertext: [u8; 32],
    pub tag: [u8; 16],
}

/// Immutable Recovery Key-based Vault Master Key wrap record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryWrap {
    pub vault_id: [u8; 16],
    pub recovery_generation: u32,
    pub ciphertext: [u8; 32],
    pub tag: [u8; 16],
}

/// Immutable package File Key wrap stored in the manifest public header.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileKeyWrap {
    pub algorithm_id: u16,
    pub wrap_generation: u32,
    pub ciphertext: [u8; 32],
    pub tag: [u8; 16],
}

pub fn generate_vault_master_key(
    random: &mut impl RandomSource,
) -> Result<VaultMasterKey, CryptoError> {
    VaultMasterKey::generate(random)
}

pub fn generate_file_key(random: &mut impl RandomSource) -> Result<FileKey, CryptoError> {
    FileKey::generate(random)
}

pub fn generate_recovery_key(random: &mut impl RandomSource) -> Result<RecoveryKey, CryptoError> {
    RecoveryKey::generate(random)
}

fn password_aad(vault_id: &[u8; 16], generation: u32) -> Vec<u8> {
    let mut aad = Vec::with_capacity(PASSWORD_WRAP_DOMAIN.len() + 20);
    aad.extend_from_slice(PASSWORD_WRAP_DOMAIN);
    aad.extend_from_slice(vault_id);
    aad.extend_from_slice(&generation.to_be_bytes());
    aad
}

fn recovery_aad(vault_id: &[u8; 16], generation: u32) -> Vec<u8> {
    let mut aad = Vec::with_capacity(RECOVERY_WRAP_DOMAIN.len() + 20);
    aad.extend_from_slice(RECOVERY_WRAP_DOMAIN);
    aad.extend_from_slice(vault_id);
    aad.extend_from_slice(&generation.to_be_bytes());
    aad
}

fn file_key_aad(
    vault_id: &[u8; 16],
    package_id: &[u8; 16],
    master_key_generation: u32,
    wrap_generation: u32,
) -> Vec<u8> {
    let mut aad = Vec::with_capacity(FILE_KEY_WRAP_DOMAIN.len() + 40);
    aad.extend_from_slice(FILE_KEY_WRAP_DOMAIN);
    aad.extend_from_slice(vault_id);
    aad.extend_from_slice(package_id);
    aad.extend_from_slice(&master_key_generation.to_be_bytes());
    aad.extend_from_slice(&wrap_generation.to_be_bytes());
    aad
}

pub(crate) fn derive_password_kek(
    password: &Password,
    salt: &[u8],
    parameters: Argon2Parameters,
) -> Result<Zeroizing<[u8; 32]>, CryptoError> {
    if !(MIN_SALT_LENGTH..=MAX_SALT_LENGTH).contains(&salt.len()) {
        return Err(CryptoError::InvalidField {
            field: "password wrap salt",
        });
    }
    parameters.validate_parser_bounds()?;
    let params = Params::new(
        parameters.memory_kib,
        parameters.iterations,
        parameters.parallelism,
        Some(32),
    )
    .map_err(|_| CryptoError::PasswordParametersRejected)?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut output = Zeroizing::new([0_u8; 32]);
    argon2
        .hash_password_into(password.as_bytes(), salt, output.as_mut())
        .map_err(|_| CryptoError::PasswordParametersRejected)?;
    Ok(output)
}

pub fn wrap_master_key_with_password(
    master_key: &VaultMasterKey,
    password: &Password,
    vault_id: [u8; 16],
    wrap_generation: u32,
    random: &mut impl RandomSource,
    usage: &mut AeadUsageRegistry,
) -> Result<PasswordWrap, CryptoError> {
    wrap_master_key_with_password_parameters(
        master_key,
        password,
        vault_id,
        wrap_generation,
        Argon2Parameters::RECOMMENDED,
        random,
        usage,
    )
}

/// Create a password wrap with application-benchmarked production parameters.
pub fn wrap_master_key_with_password_parameters(
    master_key: &VaultMasterKey,
    password: &Password,
    vault_id: [u8; 16],
    wrap_generation: u32,
    parameters: Argon2Parameters,
    random: &mut impl RandomSource,
    usage: &mut AeadUsageRegistry,
) -> Result<PasswordWrap, CryptoError> {
    // Revalidate through the public production constructor policy.
    let parameters = Argon2Parameters::new(
        parameters.memory_kib,
        parameters.iterations,
        parameters.parallelism,
    )?;
    wrap_master_key_with_password_inner(
        master_key,
        password,
        vault_id,
        wrap_generation,
        parameters,
        random,
        usage,
    )
}

fn wrap_master_key_with_password_inner(
    master_key: &VaultMasterKey,
    password: &Password,
    vault_id: [u8; 16],
    wrap_generation: u32,
    parameters: Argon2Parameters,
    random: &mut impl RandomSource,
    usage: &mut AeadUsageRegistry,
) -> Result<PasswordWrap, CryptoError> {
    let mut salt = vec![0_u8; MIN_SALT_LENGTH];
    random.fill_bytes(&mut salt)?;
    let kek = derive_password_kek(password, &salt, parameters)?;
    usage.reserve_password_wrap(kek.as_ref(), vault_id, wrap_generation)?;
    let mut ciphertext = Zeroizing::new(*master_key.as_bytes());
    let aad = password_aad(&vault_id, wrap_generation);
    let tag = encrypt_detached(kek.as_ref(), &ZERO_NONCE, &aad, ciphertext.as_mut())?;
    Ok(PasswordWrap {
        vault_id,
        wrap_generation,
        parameters,
        salt,
        ciphertext: *ciphertext,
        tag,
    })
}

pub fn unwrap_master_key_with_password(
    record: &PasswordWrap,
    password: &Password,
) -> Result<VaultMasterKey, CryptoError> {
    let kek = derive_password_kek(password, &record.salt, record.parameters)?;
    let mut plaintext = Zeroizing::new(record.ciphertext);
    let aad = password_aad(&record.vault_id, record.wrap_generation);
    decrypt_detached(
        kek.as_ref(),
        &ZERO_NONCE,
        &aad,
        plaintext.as_mut(),
        &record.tag,
    )?;
    Ok(VaultMasterKey::from_bytes(*plaintext))
}

pub fn wrap_master_key_with_recovery(
    master_key: &VaultMasterKey,
    recovery_key: &RecoveryKey,
    vault_id: [u8; 16],
    recovery_generation: u32,
    usage: &mut AeadUsageRegistry,
) -> Result<RecoveryWrap, CryptoError> {
    let kek = recovery_kek(recovery_key.as_bytes(), &vault_id)?;
    usage.reserve_recovery_wrap(kek.as_ref(), vault_id, recovery_generation)?;
    let mut ciphertext = Zeroizing::new(*master_key.as_bytes());
    let aad = recovery_aad(&vault_id, recovery_generation);
    let tag = encrypt_detached(kek.as_ref(), &ZERO_NONCE, &aad, ciphertext.as_mut())?;
    Ok(RecoveryWrap {
        vault_id,
        recovery_generation,
        ciphertext: *ciphertext,
        tag,
    })
}

pub fn unwrap_master_key_with_recovery(
    record: &RecoveryWrap,
    recovery_key: &RecoveryKey,
) -> Result<VaultMasterKey, CryptoError> {
    let kek = recovery_kek(recovery_key.as_bytes(), &record.vault_id)?;
    let mut plaintext = Zeroizing::new(record.ciphertext);
    let aad = recovery_aad(&record.vault_id, record.recovery_generation);
    decrypt_detached(
        kek.as_ref(),
        &ZERO_NONCE,
        &aad,
        plaintext.as_mut(),
        &record.tag,
    )?;
    Ok(VaultMasterKey::from_bytes(*plaintext))
}

pub fn wrap_file_key(
    master_key: &VaultMasterKey,
    file_key: &FileKey,
    vault_id: &[u8; 16],
    package_id: &[u8; 16],
    master_key_generation: u32,
    wrap_generation: u32,
    usage: &mut AeadUsageRegistry,
) -> Result<FileKeyWrap, CryptoError> {
    let wrapping_key = package_wrap_key(
        master_key.as_bytes(),
        package_id,
        master_key_generation,
        wrap_generation,
    )?;
    usage.reserve_file_key_wrap(
        wrapping_key.as_ref(),
        *vault_id,
        *package_id,
        master_key_generation,
        wrap_generation,
    )?;
    let aad = file_key_aad(vault_id, package_id, master_key_generation, wrap_generation);
    let mut ciphertext = Zeroizing::new(*file_key.as_bytes());
    let tag = encrypt_detached(
        wrapping_key.as_ref(),
        &ZERO_NONCE,
        &aad,
        ciphertext.as_mut(),
    )?;
    Ok(FileKeyWrap {
        algorithm_id: CRYPTO_SUITE_ID,
        wrap_generation,
        ciphertext: *ciphertext,
        tag,
    })
}

pub fn unwrap_file_key(
    master_key: &VaultMasterKey,
    record: &FileKeyWrap,
    vault_id: &[u8; 16],
    package_id: &[u8; 16],
    master_key_generation: u32,
) -> Result<FileKey, CryptoError> {
    if record.algorithm_id != CRYPTO_SUITE_ID {
        return Err(CryptoError::UnsupportedAlgorithm {
            algorithm_id: record.algorithm_id,
        });
    }
    let wrapping_key = package_wrap_key(
        master_key.as_bytes(),
        package_id,
        master_key_generation,
        record.wrap_generation,
    )?;
    let aad = file_key_aad(
        vault_id,
        package_id,
        master_key_generation,
        record.wrap_generation,
    );
    let mut plaintext = Zeroizing::new(record.ciphertext);
    decrypt_detached(
        wrapping_key.as_ref(),
        &ZERO_NONCE,
        &aad,
        plaintext.as_mut(),
        &record.tag,
    )?;
    Ok(FileKey::from_bytes(*plaintext))
}

impl PasswordWrap {
    /// Explicit bounded binary codec for durable password-wrap records.
    pub fn encode(&self) -> Result<Vec<u8>, CryptoError> {
        if !(MIN_SALT_LENGTH..=MAX_SALT_LENGTH).contains(&self.salt.len()) {
            return Err(CryptoError::InvalidField {
                field: "password wrap salt",
            });
        }
        self.parameters.validate_parser_bounds()?;
        let salt_length =
            u16::try_from(self.salt.len()).map_err(|_| CryptoError::ArithmeticOverflow {
                field: "password wrap salt length",
            })?;
        let mut output = Vec::with_capacity(100 + self.salt.len());
        output.extend_from_slice(PASSWORD_WRAP_MAGIC);
        output.extend_from_slice(&FORMAT_MAJOR.to_be_bytes());
        output.extend_from_slice(&FORMAT_MINOR.to_be_bytes());
        output.extend_from_slice(&self.vault_id);
        output.extend_from_slice(&self.wrap_generation.to_be_bytes());
        output.extend_from_slice(&ARGON2_VERSION_13.to_be_bytes());
        output.extend_from_slice(&self.parameters.memory_kib.to_be_bytes());
        output.extend_from_slice(&self.parameters.iterations.to_be_bytes());
        output.extend_from_slice(&self.parameters.parallelism.to_be_bytes());
        output.extend_from_slice(&salt_length.to_be_bytes());
        output.extend_from_slice(&CRYPTO_SUITE_ID.to_be_bytes());
        output.extend_from_slice(&0_u32.to_be_bytes());
        output.extend_from_slice(&0_u32.to_be_bytes());
        output.extend_from_slice(&self.salt);
        output.extend_from_slice(&self.ciphertext);
        output.extend_from_slice(&self.tag);
        Ok(output)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, CryptoError> {
        const PREFIX_LENGTH: usize = 60;
        if bytes.len() < PREFIX_LENGTH + 48 {
            return Err(CryptoError::Truncated {
                context: "password wrap",
            });
        }
        if bytes.get(..8) != Some(PASSWORD_WRAP_MAGIC) {
            return Err(CryptoError::InvalidMagic {
                format: FormatKind::PasswordWrap,
            });
        }
        let major = read_u16(bytes, 8, "password wrap version")?;
        let minor = read_u16(bytes, 10, "password wrap version")?;
        if major != FORMAT_MAJOR || minor != FORMAT_MINOR {
            return Err(CryptoError::UnsupportedVersion {
                format: FormatKind::PasswordWrap,
                major,
                minor,
            });
        }
        let vault_id = read_array::<16>(bytes, 12, "password wrap vault ID")?;
        let wrap_generation = read_u32(bytes, 28, "password wrap generation")?;
        let argon_version = read_u32(bytes, 32, "Argon2 version")?;
        if argon_version != ARGON2_VERSION_13 {
            return Err(CryptoError::UnsupportedAlgorithm {
                algorithm_id: u16::try_from(argon_version).unwrap_or(u16::MAX),
            });
        }
        let parameters = Argon2Parameters {
            memory_kib: read_u32(bytes, 36, "Argon2 memory")?,
            iterations: read_u32(bytes, 40, "Argon2 iterations")?,
            parallelism: read_u32(bytes, 44, "Argon2 parallelism")?,
        };
        parameters.validate_parser_bounds()?;
        let salt_length = usize::from(read_u16(bytes, 48, "password salt length")?);
        if !(MIN_SALT_LENGTH..=MAX_SALT_LENGTH).contains(&salt_length) {
            return Err(CryptoError::InvalidField {
                field: "password wrap salt",
            });
        }
        let suite = read_u16(bytes, 50, "password wrap suite")?;
        if suite != CRYPTO_SUITE_ID {
            return Err(CryptoError::UnsupportedSuite { suite_id: suite });
        }
        let flags = read_u32(bytes, 52, "password wrap flags")?;
        if flags != 0 {
            return Err(CryptoError::UnknownFlags {
                field: "password wrap",
                flags,
            });
        }
        if read_u32(bytes, 56, "password wrap reserved")? != 0 {
            return Err(CryptoError::InvalidField {
                field: "password wrap reserved",
            });
        }
        let expected = PREFIX_LENGTH
            .checked_add(salt_length)
            .and_then(|length| length.checked_add(48))
            .ok_or(CryptoError::ArithmeticOverflow {
                field: "password wrap length",
            })?;
        if bytes.len() != expected {
            return if bytes.len() < expected {
                Err(CryptoError::Truncated {
                    context: "password wrap",
                })
            } else {
                Err(CryptoError::TrailingData {
                    context: "password wrap",
                })
            };
        }
        let salt_end = PREFIX_LENGTH + salt_length;
        Ok(Self {
            vault_id,
            wrap_generation,
            parameters,
            salt: bytes[PREFIX_LENGTH..salt_end].to_vec(),
            ciphertext: read_array(bytes, salt_end, "password wrap ciphertext")?,
            tag: read_array(bytes, salt_end + 32, "password wrap tag")?,
        })
    }
}

impl RecoveryWrap {
    /// Explicit fixed-width binary codec for durable recovery-wrap records.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut output = Vec::with_capacity(88);
        output.extend_from_slice(RECOVERY_WRAP_MAGIC);
        output.extend_from_slice(&FORMAT_MAJOR.to_be_bytes());
        output.extend_from_slice(&FORMAT_MINOR.to_be_bytes());
        output.extend_from_slice(&self.vault_id);
        output.extend_from_slice(&self.recovery_generation.to_be_bytes());
        output.extend_from_slice(&CRYPTO_SUITE_ID.to_be_bytes());
        output.extend_from_slice(&0_u16.to_be_bytes());
        output.extend_from_slice(&0_u32.to_be_bytes());
        output.extend_from_slice(&self.ciphertext);
        output.extend_from_slice(&self.tag);
        output
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, CryptoError> {
        const LENGTH: usize = 88;
        if bytes.len() != LENGTH {
            return if bytes.len() < LENGTH {
                Err(CryptoError::Truncated {
                    context: "recovery wrap",
                })
            } else {
                Err(CryptoError::TrailingData {
                    context: "recovery wrap",
                })
            };
        }
        if bytes.get(..8) != Some(RECOVERY_WRAP_MAGIC) {
            return Err(CryptoError::InvalidMagic {
                format: FormatKind::RecoveryWrap,
            });
        }
        let major = read_u16(bytes, 8, "recovery wrap version")?;
        let minor = read_u16(bytes, 10, "recovery wrap version")?;
        if major != FORMAT_MAJOR || minor != FORMAT_MINOR {
            return Err(CryptoError::UnsupportedVersion {
                format: FormatKind::RecoveryWrap,
                major,
                minor,
            });
        }
        let suite = read_u16(bytes, 32, "recovery wrap suite")?;
        if suite != CRYPTO_SUITE_ID {
            return Err(CryptoError::UnsupportedSuite { suite_id: suite });
        }
        if read_u16(bytes, 34, "recovery wrap reserved")? != 0
            || read_u32(bytes, 36, "recovery wrap flags")? != 0
        {
            return Err(CryptoError::InvalidField {
                field: "recovery wrap reserved",
            });
        }
        Ok(Self {
            vault_id: read_array(bytes, 12, "recovery wrap vault ID")?,
            recovery_generation: read_u32(bytes, 28, "recovery generation")?,
            ciphertext: read_array(bytes, 40, "recovery ciphertext")?,
            tag: read_array(bytes, 72, "recovery tag")?,
        })
    }
}

fn read_array<const N: usize>(
    bytes: &[u8],
    offset: usize,
    field: &'static str,
) -> Result<[u8; N], CryptoError> {
    bytes
        .get(offset..offset.saturating_add(N))
        .and_then(|slice| slice.try_into().ok())
        .ok_or(CryptoError::Truncated { context: field })
}

fn read_u16(bytes: &[u8], offset: usize, field: &'static str) -> Result<u16, CryptoError> {
    Ok(u16::from_be_bytes(read_array(bytes, offset, field)?))
}

fn read_u32(bytes: &[u8], offset: usize, field: &'static str) -> Result<u32, CryptoError> {
    Ok(u32::from_be_bytes(read_array(bytes, offset, field)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DeterministicRandom;

    fn password() -> Password {
        match Password::new(b"correct horse battery staple".to_vec()) {
            Ok(value) => value,
            Err(error) => panic!("test password rejected: {error}"),
        }
    }

    fn test_password_wrap(
        master: &VaultMasterKey,
        password: &Password,
        rng: &mut impl RandomSource,
    ) -> PasswordWrap {
        let mut usage = AeadUsageRegistry::new();
        match wrap_master_key_with_password_inner(
            master,
            password,
            [3; 16],
            7,
            Argon2Parameters::testing(),
            rng,
            &mut usage,
        ) {
            Ok(value) => value,
            Err(error) => panic!("test wrap failed: {error}"),
        }
    }

    #[test]
    fn password_wrap_roundtrip_wrong_password_and_tamper() {
        let master = VaultMasterKey::from_bytes([0x11; 32]);
        let password = password();
        let mut rng = DeterministicRandom::new(9);
        let record = test_password_wrap(&master, &password, &mut rng);
        let encoded = match record.encode() {
            Ok(value) => value,
            Err(error) => panic!("encode failed: {error}"),
        };
        let decoded = match PasswordWrap::decode(&encoded) {
            Ok(value) => value,
            Err(error) => panic!("decode failed: {error}"),
        };
        assert!(unwrap_master_key_with_password(&decoded, &password).is_ok());

        let wrong = match Password::new(b"incorrect".to_vec()) {
            Ok(value) => value,
            Err(error) => panic!("test password rejected: {error}"),
        };
        assert!(matches!(
            unwrap_master_key_with_password(&decoded, &wrong),
            Err(CryptoError::AuthenticationFailed)
        ));

        let mut tampered = decoded;
        tampered.tag[0] ^= 1;
        assert!(matches!(
            unwrap_master_key_with_password(&tampered, &password),
            Err(CryptoError::AuthenticationFailed)
        ));
    }

    #[test]
    fn recovery_and_file_key_wrap_reject_wrong_keys() {
        let master = VaultMasterKey::from_bytes([0x22; 32]);
        let recovery = RecoveryKey::from_bytes([0x33; 32]);
        let mut usage = AeadUsageRegistry::new();
        let recovery_record =
            match wrap_master_key_with_recovery(&master, &recovery, [4; 16], 2, &mut usage) {
                Ok(value) => value,
                Err(error) => panic!("recovery wrap failed: {error}"),
            };
        let encoded = recovery_record.encode();
        let decoded = match RecoveryWrap::decode(&encoded) {
            Ok(value) => value,
            Err(error) => panic!("recovery decode failed: {error}"),
        };
        assert!(unwrap_master_key_with_recovery(&decoded, &recovery).is_ok());
        assert!(matches!(
            unwrap_master_key_with_recovery(&decoded, &RecoveryKey::from_bytes([0x34; 32])),
            Err(CryptoError::AuthenticationFailed)
        ));

        let file = FileKey::from_bytes([0x44; 32]);
        let wrapped = match wrap_file_key(&master, &file, &[4; 16], &[5; 16], 3, 9, &mut usage) {
            Ok(value) => value,
            Err(error) => panic!("file wrap failed: {error}"),
        };
        assert!(unwrap_file_key(&master, &wrapped, &[4; 16], &[5; 16], 3).is_ok());
        assert!(matches!(
            unwrap_file_key(
                &VaultMasterKey::from_bytes([0x23; 32]),
                &wrapped,
                &[4; 16],
                &[5; 16],
                3
            ),
            Err(CryptoError::AuthenticationFailed)
        ));
    }

    #[test]
    fn wrap_codecs_reject_trailing_and_reserved_data() {
        let master = VaultMasterKey::from_bytes([1; 32]);
        let mut rng = DeterministicRandom::new(11);
        let record = test_password_wrap(&master, &password(), &mut rng);
        let mut encoded = match record.encode() {
            Ok(value) => value,
            Err(error) => panic!("encode failed: {error}"),
        };
        encoded.push(0);
        assert!(matches!(
            PasswordWrap::decode(&encoded),
            Err(CryptoError::TrailingData { .. })
        ));

        let recovery = match wrap_master_key_with_recovery(
            &master,
            &RecoveryKey::from_bytes([2; 32]),
            [3; 16],
            1,
            &mut AeadUsageRegistry::new(),
        ) {
            Ok(value) => value,
            Err(error) => panic!("recovery wrap failed: {error}"),
        };
        let mut encoded = recovery.encode();
        encoded[39] = 1;
        assert!(RecoveryWrap::decode(&encoded).is_err());
    }

    #[test]
    fn wrap_writers_and_hydration_reject_reused_encryption_identities() {
        let master = VaultMasterKey::from_bytes([0x61; 32]);
        let recovery = RecoveryKey::from_bytes([0x62; 32]);
        let vault = [0x63; 16];
        let package = [0x64; 16];
        let mut usage = AeadUsageRegistry::new();

        assert!(wrap_master_key_with_recovery(&master, &recovery, vault, 1, &mut usage).is_ok());
        assert_eq!(
            wrap_master_key_with_recovery(&master, &recovery, vault, 2, &mut usage),
            Err(CryptoError::AeadIdentityAlreadyUsed)
        );
        assert_eq!(
            wrap_master_key_with_recovery(
                &master,
                &RecoveryKey::from_bytes([0x69; 32]),
                vault,
                1,
                &mut usage,
            ),
            Err(CryptoError::AeadIdentityAlreadyUsed)
        );

        assert!(
            wrap_file_key(
                &master,
                &FileKey::from_bytes([0x65; 32]),
                &vault,
                &package,
                3,
                4,
                &mut usage,
            )
            .is_ok()
        );
        assert_eq!(
            wrap_file_key(
                &master,
                &FileKey::from_bytes([0x66; 32]),
                &[0x67; 16],
                &package,
                3,
                4,
                &mut usage,
            ),
            Err(CryptoError::AeadIdentityAlreadyUsed)
        );
        assert_eq!(
            wrap_file_key(
                &VaultMasterKey::from_bytes([0x6a; 32]),
                &FileKey::from_bytes([0x6b; 32]),
                &vault,
                &package,
                3,
                4,
                &mut usage,
            ),
            Err(CryptoError::AeadIdentityAlreadyUsed)
        );
        assert!(
            wrap_file_key(
                &master,
                &FileKey::from_bytes([0x65; 32]),
                &vault,
                &package,
                3,
                5,
                &mut usage,
            )
            .is_ok()
        );

        let mut restored = AeadUsageRegistry::new();
        assert!(
            restored
                .reserve_existing_recovery_wrap(&recovery, &vault, 1)
                .is_ok()
        );
        assert_eq!(
            wrap_master_key_with_recovery(&master, &recovery, vault, 99, &mut restored),
            Err(CryptoError::AeadIdentityAlreadyUsed)
        );
        assert!(
            restored
                .reserve_existing_file_key_wrap(&master, &vault, &package, 3, 4)
                .is_ok()
        );
        assert_eq!(
            wrap_file_key(
                &master,
                &FileKey::from_bytes([0x68; 32]),
                &vault,
                &package,
                3,
                4,
                &mut restored,
            ),
            Err(CryptoError::AeadIdentityAlreadyUsed)
        );
    }

    #[test]
    fn password_wrap_hydration_rejects_a_repeated_salt_identity() {
        let master = VaultMasterKey::from_bytes([0x71; 32]);
        let password = password();
        let mut first_rng = DeterministicRandom::new(42);
        let record = test_password_wrap(&master, &password, &mut first_rng);
        let mut restored = AeadUsageRegistry::new();
        assert!(
            restored
                .reserve_existing_password_wrap(&record, &password)
                .is_ok()
        );

        let mut repeated_rng = DeterministicRandom::new(42);
        assert_eq!(
            wrap_master_key_with_password_inner(
                &master,
                &password,
                [3; 16],
                8,
                Argon2Parameters::testing(),
                &mut repeated_rng,
                &mut restored,
            ),
            Err(CryptoError::AeadIdentityAlreadyUsed)
        );

        let mut semantic_usage = AeadUsageRegistry::new();
        let mut unique_rng = DeterministicRandom::new(43);
        assert!(
            wrap_master_key_with_password_inner(
                &master,
                &password,
                [3; 16],
                7,
                Argon2Parameters::testing(),
                &mut unique_rng,
                &mut semantic_usage,
            )
            .is_ok()
        );
        let mut different_salt_rng = DeterministicRandom::new(44);
        assert_eq!(
            wrap_master_key_with_password_inner(
                &master,
                &password,
                [3; 16],
                7,
                Argon2Parameters::testing(),
                &mut different_salt_rng,
                &mut semantic_usage,
            ),
            Err(CryptoError::AeadIdentityAlreadyUsed)
        );
    }

    #[test]
    fn candidate_vectors_match_fixtures() {
        let master = VaultMasterKey::from_bytes([0x11; 32]);
        let password = password();
        let mut rng = DeterministicRandom::new(9);
        let password_record = test_password_wrap(&master, &password, &mut rng);
        let password_bytes = match password_record.encode() {
            Ok(value) => value,
            Err(error) => panic!("password vector encode failed: {error}"),
        };
        let recovery_record = match wrap_master_key_with_recovery(
            &master,
            &RecoveryKey::from_bytes([0x33; 32]),
            [4; 16],
            2,
            &mut AeadUsageRegistry::new(),
        ) {
            Ok(value) => value,
            Err(error) => panic!("recovery vector failed: {error}"),
        };
        let file_record = match wrap_file_key(
            &master,
            &FileKey::from_bytes([0x44; 32]),
            &[4; 16],
            &[5; 16],
            3,
            9,
            &mut AeadUsageRegistry::new(),
        ) {
            Ok(value) => value,
            Err(error) => panic!("file vector failed: {error}"),
        };
        let fixture = include_str!("../tests/vectors/crypto_v1/key_wraps.txt");
        assert_eq!(
            hex(&password_bytes),
            fixture_value(fixture, "password_wrap_hex")
        );
        assert_eq!(
            hex(&recovery_record.encode()),
            fixture_value(fixture, "recovery_wrap_hex")
        );
        assert_eq!(
            hex(&file_record.ciphertext),
            fixture_value(fixture, "file_key_ciphertext")
        );
        assert_eq!(
            hex(&file_record.tag),
            fixture_value(fixture, "file_key_tag")
        );
    }

    fn hex(bytes: &[u8]) -> String {
        let mut value = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            use std::fmt::Write as _;
            if write!(value, "{byte:02x}").is_err() {
                panic!("writing to a String unexpectedly failed");
            }
        }
        value
    }

    fn fixture_value<'a>(fixture: &'a str, key: &str) -> &'a str {
        fixture
            .lines()
            .find_map(|line| {
                line.strip_prefix(key)
                    .and_then(|rest| rest.strip_prefix('='))
            })
            .unwrap_or_else(|| panic!("fixture key missing: {key}"))
    }
}
