use super::*;

#[test]
fn defaults_leave_geoip_unconfigured_and_safe() {
    let cfg: GeoIpConfig = serde_yaml::from_str("{}").unwrap();
    assert!(cfg.db_path.is_none());
    assert!(cfg.update_url.is_none());
    assert_eq!(cfg.update_interval_sec, 86400);
    assert_eq!(cfg.retry_interval_sec, 600);
    assert_eq!(cfg.max_db_bytes, 268_435_456);
}

#[test]
fn snake_case_keys_parse() {
    let yaml = r#"
db_path: /var/lib/edgion/geoip/db.mmdb
update_url: https://mirror.internal/geo.mmdb
max_db_bytes: 1024
"#;
    let cfg: GeoIpConfig = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(cfg.db_path, Some(PathBuf::from("/var/lib/edgion/geoip/db.mmdb")));
    assert_eq!(cfg.max_db_bytes, 1024);
}

#[test]
fn configured_updater_accepts_safe_defaults() {
    let cfg = GeoIpConfig {
        update_url: Some("https://mirror.internal/geo.mmdb".into()),
        ..Default::default()
    };
    assert!(cfg.validate_updater_settings().is_ok());
}

#[test]
#[allow(clippy::type_complexity)]
fn configured_updater_rejects_zero_durations_intervals_and_size_cap() {
    let base = GeoIpConfig {
        update_url: Some("https://mirror.internal/geo.mmdb".into()),
        ..Default::default()
    };
    let cases: &[(&str, fn(&mut GeoIpConfig))] = &[
        ("update_interval_sec", |cfg| cfg.update_interval_sec = 0),
        ("retry_interval_sec", |cfg| cfg.retry_interval_sec = 0),
        ("connect_timeout_sec", |cfg| cfg.connect_timeout_sec = 0),
        ("download_deadline_sec", |cfg| cfg.download_deadline_sec = 0),
        ("stall_timeout_sec", |cfg| cfg.stall_timeout_sec = 0),
        ("max_db_bytes", |cfg| cfg.max_db_bytes = 0),
    ];

    for (field, make_invalid) in cases {
        let mut cfg = base.clone();
        make_invalid(&mut cfg);
        let err = cfg.validate_updater_settings().unwrap_err();
        assert!(err.contains(field), "expected {field} in: {err}");
    }
}

#[test]
fn disabled_updater_leaves_updater_only_values_inert() {
    let cfg = GeoIpConfig {
        update_url: None,
        update_interval_sec: 0,
        retry_interval_sec: 0,
        connect_timeout_sec: 0,
        download_deadline_sec: 0,
        stall_timeout_sec: 0,
        max_db_bytes: 0,
        ..Default::default()
    };
    assert!(cfg.validate_updater_settings().is_ok());

    let cfg = GeoIpConfig {
        update_url: Some("".into()),
        update_interval_sec: 0,
        retry_interval_sec: 0,
        connect_timeout_sec: 0,
        download_deadline_sec: 0,
        stall_timeout_sec: 0,
        max_db_bytes: 0,
        ..Default::default()
    };
    assert!(cfg.validate_updater_settings().is_ok());
}

/// A camelCase typo must be a loud error, never a silent all-defaults struct.
#[test]
fn camel_case_key_is_rejected() {
    let yaml = "dbPath: /tmp/x.mmdb\n";
    let err = serde_yaml::from_str::<GeoIpConfig>(yaml).unwrap_err();
    assert!(err.to_string().contains("dbPath"), "got: {err}");
}

/// Regression test: `update_url` is credential-bearing (may contain a token
/// for private mirrors) and must be redacted in Debug output. Other fields
/// are non-sensitive and should print normally. Unlike the previous
/// version of this test, this no longer depends on a hand-written `Debug`
/// impl remembering to redact this one field: `GeoIpConfig` derives
/// `Debug`, and `update_url`'s `RedactedString` type redacts itself — so
/// this test is really exercising that the field's *type* is
/// `RedactedString`, not a policy someone could forget to update.
#[test]
fn update_url_is_redacted_in_debug() {
    let cfg = GeoIpConfig {
        db_path: Some(PathBuf::from("/var/lib/geoip.mmdb")),
        update_url: Some("https://mirror.internal/db.mmdb?token=secret-token-1234".into()),
        update_auth_header_env: Some("GEOIP_AUTH".to_string()),
        max_db_bytes: 1024,
        ..Default::default()
    };
    let dbg = format!("{:?}", cfg);

    // Credential must never appear in Debug output
    assert!(
        !dbg.contains("secret-token-1234"),
        "update_url credential must be redacted in Debug"
    );
    assert!(
        dbg.contains("[redacted]"),
        "update_url must show [redacted] placeholder"
    );

    // Other fields should be visible
    assert!(dbg.contains("geoip.mmdb"), "db_path should be in Debug");
    assert!(
        dbg.contains("GEOIP_AUTH"),
        "update_auth_header_env (env var name, not secret) should be in Debug"
    );
    assert!(dbg.contains("1024"), "max_db_bytes should be in Debug");
}
