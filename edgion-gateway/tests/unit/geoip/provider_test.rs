use super::*;
use std::io::Write;

fn fixture_city_db() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../edgion-tests/geoip/edgion-test-city.mmdb")
}

#[test]
fn empty_provider_reports_unavailable() {
    let p = GeoIpProvider::new();
    assert!(!p.is_loaded());
    assert_eq!(p.lookup("203.0.113.7".parse().unwrap()), GeoLookup::Unavailable);
}

#[test]
fn missing_file_is_an_error_not_a_panic() {
    let p = GeoIpProvider::new();
    let err = p.load_from(std::path::Path::new("/nonexistent/x.mmdb")).unwrap_err();
    assert!(!err.is_empty());
    assert!(!p.is_loaded());
}

#[test]
fn corrupt_file_is_an_error_not_a_panic() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad.mmdb");
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(b"this is not an mmdb").unwrap();
    drop(f);

    let p = GeoIpProvider::new();
    assert!(p.load_from(&path).is_err());
    assert!(!p.is_loaded());
}

#[test]
fn loaded_db_resolves_a_known_address() {
    let p = GeoIpProvider::new();
    p.load_from(&fixture_city_db()).expect("fixture must load");
    assert!(p.is_loaded());

    match p.lookup("203.0.113.7".parse().unwrap()) {
        GeoLookup::Found(info) => {
            assert_eq!(info.country_iso.as_deref(), Some("GB"));
        }
        other => panic!("expected Found, got {other:?}"),
    }
}

#[test]
fn unknown_address_is_not_found_not_unavailable() {
    let p = GeoIpProvider::new();
    p.load_from(&fixture_city_db()).unwrap();
    assert_eq!(p.lookup("8.8.8.8".parse().unwrap()), GeoLookup::NotFound);
}

/// A dual-stack listener hands us the mapped form; it must resolve
/// identically to the plain IPv4 address.
#[test]
fn ipv4_mapped_address_resolves_like_its_ipv4_form() {
    let p = GeoIpProvider::new();
    p.load_from(&fixture_city_db()).unwrap();
    let plain = p.lookup("203.0.113.7".parse().unwrap());
    let mapped = p.lookup("::ffff:203.0.113.7".parse().unwrap());
    assert_eq!(plain, mapped);
}

/// `127.0.0.6` resolves on the City fixture but carries no city/subdivision
/// record. This is per-record absence, and must route to `onMissing`, not
/// to the database-unavailable (`failOpen`) path — those are two
/// different `GeoLookup` outcomes (`Found` with `None` dimensions, vs.
/// `Unavailable`) and callers must not conflate them.
#[test]
fn dimension_absent_on_record_is_ordinary_missingness() {
    let p = GeoIpProvider::new();
    p.load_from(&fixture_city_db()).unwrap();
    let GeoLookup::Found(info) = p.lookup("127.0.0.6".parse().unwrap()) else {
        panic!("127.0.0.6 must resolve on the City fixture");
    };
    assert_eq!(info.country_iso.as_deref(), Some("US"));
    assert_eq!(info.city, None);
    assert_eq!(info.subdivision, None);
}

/// Reloading swaps the active database without disturbing the API.
#[test]
fn reload_replaces_the_active_database() {
    let p = GeoIpProvider::new();
    p.load_from(&fixture_city_db()).unwrap();
    let first = p.build_epoch();
    p.load_from(&fixture_city_db()).unwrap();
    assert_eq!(p.build_epoch(), first);
    assert!(p.is_loaded());
}

#[test]
fn decode_fault_is_failed_not_not_found() {
    let p = GeoIpProvider::new();
    p.load_from(&fixture_city_db()).unwrap();
    let _fault = inject_lookup_fault(LookupFault::Decode);
    assert_eq!(p.lookup("203.0.113.7".parse().unwrap()), GeoLookup::Failed);
}

#[test]
fn panic_fault_is_failed_and_increments_panic_counter() {
    let p = GeoIpProvider::new();
    p.load_from(&fixture_city_db()).unwrap();
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    metrics::with_local_recorder(&recorder, || {
        let _fault = inject_lookup_fault(LookupFault::Panic);
        assert_eq!(p.lookup("203.0.113.7".parse().unwrap()), GeoLookup::Failed);
    });
    let rendered = handle.render();
    assert!(
        rendered.contains("edgion_geoip_lookup_panic_total"),
        "panic seam must increment edgion_geoip_lookup_panic_total, got:\n{rendered}"
    );
}

#[test]
fn decode_fault_does_not_increment_panic_counter() {
    let p = GeoIpProvider::new();
    p.load_from(&fixture_city_db()).unwrap();
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    metrics::with_local_recorder(&recorder, || {
        let _fault = inject_lookup_fault(LookupFault::Decode);
        assert_eq!(p.lookup("8.8.8.8".parse().unwrap()), GeoLookup::Failed);
    });
    let rendered = handle.render();
    assert!(
        !rendered.contains("edgion_geoip_lookup_panic_total"),
        "decode seam must not increment the panic counter, got:\n{rendered}"
    );
}

#[test]
fn fault_guard_clears_so_later_lookups_are_real() {
    let p = GeoIpProvider::new();
    p.load_from(&fixture_city_db()).unwrap();
    {
        let _fault = inject_lookup_fault(LookupFault::Decode);
        assert_eq!(p.lookup("203.0.113.7".parse().unwrap()), GeoLookup::Failed);
    }
    match p.lookup("203.0.113.7".parse().unwrap()) {
        GeoLookup::Found(_) => {}
        other => panic!("cleared seam must not stick, got {other:?}"),
    }
}
