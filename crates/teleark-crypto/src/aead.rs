use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce, Tag,
    aead::{AeadInOut, consts::U12},
};

use crate::CryptoError;

pub(crate) const ZERO_NONCE: [u8; 12] = [0_u8; 12];

pub(crate) fn encrypt_detached(
    key: &[u8],
    nonce_bytes: &[u8; 12],
    aad: &[u8],
    buffer: &mut [u8],
) -> Result<[u8; 16], CryptoError> {
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|_| CryptoError::InvalidField { field: "AES key" })?;
    let nonce =
        <&Nonce<U12>>::try_from(nonce_bytes.as_slice()).map_err(|_| CryptoError::InvalidField {
            field: "AES-GCM nonce",
        })?;
    let tag = cipher
        .encrypt_inout_detached(nonce, aad, buffer.into())
        .map_err(|_| CryptoError::AuthenticationFailed)?;
    tag.as_slice()
        .try_into()
        .map_err(|_| CryptoError::InvalidField {
            field: "AES-GCM tag",
        })
}

pub(crate) fn decrypt_detached(
    key: &[u8],
    nonce_bytes: &[u8; 12],
    aad: &[u8],
    buffer: &mut [u8],
    tag_bytes: &[u8; 16],
) -> Result<(), CryptoError> {
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|_| CryptoError::InvalidField { field: "AES key" })?;
    let nonce =
        <&Nonce<U12>>::try_from(nonce_bytes.as_slice()).map_err(|_| CryptoError::InvalidField {
            field: "AES-GCM nonce",
        })?;
    let tag = <&Tag>::try_from(tag_bytes.as_slice()).map_err(|_| CryptoError::InvalidField {
        field: "AES-GCM tag",
    })?;
    cipher
        .decrypt_inout_detached(nonce, aad, buffer.into(), tag)
        .map_err(|_| CryptoError::AuthenticationFailed)
}
