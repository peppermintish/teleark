//! Authenticated, versioned proof that a Vault key belongs to a Telegram channel.
//! The document is public; possession of it never grants access to the key.

use zeroize::Zeroizing;

use crate::VaultMasterKey;

const MAGIC: &[u8; 8] = b"TARKKP01";
const DOMAIN: &str = "teleark/channel-key-proof/v1";
const LENGTH: usize = 8 + 8 + 8 + 16 + 32;

/// The exact v1 wire representation: magic, account, channel, vault ID, MAC.
#[must_use]
pub fn seal_channel_key_proof(
    master: &VaultMasterKey,
    vault_id: [u8; 16],
    account: i64,
    channel: i64,
) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(LENGTH);
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&account.to_be_bytes());
    bytes.extend_from_slice(&channel.to_be_bytes());
    bytes.extend_from_slice(&vault_id);
    let key = Zeroizing::new(blake3::derive_key(DOMAIN, master.as_bytes()));
    bytes.extend_from_slice(blake3::keyed_hash(&key, &bytes).as_bytes());
    bytes
}

/// Rejects different scopes, altered bytes, unknown versions and wrong keys.
#[must_use]
pub fn open_channel_key_proof(
    bytes: &[u8],
    master: &VaultMasterKey,
    vault_id: [u8; 16],
    account: i64,
    channel: i64,
) -> bool {
    if account <= 0
        || channel <= 0
        || bytes.len() != LENGTH
        || &bytes[..8] != MAGIC
        || bytes[8..16] != account.to_be_bytes()
        || bytes[16..24] != channel.to_be_bytes()
        || bytes[24..40] != vault_id
    {
        return false;
    }
    let key = Zeroizing::new(blake3::derive_key(DOMAIN, master.as_bytes()));
    let expected = blake3::keyed_hash(&key, &bytes[..40]);
    let mut difference = 0_u8;
    for (&actual, &expected) in bytes[40..].iter().zip(expected.as_bytes()) {
        difference |= actual ^ expected;
    }
    difference == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proof_authenticates_exact_key_and_channel() {
        let key = VaultMasterKey::from_bytes([7; 32]);
        let vault_id = [9; 16];
        let proof = seal_channel_key_proof(&key, vault_id, 7, 11);
        let vector = "5441524b4b5030310000000000000007000000000000000b09090909090909090909090909090909b6f2b2d48cea757d66a921be99a3ad5180f55e4ce6147dcd5d4ee6764b19ef40";
        assert_eq!(
            proof
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>(),
            vector
        );
        assert!(open_channel_key_proof(&proof, &key, vault_id, 7, 11));
        assert!(!open_channel_key_proof(&proof, &key, vault_id, 8, 11));
        assert!(!open_channel_key_proof(&proof, &key, vault_id, 7, 12));
        assert!(!open_channel_key_proof(&proof, &key, [8; 16], 7, 11));
        assert!(!open_channel_key_proof(
            &proof,
            &VaultMasterKey::from_bytes([8; 32]),
            vault_id,
            7,
            11
        ));
        let mut tampered = proof.clone();
        tampered[71] ^= 1;
        assert!(!open_channel_key_proof(&tampered, &key, vault_id, 7, 11));
        assert!(!open_channel_key_proof(&proof[..71], &key, vault_id, 7, 11));
        assert!(!open_channel_key_proof(&proof, &key, vault_id, 0, 11));
        assert!(!open_channel_key_proof(&proof, &key, vault_id, 7, -11));
    }
}
