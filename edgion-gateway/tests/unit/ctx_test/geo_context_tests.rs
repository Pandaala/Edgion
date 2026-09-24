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
    let _g = GEO_TEST_LOCK.lock();
    GeoIpProvider::global().clear();

    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.direct_peer_ip = "::ffff:203.0.113.7".to_string();
    ctx.request_info.remote_ip = "203.0.113.7".to_string();
    // The mapped and plain forms normalize equal, so the second slot must
    // reuse the first result rather than resolving independently.
    let a = ctx.direct_peer_geo().clone();
    let b = ctx.remote_geo().clone();
    assert_eq!(a, b);
}

#[test]
fn unparseable_address_reports_not_found_not_panic() {
    let _g = GEO_TEST_LOCK.lock();
    GeoIpProvider::global().clear();

    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.remote_ip = "not-an-ip".to_string();
    assert_eq!(*ctx.remote_geo(), GeoLookup::NotFound);
}
