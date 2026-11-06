use super::*;

#[test]
fn init_with_unconfigured_geoip_leaves_provider_unloaded() {
    let _g = GEO_TEST_LOCK.lock();
    GeoIpProvider::global().clear();

    let cfg = GeoIpConfig::default();
    init(&cfg);
    assert!(!GeoIpProvider::global().is_loaded());
}

#[test]
fn init_with_a_missing_db_path_does_not_panic_and_stays_unloaded() {
    let _g = GEO_TEST_LOCK.lock();
    GeoIpProvider::global().clear();

    let cfg = GeoIpConfig {
        db_path: Some("/nonexistent/geo.mmdb".into()),
        ..Default::default()
    };
    init(&cfg);
    assert!(!GeoIpProvider::global().is_loaded());
}

/// No `db_path` at all must also not panic, and must not attempt to start
/// the updater even if `update_url` were set (there is nothing to swap
/// into).
#[test]
fn init_with_no_db_path_and_an_update_url_does_not_panic() {
    let _g = GEO_TEST_LOCK.lock();
    GeoIpProvider::global().clear();

    let cfg = GeoIpConfig {
        db_path: None,
        update_url: Some("https://mirror.internal/geo.mmdb".into()),
        ..Default::default()
    };
    init(&cfg);
    assert!(!GeoIpProvider::global().is_loaded());
}

#[test]
#[cfg(feature = "geoip")]
fn swap_directory_guard_accepts_a_writable_existing_parent() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("db.mmdb");
    assert!(ensure_swap_directory_ready(&db_path).is_ok());
}

#[test]
#[cfg(feature = "geoip")]
fn swap_directory_guard_rejects_a_missing_parent() {
    let db_path = Path::new("/nonexistent/does/not/exist/db.mmdb");
    assert!(ensure_swap_directory_ready(db_path).is_err());
}

/// BLOCKER 4 regression: a geoip config with no `db_path` must still
/// create the `edgion_geoip_db_loaded` gauge (reading 0), not leave the
/// series absent. An absent series makes the documented alert
/// `edgion_geoip_db_loaded == 0` match nothing in exactly this
/// misconfiguration, which is the one case the alert exists to catch.
#[test]
fn init_with_no_db_path_still_creates_the_loaded_gauge_at_zero() {
    let _g = GEO_TEST_LOCK.lock();
    GeoIpProvider::global().clear();

    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    metrics::with_local_recorder(&recorder, || {
        let cfg = GeoIpConfig {
            db_path: None,
            ..Default::default()
        };
        init(&cfg);
        let rendered = handle.render();
        assert!(
            rendered.contains("edgion_geoip_db_loaded"),
            "gauge must exist even when db_path is unset, got:\n{rendered}"
        );
        assert!(
            rendered.contains("edgion_geoip_db_loaded 0"),
            "gauge must read 0 when db_path is unset, got:\n{rendered}"
        );
    });
}

/// `update_url` set but a non-writable parent must fail to spawn
/// the updater without panicking or loading a database.
#[test]
#[cfg(feature = "geoip")]
fn init_with_update_url_and_unwritable_parent_does_not_spawn() {
    let _g = GEO_TEST_LOCK.lock();
    GeoIpProvider::global().clear();

    let cfg = GeoIpConfig {
        db_path: Some(std::path::PathBuf::from("/nonexistent/does/not/exist/db.mmdb")),
        update_url: Some("https://mirror.internal/geo.mmdb".into()),
        ..Default::default()
    };
    // Must not panic; the missing db_path already WARNs during load, and
    // the same-filesystem guard then refuses to start the updater.
    init(&cfg);
    assert!(!GeoIpProvider::global().is_loaded());
}

#[test]
#[cfg(not(feature = "geoip"))]
fn compiled_out_geoip_never_loads_a_configured_database() {
    let _g = GEO_TEST_LOCK.lock();
    GeoIpProvider::global().clear();

    let fixture =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../edgion-tests/geoip/edgion-test-city.mmdb");
    assert_eq!(
        GeoIpProvider::global().load_from(&fixture).unwrap_err(),
        GEOIP_NOT_COMPILED
    );
    assert_eq!(updater::validate_structural(&fixture).unwrap_err(), GEOIP_NOT_COMPILED);

    init(&GeoIpConfig {
        db_path: Some(fixture),
        ..Default::default()
    });
    assert!(!GeoIpProvider::global().is_loaded());
    assert!(matches!(
        GeoIpProvider::global().lookup("8.8.8.8".parse().unwrap()),
        GeoLookup::Unavailable
    ));
}
