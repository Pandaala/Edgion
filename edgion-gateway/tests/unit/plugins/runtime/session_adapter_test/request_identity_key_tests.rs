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

#[tokio::test]
async fn entry_method_accessor_remains_borrowed_and_distinct_from_current_identity() {
    use super::record_outcome_tests::test_session;
    use super::PingoraSessionAdapter;
    use crate::plugins::runtime::traits::PluginSession;

    for (entry, current) in [("GET", "POST"), ("POST", "GET"), ("REPORT", "PATCH")] {
        let mut session = test_session().await;
        let mut ctx = EdgionHttpContext::default();
        ctx.request_info.method = Some(http::Method::from_bytes(entry.as_bytes()).unwrap());
        ctx.request_info.entry_raw_path = "/dye-test".into();
        ctx.request_info.normalized_current_path = "/dye-test".into();
        ctx.request_info.hostname = NormalizedHostname::new("entry.example:8080");
        let mut adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
        assert_eq!(adapter.get_request_method(), Some(entry));
        adapter.set_upstream_method(current).unwrap();
        assert_eq!(adapter.get_request_method(), Some(entry));
        assert_eq!(adapter.get_method(), current);
        assert_eq!(adapter.key_get(&KeyGet::Method).as_deref(), Some(current));
        adapter.set_upstream_uri("/effective?key=current").unwrap();
        assert_eq!(adapter.get_path(), "/effective");
        assert_eq!(adapter.get_uri(), "/effective?key=current");
        assert_eq!(adapter.get_query().as_deref(), Some("key=current"));
        assert_eq!(adapter.get_request_path(), "/dye-test");
        assert_eq!(adapter.get_request_uri(), "/dye-test");
        assert_eq!(adapter.get_query_param("key"), None);
        assert_eq!(adapter.key_get(&KeyGet::Host).as_deref(), Some("entry.example"));
    }
    let mut session = test_session().await;
    let mut ctx = EdgionHttpContext::default();
    let adapter = PingoraSessionAdapter::new(&mut session, &mut ctx);
    assert_eq!(adapter.get_method(), "GET");
    assert!(adapter.get_request_method().is_none());
}
