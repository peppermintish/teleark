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

pub(super) fn load(database: &Database) -> Result<NetworkRoute, ApplicationError> {
    let record = database
        .setting(POLICY_KEY)
        .map_err(map_storage_error)?
        .ok_or_else(invalid)?;
    decode(&record.value)
}

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
