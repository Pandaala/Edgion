use super::admin_exposure_is_insecure;
use std::net::SocketAddr;
use std::str::FromStr;

fn addr(s: &str) -> SocketAddr {
    SocketAddr::from_str(s).unwrap()
}

#[test]
fn loopback_empty_allowlist_is_safe() {
    assert!(!admin_exposure_is_insecure(&addr("127.0.0.1:12101"), &[]));
}

#[test]
fn ipv6_loopback_empty_allowlist_is_safe() {
    assert!(!admin_exposure_is_insecure(&addr("[::1]:12101"), &[]));
}

#[test]
fn unspecified_empty_allowlist_is_insecure() {
    assert!(admin_exposure_is_insecure(&addr("0.0.0.0:12101"), &[]));
}

#[test]
fn unspecified_with_allowlist_is_safe() {
    assert!(!admin_exposure_is_insecure(
        &addr("0.0.0.0:12101"),
        &["10.0.0.0/8".to_string()]
    ));
}

#[test]
fn specific_nonloopback_empty_allowlist_is_insecure() {
    assert!(admin_exposure_is_insecure(&addr("10.1.2.3:12101"), &[]));
}

#[test]
fn ipv4_mapped_ipv6_loopback_empty_allowlist_is_safe() {
    assert!(!admin_exposure_is_insecure(&addr("[::ffff:127.0.0.1]:12101"), &[]));
}

#[test]
fn ipv6_unspecified_empty_allowlist_is_insecure() {
    assert!(admin_exposure_is_insecure(&addr("[::]:12101"), &[]));
}

#[test]
fn ipv4_mapped_nonloopback_empty_allowlist_is_insecure() {
    assert!(admin_exposure_is_insecure(&addr("[::ffff:10.1.2.3]:12101"), &[]));
}
