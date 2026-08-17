//! Throwaway verification for Task 2a of the GeoIP country-restriction feature.
//!
//! The synthetic MMDB fixtures under `edgion-tests/geoip/` were validated with
//! the Python `maxminddb` reader only. The gateway reads them through the Rust
//! `maxminddb` crate, and a field-type mismatch between the writer and the
//! `geoip2::City` projection fails silently: an unexpected type on a field
//! (for example a `latitude` encoded as something `geoip2::City` cannot
//! decode) makes the projection yield `None` for that field, so every
//! coordinate assertion downstream would pass vacuously instead of failing.
//! This test prints every projected field for every probe address in the
//! authored address map (see `edgion-tests/geoip/README.md`) and asserts each
//! value matches expectations, so a mismatch is caught here instead of in a
//! much later, harder-to-diagnose test.

use std::net::IpAddr;
use std::path::PathBuf;

use maxminddb::geoip2;

fn fixture_path(name: &str) -> PathBuf {
    // CARGO_MANIFEST_DIR is edgion-gateway/; the fixtures live at the
    // workspace-sibling edgion-tests/geoip/.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("edgion-tests")
        .join("geoip")
        .join(name)
}

struct Probe {
    addr: &'static str,
    country: Option<&'static str>,
    continent: Option<&'static str>,
    city: Option<&'static str>,
    subdivision: Option<&'static str>,
    coords: Option<(f64, f64)>,
}

const PROBES: &[Probe] = &[
    Probe {
        addr: "127.0.0.1",
        country: Some("CN"),
        continent: Some("AS"),
        city: Some("Beijing"),
        subdivision: Some("BJ"),
        coords: Some((39.9042, 116.4074)),
    },
    Probe {
        addr: "127.0.0.5",
        country: Some("RU"),
        continent: Some("EU"),
        city: Some("Moscow"),
        subdivision: Some("MOW"),
        coords: Some((55.7558, 37.6173)),
    },
    Probe {
        addr: "127.0.0.6",
        country: Some("US"),
        continent: Some("NA"),
        city: None,
        subdivision: None,
        coords: None,
    },
    Probe {
        addr: "127.0.0.7",
        country: Some("HK"),
        continent: Some("AS"),
        city: Some("Kowloon"),
        subdivision: Some("KOW"),
        coords: Some((22.3193, 114.1694)),
    },
    Probe {
        addr: "10.1.2.3",
        country: Some("US"),
        continent: Some("NA"),
        city: Some("Cambridge"),
        subdivision: Some("MA"),
        coords: Some((42.3736, -71.1097)),
    },
    Probe {
        addr: "203.0.113.55",
        country: Some("GB"),
        continent: Some("EU"),
        city: Some("London"),
        subdivision: Some("ENG"),
        coords: Some((51.5074, -0.1278)),
    },
    Probe {
        addr: "198.51.100.9",
        country: Some("HK"),
        continent: Some("AS"),
        city: None,
        subdivision: None,
        coords: None,
    },
    // Unmapped probes: must resolve to nothing.
    Probe {
        addr: "8.8.8.8",
        country: None,
        continent: None,
        city: None,
        subdivision: None,
        coords: None,
    },
    Probe {
        addr: "127.0.0.2",
        country: None,
        continent: None,
        city: None,
        subdivision: None,
        coords: None,
    },
    Probe {
        addr: "::ffff:127.0.0.1",
        country: None,
        continent: None,
        city: None,
        subdivision: None,
        coords: None,
    },
];

