use std::fmt;

use teleark_core::{ApplicationError, ApplicationErrorKind};
use zeroize::Zeroizing;

pub(crate) const TELEGRAM_API_HASH_LENGTH: usize = 32;
const DISTRIBUTION_API_ID: Option<&str> = option_env!("TELEARK_DISTRIBUTION_TELEGRAM_API_ID");
const DISTRIBUTION_API_HASH: Option<&str> = option_env!("TELEARK_DISTRIBUTION_TELEGRAM_API_HASH");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TelegramCredentialSource {
    User,
    Distribution,
}

pub(crate) struct ActiveTelegramCredentials {
    pub(crate) api_id: i32,
    pub(crate) api_hash: Zeroizing<String>,
}

impl fmt::Debug for ActiveTelegramCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ActiveTelegramCredentials")
            .field("api_id", &self.api_id)
            .field("api_hash", &"[REDACTED]")
            .finish()
    }
}

pub(crate) fn validate_api_hash(secret: &str) -> Result<(), ApplicationError> {
    if secret.len() == TELEGRAM_API_HASH_LENGTH
        && secret.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        Ok(())
    } else {
        Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest))
    }
}

pub(crate) fn distribution_credentials()
-> Result<Option<ActiveTelegramCredentials>, ApplicationError> {
    parse_distribution_credentials(DISTRIBUTION_API_ID, DISTRIBUTION_API_HASH)
}

fn parse_distribution_credentials(
    api_id: Option<&str>,
    api_hash: Option<&str>,
) -> Result<Option<ActiveTelegramCredentials>, ApplicationError> {
    match (api_id, api_hash) {
        (None, None) | (Some(""), Some("")) => Ok(None),
        (Some(api_id), Some(api_hash)) => {
            let api_id = api_id
                .parse::<i32>()
                .ok()
                .filter(|api_id| *api_id > 0)
                .ok_or_else(|| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?;
            validate_api_hash(api_hash)?;
            Ok(Some(ActiveTelegramCredentials {
                api_id,
                api_hash: Zeroizing::new(api_hash.to_owned()),
            }))
        }
        _ => Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest)),
    }
}

pub(crate) const USER_PAIR_SERVICE: &str = "app.teleark.telegram-api.v1";

/// Fixed v1 credential codec: one version byte, positive little-endian i32 ID,
/// followed by exactly 32 ASCII hexadecimal bytes. Never serialized Rust layout.
pub(crate) fn encode_user_pair(
    api_id: i32,
    api_hash: &str,
) -> Result<Zeroizing<Vec<u8>>, ApplicationError> {
    if api_id <= 0 {
        return Err(ApplicationError::new(ApplicationErrorKind::InvalidRequest));
    }
    validate_api_hash(api_hash)?;
    let mut bytes = Zeroizing::new(Vec::with_capacity(37));
    bytes.push(1);
    bytes.extend_from_slice(&api_id.to_le_bytes());
    bytes.extend_from_slice(api_hash.as_bytes());
    Ok(bytes)
}

