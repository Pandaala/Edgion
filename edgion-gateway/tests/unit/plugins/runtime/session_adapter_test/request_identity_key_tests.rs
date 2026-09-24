use super::request_identity_key_core;
use crate::ctx::NormalizedHostname;
use crate::EdgionHttpContext;
use edgion_resources::common::KeyGet;

#[test]
fn direct_peer_ip_is_distinct_from_resolved_remote_ip() {
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.direct_peer_ip = "192.0.2.10".to_string();
    ctx.request_info.remote_ip = "198.51.100.20".to_string();

    assert_eq!(
        request_identity_key_core(&ctx, &KeyGet::DirectPeerIp).as_deref(),
        Some("192.0.2.10")
    );
    assert_ne!(
        request_identity_key_core(&ctx, &KeyGet::DirectPeerIp).as_deref(),
        Some(ctx.request_info.remote_ip.as_str())
    );
}

#[test]
fn host_uses_the_frozen_normalized_hostname() {
    let mut ctx = EdgionHttpContext::default();
    ctx.request_info.hostname = NormalizedHostname::new(" API.Example.COM.:8443 ");

    assert_eq!(
        request_identity_key_core(&ctx, &KeyGet::Host).as_deref(),
        Some("api.example.com")
    );
}

#[test]
fn empty_request_identity_values_are_missing() {
    let ctx = EdgionHttpContext::default();

    assert!(request_identity_key_core(&ctx, &KeyGet::DirectPeerIp).is_none());
    assert!(request_identity_key_core(&ctx, &KeyGet::Host).is_none());
}
