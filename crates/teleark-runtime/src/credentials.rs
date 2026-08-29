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

#[cfg(test)]
mod tests {
    use super::*;

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
