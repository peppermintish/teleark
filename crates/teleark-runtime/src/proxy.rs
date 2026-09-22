//! Versioned network policy. A missing/corrupt record is never interpreted as
//! direct access. Migration 14 alone creates the explicit initial direct policy.
use super::*;
use teleark_telegram::network::{NetworkRoute, ProxyConfig, ProxyProtocol};

const POLICY_KEY: &str = "network.proxy";
const POLICY_VERSION: u64 = 1;

fn invalid() -> ApplicationError {
    ApplicationError::new(ApplicationErrorKind::Persistence)
}

fn encode(route: &NetworkRoute) -> Result<String, ApplicationError> {
    let value = match route {
        NetworkRoute::Direct => serde_json::json!({"version": POLICY_VERSION, "mode": "direct"}),
        NetworkRoute::Proxy(proxy) => {
            ProxyConfig::new(
                proxy.protocol,
                proxy.address,
                proxy.username().to_owned(),
                proxy.password().to_owned(),
            )
            .map_err(|_| ApplicationError::new(ApplicationErrorKind::InvalidRequest))?;
            serde_json::json!({"version": POLICY_VERSION, "mode": match proxy.protocol { ProxyProtocol::Socks5 => "socks5", ProxyProtocol::HttpConnect => "http_connect" }, "address": proxy.address.to_string(), "username": proxy.username(), "password": proxy.password()})
        }
    };
    serde_json::to_string(&value).map_err(|_| invalid())
}

fn decode(bytes: &str) -> Result<NetworkRoute, ApplicationError> {
    if bytes.len() > 8192 {
        return Err(invalid());
    }
    let value: serde_json::Value = serde_json::from_str(bytes).map_err(|_| invalid())?;
    let object = value.as_object().ok_or_else(invalid)?;
    if value.get("version").and_then(|v| v.as_u64()) != Some(POLICY_VERSION) {
        return Err(invalid());
    }
    match value.get("mode").and_then(|v| v.as_str()) {
        Some("direct") if object.len() == 2 => Ok(NetworkRoute::Direct),
        Some(mode @ ("socks5" | "http_connect")) if object.len() == 5 => {
            let text = |key| value.get(key).and_then(|v| v.as_str()).ok_or_else(invalid);
            let address = text("address")?.parse().map_err(|_| invalid())?;
            let proxy = ProxyConfig::new(
                if mode == "socks5" {
                    ProxyProtocol::Socks5
                } else {
                    ProxyProtocol::HttpConnect
                },
                address,
                text("username")?.to_owned(),
                text("password")?.to_owned(),
            )
            .map_err(|_| invalid())?;
            Ok(NetworkRoute::Proxy(proxy))
        }
        _ => Err(invalid()),
    }
}

const SERVICE: &str = "app.teleark.network-policy.v1";

pub(super) struct StoredProxyPolicy {
    encoded: zeroize::Zeroizing<String>,
    kind: ProxyPolicyKind,
}
enum ProxyPolicyKind {
    Legacy(NetworkRoute),
    Credential(String),
}
fn policy(encoded: String) -> Result<StoredProxyPolicy, ApplicationError> {
    if encoded.len() > 8192 {
        return Err(invalid());
    }
    let value: serde_json::Value = serde_json::from_str(&encoded).map_err(|_| invalid())?;
    let kind = if value.get("version").and_then(|v| v.as_u64()) == Some(2) {
        let object = value.as_object().ok_or_else(invalid)?;
        let identity = value
            .get("credential_id")
            .and_then(|v| v.as_str())
            .ok_or_else(invalid)?;
        if object.len() != 3
            || value.get("mode").and_then(|v| v.as_str()) != Some("credential")
            || identity.len() != 64
            || !identity.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(invalid());
        }
        ProxyPolicyKind::Credential(identity.to_owned())
    } else {
        ProxyPolicyKind::Legacy(decode(&encoded)?)
    };
    Ok(StoredProxyPolicy {
        encoded: zeroize::Zeroizing::new(encoded),
        kind,
    })
}