pub(crate) fn decode_user_pair(
    bytes: &[u8],
) -> Result<ActiveTelegramCredentials, ApplicationError> {
    let invalid = || ApplicationError::new(ApplicationErrorKind::Persistence);
    if bytes.len() != 37 || bytes[0] != 1 {
        return Err(invalid());
    }
    let api_id = i32::from_le_bytes(bytes[1..5].try_into().map_err(|_| invalid())?);
    let api_hash = std::str::from_utf8(&bytes[5..]).map_err(|_| invalid())?;
    if api_id <= 0 {
        return Err(invalid());
    }
    validate_api_hash(api_hash).map_err(|_| invalid())?;
    Ok(ActiveTelegramCredentials {
        api_id,
        api_hash: Zeroizing::new(api_hash.to_owned()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_pair_v1_is_explicit_bounded_and_rejects_future_or_corrupt_bytes() {
        let encoded = encode_user_pair(12345, "0123456789abcdef0123456789abcdef").expect("encode");
        assert_eq!(&encoded[..5], &[1, 0x39, 0x30, 0, 0]);
        assert_eq!(encoded.len(), 37);
        let decoded = decode_user_pair(&encoded).expect("decode");
        assert_eq!(decoded.api_id, 12345);
        let mut future = encoded.to_vec();
        future[0] = 2;
        assert!(decode_user_pair(&future).is_err());
        assert!(decode_user_pair(&encoded[..36]).is_err());
        let mut invalid_id = encoded.to_vec();
        invalid_id[1..5].fill(0);
        assert!(decode_user_pair(&invalid_id).is_err());
        let mut damaged = encoded.to_vec();
        damaged[5] = b'z';
        assert!(decode_user_pair(&damaged).is_err());
    }

    #[test]
    fn legacy_api_pair_migrates_without_leaving_hash_in_settings_and_clear_does_not_restore_it() {
        use crate::*;
        let dir = tempfile::tempdir().expect("fixture directory");
        let path = dir.path().join("library.sqlite3");
        let mut db = Database::open(&path).expect("database");
        db.set_settings(&[
            SettingRecord {
                key: TELEGRAM_API_ID_SETTING_KEY.into(),
                value: "12345".into(),
                updated_at_unix_ms: 1,
            },
            SettingRecord {
                key: TELEGRAM_API_HASH_SETTING_KEY.into(),
                value: "0123456789abcdef0123456789abcdef".into(),
                updated_at_unix_ms: 1,
            },
        ])
        .expect("legacy pair");
        drop(db);
        let library = DesktopLibrary::open_synthetic(&path).expect("library");
        assert_eq!(
            library
                .telegram_credentials_status()
                .expect("migrate")
                .expect("present")
                .api_id,
            12345
        );
        let db = Database::open(&path).expect("inspect");
        assert!(
            db.setting(TELEGRAM_API_HASH_SETTING_KEY)
                .expect("legacy hash")
                .is_none()
        );
        drop(db);
        library.clear_telegram_credentials().expect("clear");
        drop(library);
        let library = DesktopLibrary::open_synthetic(&path).expect("reopen");
        assert!(
            library
                .telegram_credentials_status()
                .expect("cleared")
                .is_none()
        );
    }

    #[test]
    fn api_hash_validation_accepts_only_the_documented_shape() {
        validate_api_hash("0123456789abcdef0123456789abcdef").expect("valid API hash");
        for invalid in ["", "short", "z123456789abcdef0123456789abcdef"] {
            assert_eq!(
                validate_api_hash(invalid)
                    .expect_err("reject malformed API hash")
                    .kind(),
                ApplicationErrorKind::InvalidRequest
            );
        }
    }

    #[test]
    fn active_credentials_never_reveal_the_hash_through_debug() {
        let credentials = ActiveTelegramCredentials {
            api_id: 12_345,
            api_hash: Zeroizing::new("0123456789abcdef0123456789abcdef".to_owned()),
        };
        let rendered = format!("{credentials:?}");
        assert!(rendered.contains("[REDACTED]"));
        assert!(!rendered.contains(credentials.api_hash.as_str()));
    }

    #[test]
    fn distribution_credentials_require_a_complete_valid_build_pair() {
        assert!(
            parse_distribution_credentials(None, None)
                .expect("empty distribution pair")
                .is_none()
        );
        for invalid in [
            parse_distribution_credentials(Some("12345"), None),
            parse_distribution_credentials(None, Some("0123456789abcdef0123456789abcdef")),
            parse_distribution_credentials(Some("0"), Some("0123456789abcdef0123456789abcdef")),
            parse_distribution_credentials(Some("12345"), Some("short")),
        ] {
            assert_eq!(
                invalid
                    .expect_err("reject incomplete or malformed distribution credentials")
                    .kind(),
                ApplicationErrorKind::InvalidRequest
            );
        }
        let configured =
            parse_distribution_credentials(Some("12345"), Some("0123456789abcdef0123456789abcdef"))
                .expect("valid distribution pair")
                .expect("configured distribution credentials");
        assert_eq!(configured.api_id, 12_345);
        assert!(!format!("{configured:?}").contains(configured.api_hash.as_str()));
    }
}
