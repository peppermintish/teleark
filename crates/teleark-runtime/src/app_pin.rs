//! Application access PIN. Independent of Telegram sessions and Vault key leases.
//! The versioned verifier is an authenticated wrap of a disposable random key;
//! it never contains a PIN, a Telegram credential, or a file encryption key.
use crate::{DesktopLibrary, StorageRequest, map_storage_error};
use teleark_core::{ApplicationError, ApplicationErrorKind};
use teleark_crypto::{
    AeadUsageRegistry, OsRandom, Password, PasswordWrap, RandomSource, generate_vault_master_key,
    unwrap_master_key_with_password, wrap_master_key_with_password,
};
use teleark_storage::SettingRecord;
use zeroize::Zeroizing;

pub(crate) const SETTING_KEY: &str = "application.pin";

#[derive(Clone)]
pub struct AppPinRecord(String);

fn invalid() -> ApplicationError {
    ApplicationError::new(ApplicationErrorKind::InvalidRequest)
}

impl AppPinRecord {
    /// Expensive Argon2 work: call only on a retained background worker.
    pub fn create(pin: String) -> Result<Self, ApplicationError> {
        let pin = Zeroizing::new(pin);
        if !(6..=12).contains(&pin.len()) || !pin.bytes().all(|b| b.is_ascii_digit()) {
            return Err(invalid());
        }
        let password = Password::new(pin.as_bytes().to_vec()).map_err(|_| invalid())?;
        let mut random = OsRandom;
        let mut domain = [0; 16];
        random.fill_bytes(&mut domain).map_err(|_| invalid())?;
        let key = generate_vault_master_key(&mut random).map_err(|_| invalid())?;
        let wrap = wrap_master_key_with_password(
            &key,
            &password,
            domain,
            1,
            &mut random,
            &mut AeadUsageRegistry::new(),
        )
        .map_err(|_| invalid())?;
        Ok(Self(
            serde_json::json!({"version": 1, "verifier": wrap.encode().map_err(|_| invalid())?})
                .to_string(),
        ))
    }

    pub(crate) fn decode(value: String) -> Result<Self, ApplicationError> {
        let record = Self(value);
        record.wrap()?;
        Ok(record)
    }

    fn wrap(&self) -> Result<PasswordWrap, ApplicationError> {
        if self.0.len() > 4096 {
            return Err(invalid());
        }
        let value: serde_json::Value = serde_json::from_str(&self.0).map_err(|_| invalid())?;
        if value["version"].as_u64() != Some(1) {
            return Err(invalid());
        }
        let bytes: Vec<u8> =
            serde_json::from_value(value["verifier"].clone()).map_err(|_| invalid())?;
        PasswordWrap::decode(&bytes).map_err(|_| invalid())
    }

    /// Authentication failure is deliberately separate from corrupt/newer records.
    pub fn verify(&self, pin: String) -> Result<bool, ApplicationError> {
        let wrap = self.wrap()?;
        let pin = Zeroizing::new(pin);
        if !(6..=12).contains(&pin.len()) || !pin.bytes().all(|b| b.is_ascii_digit()) {
            return Ok(false);
        }
        let password = Password::new(pin.as_bytes().to_vec()).map_err(|_| invalid())?;
        Ok(unwrap_master_key_with_password(&wrap, &password).is_ok())
    }
}

impl DesktopLibrary {
    pub fn app_pin(&self) -> Result<Option<AppPinRecord>, ApplicationError> {
        self.worker
            .request("app_pin", |reply| StorageRequest::AppPin { reply })
    }

    /// Compare-and-replace keeps stale settings callbacks from overwriting a new PIN.
    pub fn replace_app_pin(
        &self,
        expected: Option<AppPinRecord>,
        next: Option<AppPinRecord>,
    ) -> Result<(), ApplicationError> {
        self.worker
            .request("replace_app_pin", |reply| StorageRequest::ReplaceAppPin {
                expected,
                next,
                reply,
            })
    }
}

pub(crate) fn replace(
    database: &mut teleark_storage::Database,
    expected: Option<AppPinRecord>,
    next: Option<AppPinRecord>,
) -> Result<(), ApplicationError> {
    let current = database
        .setting(SETTING_KEY)
        .map_err(map_storage_error)?
        .map(|s| s.value);
    if current != expected.map(|r| r.0) {
        return Err(ApplicationError::new(ApplicationErrorKind::Conflict));
    }
    match next {
        Some(record) => database
            .set_setting(&SettingRecord {
                key: SETTING_KEY.into(),
                value: record.0,
                updated_at_unix_ms: crate::system_time_unix_ms(std::time::SystemTime::now())
                    .ok_or_else(invalid)?,
            })
            .map_err(map_storage_error),
        None => database
            .delete_setting(SETTING_KEY)
            .map(|_| ())
            .map_err(map_storage_error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_roundtrip_replacement_disable_and_stale_write() {
        let dir = tempfile::tempdir().expect("fixture");
        let library = DesktopLibrary::open(dir.path().join("library.sqlite3")).expect("library");
        assert!(library.app_pin().expect("read").is_none());
        let pin = AppPinRecord::create("193857".into()).expect("create");
        assert!(pin.verify("193857".into()).expect("verify"));
        assert!(!pin.verify("193858".into()).expect("incorrect"));
        library
            .replace_app_pin(None, Some(pin.clone()))
            .expect("enable");
        let loaded = library.app_pin().expect("read").expect("enabled");
        assert!(loaded.verify("193857".into()).expect("persisted verifier"));
        assert!(library.replace_app_pin(None, None).is_err());
        library.replace_app_pin(Some(pin), None).expect("disable");
        assert!(library.app_pin().expect("disabled").is_none());
    }

    #[test]
    fn frozen_v1_envelope_authenticates_and_is_not_a_vault_key() {
        let record = AppPinRecord::decode(include_str!("fixtures/app-pin-v1.json").trim().into())
            .expect("v1 reader");
        assert!(record.verify("193857".into()).expect("frozen PIN"));
        assert!(!record.verify("000000".into()).expect("wrong PIN"));
    }

    #[test]
    fn invalid_and_newer_records_fail_closed() {
        for value in ["", "{}", "{\"version\":2,\"verifier\":[]}"] {
            assert!(AppPinRecord::decode(value.into()).is_err());
        }
        for pin in ["12345", "abcdef", "1234567890123"] {
            assert!(AppPinRecord::create(pin.into()).is_err());
        }
    }
}
