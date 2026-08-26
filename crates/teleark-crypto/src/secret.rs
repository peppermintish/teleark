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
}