pub(super) fn read_policy(database: &Database) -> Result<StoredProxyPolicy, ApplicationError> {
    policy(
        database
            .setting(POLICY_KEY)
            .map_err(map_storage_error)?
            .ok_or_else(invalid)?
            .value,
    )
}

pub(super) fn save_reference(
    database: &mut Database,
    expected: &str,
    replacement: String,
) -> Result<(), ApplicationError> {
    let current = read_policy(database)?;
    if current.encoded.as_str() != expected {
        return Err(ApplicationError::new(ApplicationErrorKind::Conflict));
    }
    policy(replacement.clone())?;
    let updated_at_unix_ms = system_time_unix_ms(SystemTime::now()).ok_or_else(invalid)?;
    database
        .set_settings(&[SettingRecord {
            key: POLICY_KEY.into(),
            value: replacement,
            updated_at_unix_ms,
        }])
        .map_err(map_storage_error)
}

fn read_current(library: &DesktopLibrary) -> Result<StoredProxyPolicy, ApplicationError> {
    library.worker.request("proxy_configuration", |reply| {
        StorageRequest::ProxyConfiguration { reply }
    })
}

fn replace_policy(
    library: &DesktopLibrary,
    current: StoredProxyPolicy,
    route: &NetworkRoute,
) -> Result<(), ApplicationError> {
    let (replacement, identity) = if matches!(route, NetworkRoute::Direct) {
        (encode(route)?, None)
    } else {
        use teleark_crypto::RandomSource;
        let mut random = [0u8; 32];
        teleark_crypto::OsRandom
            .fill_bytes(&mut random)
            .map_err(|_| invalid())?;
        let identity = random
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let encoded = zeroize::Zeroizing::new(encode(route)?);
        library.credential_write(SERVICE, &identity, encoded.as_bytes())?;
        (
            serde_json::json!({"version":2,"mode":"credential","credential_id":identity})
                .to_string(),
            Some(identity),
        )
    };
    let old_identity = match current.kind {
        ProxyPolicyKind::Credential(identity) => Some(identity),
        _ => None,
    };
    let result = library.worker.request("set_proxy_configuration", |reply| {
        StorageRequest::SetProxyConfiguration {
            expected: current.encoded,
            replacement,
            reply,
        }
    });
    // Each candidate has a distinct identity. Failed CAS cannot overwrite another
    // writer's route; retiring an old reference cannot delete the new credential.
    let retire = if result.is_ok() {
        old_identity
    } else {
        identity
    };
    if let Some(identity) = retire {
        let _ = library.credential_delete(SERVICE, &identity);
    }
    result
}

pub(super) fn load_configuration(
    library: &DesktopLibrary,
) -> Result<NetworkRoute, ApplicationError> {
    for _ in 0..4 {
        let current = read_current(library)?;
        match &current.kind {
            ProxyPolicyKind::Credential(identity) => {
                if let Some(bytes) = library.credential_read(SERVICE, identity)? {
                    return decode(std::str::from_utf8(&bytes).map_err(|_| invalid())?);
                }
                // A concurrent route commit can retire the old reference.
                let next = read_current(library)?;
                if current.encoded == next.encoded {
                    return Err(invalid());
                }
            }
            ProxyPolicyKind::Legacy(route) => {
                // Public direct routing never requires Keychain access.
                if matches!(route, NetworkRoute::Direct) {
                    return Ok(NetworkRoute::Direct);
                }
                let route = route.clone();
                match replace_policy(library, current, &route) {
                    Ok(()) => return Ok(route),
                    Err(error) if error.kind() == ApplicationErrorKind::Conflict => continue,
                    Err(error) => return Err(error),
                }
            }
        }
    }
    Err(ApplicationError::new(ApplicationErrorKind::Conflict))
}

