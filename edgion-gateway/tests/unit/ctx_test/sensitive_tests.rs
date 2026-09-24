use super::*;
use edgion_resources::sensitive::SENSITIVE_MODE;
use std::sync::atomic::Ordering;

fn assert_sensitive_mode_test(test_name: &str, mode: u8, assertion: impl FnOnce()) {
    const CHILD_ENV: &str = "EDGION_CTX_SENSITIVE_CHILD";
    const TEST_MODULE: &str = "ctx::sensitive_tests";
    crate::test_subprocess::assert_in_subprocess(TEST_MODULE, test_name, CHILD_ENV, test_name, || {
        SENSITIVE_MODE.store(mode, Ordering::SeqCst);
        assertion();
    });
}

#[test]
fn retry_configured_false_without_route_units() {
    let ctx = EdgionHttpContext::new();
    assert!(!ctx.retry_configured());
    assert!(ctx.peer_timeout_source.is_none());
}

#[test]
fn test_client_cert_info_wrap_mode() {
    assert_sensitive_mode_test("test_client_cert_info_wrap_mode", 2, || {
        let cert = ClientCertInfo {
            subject: "CN=alice@example.com".to_string(),
            sans: vec!["alice@example.com".to_string()],
            cn: Some("alice@example.com".to_string()),
            fingerprint: "abc123".to_string(),
        };
        let json = serde_json::to_string(&cert).unwrap();
        assert!(
            json.contains("<E>CN=alice@example.com</E>"),
            "subject not wrapped: {}",
            json
        );
        assert!(json.contains("<E>alice@example.com</E>"), "sans not wrapped: {}", json);
        assert!(json.contains("abc123"), "fingerprint should not be wrapped: {}", json);
    });
}

#[test]
fn test_client_cert_info_off_mode() {
    assert_sensitive_mode_test("test_client_cert_info_off_mode", 0, || {
        let cert = ClientCertInfo {
            subject: "CN=alice@example.com".to_string(),
            sans: vec![],
            cn: None,
            fingerprint: "abc123".to_string(),
        };
        let json = serde_json::to_string(&cert).unwrap();
        assert!(
            json.contains("CN=alice@example.com"),
            "subject should be plain in off mode: {}",
            json
        );
    });
}

#[test]
fn test_request_info_wrap_mode() {
    assert_sensitive_mode_test("test_request_info_wrap_mode", 2, || {
        let mut ri = RequestInfo {
            direct_peer_ip: "127.0.0.1".to_string(),
            remote_ip: "10.0.0.1".to_string(),
            entry_raw_path: "/api/users/alice".to_string(),
            normalized_current_path: "/api/users/alice".to_string(),
            x_forwarded_for: Some("203.0.113.5, 10.0.0.1".to_string()),
            ..Default::default()
        };
        ri.install_tls_info(None, Some("example.com".to_string()), None, None, None);
        let json = serde_json::to_string(&ri).unwrap();
        assert!(json.contains("<E>127.0.0.1</E>"), "direct_peer_ip: {}", json);
        assert!(json.contains("<E>10.0.0.1</E>"), "remote_ip: {}", json);
        assert!(json.contains("<E>/api/users/alice</E>"), "path: {}", json);
        assert!(json.contains("<E>203.0.113.5, 10.0.0.1</E>"), "xff: {}", json);
        assert!(json.contains("<E>example.com</E>"), "sni: {}", json);
    });
}

#[test]
fn test_request_info_replace_mode() {
    assert_sensitive_mode_test("test_request_info_replace_mode", 1, || {
        let ri = RequestInfo {
            direct_peer_ip: "127.0.0.1".to_string(),
            remote_ip: "10.0.0.1".to_string(),
            entry_raw_path: "/api/users/alice".to_string(),
            normalized_current_path: "/api/users/alice".to_string(),
            ..Default::default()
        };
        let json = serde_json::to_string(&ri).unwrap();
        assert!(!json.contains("127.0.0.1"), "direct_peer_ip leaked: {}", json);
        assert!(!json.contains("10.0.0.1"), "remote_ip leaked: {}", json);
        assert!(!json.contains("/api/users/alice"), "path leaked: {}", json);
        assert!(json.contains("***"), "replace_with not found: {}", json);
    });
}

#[test]
fn downstream_ip_canonicalization_collapses_ipv4_mapped_ipv6() {
    assert_eq!(
        canonicalize_ip_str("::ffff:203.0.113.7").as_deref(),
        Some("203.0.113.7")
    );
    assert_eq!(canonicalize_ip_str("2001:db8::1").as_deref(), Some("2001:db8::1"));
    assert!(canonicalize_ip_str("not-an-ip").is_none());
}

#[test]
fn body_declared_defaults_to_none_not_false() {
    // `RequestInfo` derives `Default`. A plain `bool` would default `false`,
    // which the predicate would read as "no body — run the plugin". `None`
    // means "the snapshot never ran" and fails closed instead.
    let info = RequestInfo::default();
    assert_eq!(info.body_declared, None);
}
