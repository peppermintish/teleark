use hkdf::Hkdf;
use sha2::Sha256;
use zeroize::Zeroizing;

use crate::CryptoError;

pub(crate) const PASSWORD_WRAP_DOMAIN: &[u8] = b"teleark/vault-master/password-wrap/v1";
pub(crate) const RECOVERY_WRAP_DOMAIN: &[u8] = b"teleark/vault-master/recovery-wrap/v1";
pub(crate) const FILE_KEY_WRAP_DOMAIN: &[u8] = b"teleark/file-key-wrap/v1";
pub(crate) const CONTENT_KEY_DOMAIN: &[u8] = b"teleark/content-part-key/v1";
pub(crate) const MANIFEST_KEY_DOMAIN: &[u8] = b"teleark/manifest-metadata/v1";

fn derive(
    input_key_material: &[u8],
    salt: &[u8],
    info: &[&[u8]],
) -> Result<Zeroizing<[u8; 32]>, CryptoError> {
    let hkdf = Hkdf::<Sha256>::new(Some(salt), input_key_material);
    let mut output = Zeroizing::new([0_u8; 32]);
    hkdf.expand_multi_info(info, output.as_mut())
        .map_err(|_| CryptoError::InvalidField { field: "HKDF info" })?;
    Ok(output)
}

pub(crate) fn recovery_kek(
    recovery_key: &[u8; 32],
    vault_id: &[u8; 16],
) -> Result<Zeroizing<[u8; 32]>, CryptoError> {
    derive(recovery_key, vault_id, &[RECOVERY_WRAP_DOMAIN])
}

pub(crate) fn package_wrap_key(
    master_key: &[u8; 32],
    package_id: &[u8; 16],
    master_key_generation: u32,
    wrap_generation: u32,
) -> Result<Zeroizing<[u8; 32]>, CryptoError> {
    let master_generation = master_key_generation.to_be_bytes();
    let wrap_generation = wrap_generation.to_be_bytes();
    derive(
        master_key,
        package_id,
        &[FILE_KEY_WRAP_DOMAIN, &master_generation, &wrap_generation],
    )
}

pub(crate) fn part_content_key(
    file_key: &[u8; 32],
    package_id: &[u8; 16],
    part_index: u32,
    part_instance_id: &[u8; 16],
) -> Result<Zeroizing<[u8; 32]>, CryptoError> {
    let part_index = part_index.to_be_bytes();
    derive(
        file_key,
        package_id,
        &[CONTENT_KEY_DOMAIN, &part_index, part_instance_id],
    )
}

pub(crate) fn manifest_key(
    file_key: &[u8; 32],
    package_id: &[u8; 16],
    manifest_generation: u32,
) -> Result<Zeroizing<[u8; 32]>, CryptoError> {
    let generation = manifest_generation.to_be_bytes();
    derive(file_key, package_id, &[MANIFEST_KEY_DOMAIN, &generation])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn to_hex(bytes: &[u8]) -> String {
        let mut output = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            use std::fmt::Write as _;
            let _ = write!(output, "{byte:02x}");
        }
        output
    }

    #[test]
    fn domains_and_generation_change_derived_keys() {
        let master = [7_u8; 32];
        let package = [9_u8; 16];
        let first = package_wrap_key(&master, &package, 1, 1);
        let second = package_wrap_key(&master, &package, 1, 2);
        let first = match first {
            Ok(value) => value,
            Err(error) => panic!("unexpected derivation failure: {error}"),
        };
        let second = match second {
            Ok(value) => value,
            Err(error) => panic!("unexpected derivation failure: {error}"),
        };
        assert_ne!(first.as_ref(), second.as_ref());
        assert_eq!(to_hex(first.as_ref()).len(), 64);
    }
}