pub(super) fn save_configuration(
    library: &DesktopLibrary,
    route: &NetworkRoute,
) -> Result<(), ApplicationError> {
    replace_policy(library, read_current(library)?, route)
}

#[cfg(test)]
pub(super) fn load(database: &Database) -> Result<NetworkRoute, ApplicationError> {
    let record = database
        .setting(POLICY_KEY)
        .map_err(map_storage_error)?
        .ok_or_else(invalid)?;
    decode(&record.value)
}

#[cfg(test)]
pub(super) fn save(database: &mut Database, route: &NetworkRoute) -> Result<(), ApplicationError> {
    // Preserve unsupported/newer or damaged bytes instead of overwriting them.
    load(database)?;
    let value = encode(route)?;
    let updated_at_unix_ms = system_time_unix_ms(SystemTime::now()).ok_or_else(invalid)?;
    database
        .set_settings(&[SettingRecord {
            key: POLICY_KEY.to_owned(),
            value,
            updated_at_unix_ms,
        }])
        .map_err(map_storage_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_migration_cannot_replace_a_newer_direct_route() {
        let directory = tempfile::tempdir().expect("fixture");
        let path = directory.path().join("proxy.sqlite3");
        let mut db = Database::open(&path).expect("database");
        let legacy = decode(include_str!("fixtures/proxy-v1-socks5.json")).expect("legacy");
        save(&mut db, &legacy).expect("fixture");
        drop(db);
        let library = DesktopLibrary::open_synthetic(&path).expect("library");
        let stale = read_current(&library).expect("capture legacy");
        save_configuration(&library, &NetworkRoute::Direct).expect("newer direct");
        let db = Database::open(&path).expect("inspect");
        let committed = db
            .setting(POLICY_KEY)
            .expect("committed record")
            .expect("present");
        assert_eq!(
            replace_policy(&library, stale, &legacy)
                .expect_err("stale migration")
                .kind(),
            ApplicationErrorKind::Conflict
        );
        assert_eq!(
            load_configuration(&library).expect("newer preserved"),
            NetworkRoute::Direct
        );
        // Dependency features can change JSON object ordering. The stale write
        // must preserve the actual committed bytes and timestamp in either mode.
        assert_eq!(
            db.setting(POLICY_KEY).expect("record").expect("present"),
            committed
        );
    }

    #[test]
    fn authenticated_proxy_migrates_to_versioned_reference_and_survives_restart() {
        let directory = tempfile::tempdir().expect("fixture");
        let path = directory.path().join("proxy.sqlite3");
        let mut db = Database::open(&path).expect("database");
        let legacy = decode(include_str!("fixtures/proxy-v1-socks5.json")).expect("legacy");
        save(&mut db, &legacy).expect("fixture");
        drop(db);
        let library = DesktopLibrary::open_synthetic(&path).expect("library");
        assert_eq!(load_configuration(&library).expect("migrated"), legacy);
        let db = Database::open(&path).expect("inspect");
        let value = db
            .setting(POLICY_KEY)
            .expect("record")
            .expect("present")
            .value;
        assert!(value.contains("credential_id"));
        assert!(!value.contains("password"));
        drop(db);
        drop(library);
        let library = DesktopLibrary::open_synthetic(&path).expect("reopen");
        assert_eq!(load_configuration(&library).expect("restored"), legacy);
        save_configuration(&library, &NetworkRoute::Direct).expect("direct");
        assert_eq!(
            load_configuration(&library).expect("direct"),
            NetworkRoute::Direct
        );
    }

    #[test]
    fn fixed_v1_fixtures_remain_readable_and_future_bytes_cannot_be_overwritten() {
        for bytes in [
            include_str!("fixtures/proxy-v1-direct.json"),
            include_str!("fixtures/proxy-v1-socks5.json"),
            include_str!("fixtures/proxy-v1-http-connect.json"),
        ] {
            let route = decode(bytes).expect("v1 golden fixture");
            assert_eq!(
                decode(&encode(&route).expect("encode fixture")).expect("decode fixture"),
                route
            );
        }
        let directory = tempfile::tempdir().expect("fixture directory");
        let path = directory.path().join("future.sqlite3");
        let mut db = Database::open(&path).expect("fixture database");
        let future = r#"{"version":2,"mode":"socks5","future":"preserve"}"#;
        db.set_settings(&[SettingRecord {
            key: POLICY_KEY.into(),
            value: future.into(),
            updated_at_unix_ms: 1,
        }])
        .expect("future fixture");
        assert!(save(&mut db, &NetworkRoute::Direct).is_err());
        assert_eq!(
            db.setting(POLICY_KEY).expect("read").expect("record").value,
            future
        );
        drop(db);
        let library = DesktopLibrary::open(&path).expect("library");
        assert!(
            crate::DesktopTelegram::open_configured(directory.path().join("session"), &library)
                .is_err()
        );
    }

    #[test]
    fn policy_round_trips_both_protocols_and_direct_without_logging_passwords() {
        for protocol in [ProxyProtocol::Socks5, ProxyProtocol::HttpConnect] {
            let route = NetworkRoute::Proxy(
                ProxyConfig::new(
                    protocol,
                    "127.0.0.1:1080".parse().expect("controlled proxy fixture"),
                    "user@name".into(),
                    "synthetic:/@password".into(),
                )
                .expect("controlled proxy fixture"),
            );
            assert_eq!(
                decode(&encode(&route).expect("controlled proxy fixture"))
                    .expect("controlled proxy fixture"),
                route
            );
            assert!(!format!("{route:?}").contains("synthetic:/@password"));
            assert!(!format!("{route:?}").contains("user@name"));
        }
        assert_eq!(
            decode(r#"{"version":1,"mode":"direct"}"#).expect("controlled proxy fixture"),
            NetworkRoute::Direct
        );
    }

    #[test]
    fn invalid_or_future_policies_never_default_to_direct() {
        for bytes in [
            "",
            "{}",
            r#"{"version":2,"mode":"direct"}"#,
            r#"{"version":1,"mode":"unknown"}"#,
            r#"{"version":1,"mode":"direct","address":"127.0.0.1:1080"}"#,
            r#"{"version":1,"mode":"socks5","address":"proxy.example:1080","username":"","password":""}"#,
            r#"{"version":1,"mode":"socks5","address":"127.0.0.1:0","username":"","password":""}"#,
        ] {
            assert_eq!(
                decode(bytes)
                    .expect_err("operation must fail closed")
                    .kind(),
                ApplicationErrorKind::Persistence
            );
        }
    }

    #[test]
    fn migrated_policy_persists_across_restart_and_missing_record_fails_closed() {
        let directory = tempfile::tempdir().expect("controlled proxy fixture");
        let path = directory.path().join("policy.sqlite3");
        let library = DesktopLibrary::open(&path).expect("controlled proxy fixture");
        assert_eq!(
            library
                .proxy_configuration()
                .expect("controlled proxy fixture"),
            NetworkRoute::Direct
        );
        let route = NetworkRoute::Proxy(
            ProxyConfig::new(
                ProxyProtocol::Socks5,
                "127.0.0.1:1080".parse().expect("controlled proxy fixture"),
                String::new(),
                String::new(),
            )
            .expect("controlled proxy fixture"),
        );
        library
            .set_proxy_configuration(&route)
            .expect("controlled proxy fixture");
        drop(library);
        let library = DesktopLibrary::open(&path).expect("controlled proxy fixture");
        assert_eq!(
            library
                .proxy_configuration()
                .expect("controlled proxy fixture"),
            route
        );
        drop(library);
        let mut db = Database::open(&path).expect("controlled proxy fixture");
        db.delete_settings(&[POLICY_KEY])
            .expect("controlled proxy fixture");
        assert_eq!(
            load(&db).expect_err("operation must fail closed").kind(),
            ApplicationErrorKind::Persistence
        );
    }
}