#[test]
fn city_fixture_readback_matches_address_map() {
    let path = fixture_path("edgion-test-city.mmdb");
    // SAFETY: the fixture file is a static test asset that is never modified
    // on disk while this test process holds the mapping.
    let reader: maxminddb::Reader<memmap2::Mmap> = unsafe { maxminddb::Reader::open_mmap(&path) }
        .unwrap_or_else(|e| panic!("failed to open {}: {e}", path.display()));

    assert!(
        reader.metadata().database_type.contains("City"),
        "city fixture database_type must contain \"City\", got {:?}",
        reader.metadata().database_type
    );

    for probe in PROBES {
        let ip: IpAddr = probe.addr.parse().expect("valid probe address");
        let lookup = reader.lookup(ip).unwrap_or_else(|e| {
            panic!("lookup({}) errored: {e}", probe.addr);
        });
        let result = lookup.decode::<geoip2::City>().unwrap_or_else(|e| {
            panic!("decode({}) errored: {e}", probe.addr);
        });

        let country_iso = result
            .as_ref()
            .and_then(|c| c.country.iso_code.filter(|s| !s.is_empty()));
        let continent_code = result.as_ref().and_then(|c| c.continent.code.filter(|s| !s.is_empty()));
        let city_name = result
            .as_ref()
            .and_then(|c| c.city.names.english.filter(|s| !s.is_empty()));
        let subdivision_iso = result.as_ref().and_then(|c| {
            c.subdivisions
                .first()
                .and_then(|s| s.iso_code)
                .filter(|s| !s.is_empty())
        });
        let latitude = result.as_ref().and_then(|c| c.location.latitude);
        let longitude = result.as_ref().and_then(|c| c.location.longitude);

        println!(
            "probe={:<18} country={:?} continent={:?} city={:?} subdivision={:?} lat={:?} lon={:?}",
            probe.addr, country_iso, continent_code, city_name, subdivision_iso, latitude, longitude
        );

        assert_eq!(country_iso, probe.country, "country mismatch for {}", probe.addr);
        assert_eq!(continent_code, probe.continent, "continent mismatch for {}", probe.addr);
        assert_eq!(city_name, probe.city, "city mismatch for {}", probe.addr);
        assert_eq!(
            subdivision_iso, probe.subdivision,
            "subdivision mismatch for {}",
            probe.addr
        );
        match probe.coords {
            Some((lat, lon)) => {
                assert_eq!(latitude, Some(lat), "latitude mismatch for {}", probe.addr);
                assert_eq!(longitude, Some(lon), "longitude mismatch for {}", probe.addr);
            }
            None => {
                assert_eq!(latitude, None, "expected no latitude for {}", probe.addr);
                assert_eq!(longitude, None, "expected no longitude for {}", probe.addr);
            }
        }
    }
}

#[test]
fn country_fixture_readback_matches_address_map() {
    let path = fixture_path("edgion-test-country.mmdb");
    // SAFETY: the fixture file is a static test asset that is never modified
    // on disk while this test process holds the mapping.
    let reader: maxminddb::Reader<memmap2::Mmap> = unsafe { maxminddb::Reader::open_mmap(&path) }
        .unwrap_or_else(|e| panic!("failed to open {}: {e}", path.display()));

    assert!(
        !reader.metadata().database_type.contains("City"),
        "country fixture database_type must NOT contain \"City\", got {:?}",
        reader.metadata().database_type
    );
    assert!(
        reader.metadata().database_type.contains("Country"),
        "country fixture database_type must contain \"Country\", got {:?}",
        reader.metadata().database_type
    );

    for probe in PROBES {
        let ip: IpAddr = probe.addr.parse().expect("valid probe address");
        let lookup = reader.lookup(ip).unwrap_or_else(|e| {
            panic!("lookup({}) errored: {e}", probe.addr);
        });
        let result = lookup.decode::<geoip2::Country>().unwrap_or_else(|e| {
            panic!("decode({}) errored: {e}", probe.addr);
        });

        let country_iso = result
            .as_ref()
            .and_then(|c| c.country.iso_code.filter(|s| !s.is_empty()));
        let continent_code = result.as_ref().and_then(|c| c.continent.code.filter(|s| !s.is_empty()));

        println!(
            "probe={:<18} country={:?} continent={:?}",
            probe.addr, country_iso, continent_code
        );

        assert_eq!(country_iso, probe.country, "country mismatch for {}", probe.addr);
        assert_eq!(continent_code, probe.continent, "continent mismatch for {}", probe.addr);
    }
}
