use std::fmt;

use zeroize::Zeroizing;

use crate::{CryptoError, RandomSource};

const KEY_LENGTH: usize = 32;
const MAX_PASSWORD_BYTES: usize = 1024;

macro_rules! secret_key_type {
    ($name:ident) => {
        pub struct $name(Zeroizing<[u8; KEY_LENGTH]>);

        impl $name {
            #[must_use]
            pub fn from_bytes(bytes: [u8; KEY_LENGTH]) -> Self {
                Self(Zeroizing::new(bytes))
            }

            pub(crate) fn generate(random: &mut impl RandomSource) -> Result<Self, CryptoError> {
                let mut bytes = [0_u8; KEY_LENGTH];
                random.fill_bytes(&mut bytes)?;
                Ok(Self::from_bytes(bytes))
            }

            pub(crate) fn as_bytes(&self) -> &[u8; KEY_LENGTH] {
                &self.0
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(concat!(stringify!($name), "([REDACTED])"))
            }
        }
    };
}

secret_key_type!(VaultMasterKey);
secret_key_type!(FileKey);
secret_key_type!(RecoveryKey);

const RECOVERY_KEY_PREFIX: &str = "TARK-RK1-";
const RECOVERY_KEY_DOMAIN: &[u8] = b"teleark/recovery-key/text/v1";

impl RecoveryKey {
    /// Encodes the secret using the exact, checksummed version-1 recovery text
    /// format. The caller must treat the returned string as secret material.
    #[must_use]
    pub fn expose_text(&self) -> Zeroizing<String> {
        let mut output = String::with_capacity(82);
        output.push_str(RECOVERY_KEY_PREFIX);
        push_upper_hex(&mut output, self.as_bytes());
        output.push('-');
        let checksum = recovery_checksum(self.as_bytes());
        push_upper_hex(&mut output, &checksum);
        Zeroizing::new(output)
    }

    /// Parses only the canonical version-1 representation; whitespace,
    /// case-folding, and punctuation are deliberately not normalized.
    pub fn from_text(value: &str) -> Result<Self, CryptoError> {
        if value.len() != 82 || !value.starts_with(RECOVERY_KEY_PREFIX) {
            return Err(CryptoError::InvalidField {
                field: "recovery key text",
            });
        }
        let key_hex = value.get(9..73).ok_or(CryptoError::InvalidField {
            field: "recovery key text",
        })?;
        if value.as_bytes().get(73) != Some(&b'-') {
            return Err(CryptoError::InvalidField {
                field: "recovery key text",
            });
        }
        let checksum_hex = value.get(74..82).ok_or(CryptoError::InvalidField {
            field: "recovery key text",
        })?;
        let key = decode_upper_hex::<32>(key_hex)?;
        let checksum = decode_upper_hex::<4>(checksum_hex)?;
        if recovery_checksum(&key) != checksum {
            return Err(CryptoError::InvalidField {
                field: "recovery key checksum",
            });
        }
        Ok(Self::from_bytes(key))
    }
}

fn recovery_checksum(key: &[u8; 32]) -> [u8; 4] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(RECOVERY_KEY_DOMAIN);
    hasher.update(key);
    let mut result = [0_u8; 4];
    result.copy_from_slice(&hasher.finalize().as_bytes()[..4]);
    result
}

fn push_upper_hex(output: &mut String, bytes: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for byte in bytes {
        output.push(char::from(HEX[(byte >> 4) as usize]));
        output.push(char::from(HEX[(byte & 0x0f) as usize]));
    }
}

fn decode_upper_hex<const N: usize>(value: &str) -> Result<[u8; N], CryptoError> {
    if value.len() != N.saturating_mul(2) {
        return Err(CryptoError::InvalidField {
            field: "recovery key text",
        });
    }
    let mut output = [0_u8; N];
    let (pairs, remainder) = value.as_bytes().as_chunks::<2>();
    if !remainder.is_empty() {
        return Err(CryptoError::InvalidField {
            field: "recovery key text",
        });
    }
    for (index, pair) in pairs.iter().enumerate() {
        let high = upper_hex_nibble(pair[0])?;
        let low = upper_hex_nibble(pair[1])?;
        output[index] = (high << 4) | low;
    }
    Ok(output)
}

fn upper_hex_nibble(value: u8) -> Result<u8, CryptoError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err(CryptoError::InvalidField {
            field: "recovery key text",
        }),
    }
}

/// Password bytes retained only for the duration of an unlock/wrap operation.
/// The bytes are never normalized implicitly and are zeroized on drop.
pub struct Password(Zeroizing<Vec<u8>>);

impl Password {
    pub fn new(bytes: impl Into<Vec<u8>>) -> Result<Self, CryptoError> {
        let bytes = bytes.into();
        if bytes.is_empty() || bytes.len() > MAX_PASSWORD_BYTES {
            return Err(CryptoError::InvalidField { field: "password" });
        }
        Ok(Self(Zeroizing::new(bytes)))
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        self.0.as_slice()
    }
}

impl fmt::Debug for Password {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Password([REDACTED])")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_debug_is_redacted() {
        let key = FileKey::from_bytes([0x5a; 32]);
        let password = Password::new(b"hunter2".to_vec());
        assert_eq!(format!("{key:?}"), "FileKey([REDACTED])");
        let password = match password {
            Ok(value) => value,
            Err(error) => panic!("test password rejected: {error}"),
        };
        assert_eq!(format!("{password:?}"), "Password([REDACTED])");
    }

    #[test]
    fn password_bounds_are_enforced() {
        assert!(Password::new(Vec::<u8>::new()).is_err());
        assert!(Password::new(vec![0; MAX_PASSWORD_BYTES + 1]).is_err());
        assert!(Password::new(vec![0; MAX_PASSWORD_BYTES]).is_ok());
    }

    #[test]
    fn recovery_text_is_canonical_checksummed_and_redacted_from_debug() {
        let key = RecoveryKey::from_bytes([0x5a; 32]);
        let text = key.expose_text();
        assert_eq!(
            text.as_str(),
            "TARK-RK1-5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A-8AB6868B"
        );
        assert_eq!(text.len(), 82);
        assert!(RecoveryKey::from_text(&text).is_ok());
        let mut changed = text.to_string();
        let replacement = if changed.ends_with('0') { "1" } else { "0" };
        changed.replace_range(81..82, replacement);
        assert!(RecoveryKey::from_text(&changed).is_err());
        assert!(RecoveryKey::from_text(&text.to_ascii_lowercase()).is_err());
        assert_eq!(format!("{key:?}"), "RecoveryKey([REDACTED])");
    }
}
