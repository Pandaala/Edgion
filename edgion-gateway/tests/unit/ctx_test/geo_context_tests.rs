use super::*;
use crate::geoip::{GeoIpProvider, GeoLookup, GEO_TEST_LOCK};

#[test]
fn geo_slots_start_unresolved_and_resolve_once() {
    // Serialize against the process-wide GeoIpProvider singleton: other
    // modules' tests in this same test binary load the fixture database
    // into it, so without the lock + clear this test would flake
    // depending on run order.
    let _g = GEO_TEST_LOCK.lock();
    GeoIpProvider::global().clear();

    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.direct_peer_ip = "203.0.113.7".to_string();
    ctx.request_info.remote_ip = "203.0.113.7".to_string();
    assert!(ctx.request_info.geo.get().is_none());

    // No database loaded in this test process.
    assert_eq!(*ctx.remote_geo(), GeoLookup::Unavailable);
    assert!(ctx.request_info.geo.get().is_some());
    // Second call returns the cached value, not a second lookup.
    assert_eq!(*ctx.remote_geo(), GeoLookup::Unavailable);
}

#[test]
fn equal_addresses_share_one_lookup() {
    for (direct, remote) in [
        ("203.0.113.7", "203.0.113.7"),
        ("2001:db8::7", "2001:db8::7"),
        ("203.0.113.7:443", "203.0.113.7:443"),
        ("[2001:db8::7]:443", "[2001:db8::7]:443"),
        ("::ffff:203.0.113.7", "203.0.113.7"),
        ("203.0.113.7:443", "203.0.113.7:8443"),
        ("[2001:db8::7]:443", "2001:db8:0:0:0:0:0:7"),
    ] {
        for remote_first in [false, true] {
            let mut ctx = EdgionHttpContext::default();
            ctx.request_info.direct_peer_ip = direct.into();
            ctx.request_info.remote_ip = remote.into();
            let info = std::sync::Arc::new(crate::geoip::GeoInfo::default());
            let geo = ctx.request_info.geo();
            let first = if remote_first { &geo.remote } else { &geo.direct_peer };
            first.set(GeoLookup::Found(info.clone())).unwrap();
            let second = if remote_first {
                ctx.direct_peer_geo()
            } else {
                ctx.remote_geo()
            };
            let GeoLookup::Found(reused) = second else {
                panic!("equal addresses must reuse the seeded result: {direct}, {remote}");
            };
            assert!(std::sync::Arc::ptr_eq(reused, &info));
        }
    }
}

#[test]
fn invalid_or_different_addresses_do_not_reuse_a_cached_result() {
    let _g = GEO_TEST_LOCK.lock();
    GeoIpProvider::global().clear();
    let cached = std::sync::OnceLock::new();
    cached
        .set(GeoLookup::Found(std::sync::Arc::new(crate::geoip::GeoInfo::default())))
        .unwrap();
    let ctx = EdgionHttpContext::default();
    for (address, other, expected) in [
        ("", "", GeoLookup::NotFound),
        ("not-an-ip", "not-an-ip", GeoLookup::NotFound),
        ("not-an-ip", "203.0.113.7", GeoLookup::NotFound),
        ("203.0.113.7", "not-an-ip", GeoLookup::Unavailable),
        ("203.0.113.7", "203.0.113.8", GeoLookup::Unavailable),
    ] {
        assert_eq!(ctx.resolve_geo(address, &cached, other), expected);
    }
}

#[test]
fn unparseable_address_reports_not_found_not_panic() {
    let _g = GEO_TEST_LOCK.lock();
    GeoIpProvider::global().clear();

    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.remote_ip = "not-an-ip".to_string();
    assert_eq!(*ctx.remote_geo(), GeoLookup::NotFound);
}
