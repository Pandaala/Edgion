use super::*;
use std::sync::Arc;

#[test]
fn identity_keys_preserve_raw_namespace_and_canonical_format() {
    for (namespace, name, expected) in [
        (None, "gateway", "gateway"),
        (Some(""), "gateway", "gateway"),
        (Some("first"), "gateway", "first/gateway"),
        (Some("second"), "gateway", "second/gateway"),
        (None, "", ""),
        (Some(""), "", ""),
        (Some("first"), "", "first/"),
    ] {
        let info = GatewayInfo::new(
            namespace.map(str::to_owned),
            name.to_owned(),
            None,
            None,
            None,
            None,
            Vec::new(),
            None,
            None,
        );
        assert_eq!(info.namespace(), namespace);
        assert_eq!(info.gateway_namespace(), namespace.unwrap_or(""));
        assert_eq!(info.gateway_name(), name);
        assert_eq!(info.gateway_key(), expected);
        assert_eq!(info.gateway_key(), format_gateway_key(namespace.unwrap_or(""), name));
    }
}

#[test]
fn default_identity_and_debug_remain_unchanged() {
    let info = GatewayInfo::default();
    assert_eq!(info.namespace(), None);
    assert_eq!(info.gateway_namespace(), "");
    assert_eq!(info.gateway_name(), "");
    assert_eq!(info.gateway_key(), "");
    assert_eq!(format!("{info:?}"), "GatewayInfo { namespace: None, name: \"\", listener_name: None, metrics_test_key: None, metrics_test_type: None, require_sni_host_match: None, plugin_store_keys: [], dns_resolver_candidate: None }");
}

#[test]
fn shared_generation_borrows_the_same_key_storage() {
    let info = Arc::new(GatewayInfo::new(
        Some("team".to_owned()),
        "gateway".to_owned(),
        Some("http".to_owned()),
        Some("test-key".to_owned()),
        None,
        Some(true),
        vec!["team/plugin".to_owned()],
        None,
        None,
    ));
    let shared = Arc::clone(&info);
    assert_eq!(info.gateway_key().as_ptr(), info.gateway_key().as_ptr());
    assert_eq!(info.gateway_key().as_ptr(), shared.gateway_key().as_ptr());
    assert_eq!(format!("{info:?}"), "GatewayInfo { namespace: Some(\"team\"), name: \"gateway\", listener_name: Some(\"http\"), metrics_test_key: Some(\"test-key\"), metrics_test_type: None, require_sni_host_match: Some(true), plugin_store_keys: [\"team/plugin\"], dns_resolver_candidate: None }");
}
