//! Application identity sent during MTProto connection initialization.

pub(crate) fn params() -> grammers_mtsender::ConnectionParams {
    grammers_mtsender::ConnectionParams {
        app_version: env!("CARGO_PKG_VERSION").to_owned(),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn connection_reports_the_workspace_application_version() {
        let workspace = include_str!("../../../Cargo.toml");
        let version = workspace
            .split("[workspace.package]")
            .nth(1)
            .expect("workspace package section")
            .lines()
            .find_map(|line| line.strip_prefix("version = "))
            .expect("workspace version")
            .trim_matches('"');
        assert_eq!(super::params().app_version, version);
    }
}
