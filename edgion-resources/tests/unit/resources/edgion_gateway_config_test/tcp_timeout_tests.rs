use super::*;
use crate::duration::parse_duration;
use std::time::Duration;

#[test]
fn tcp_timeout_absent_section_is_none() {
    let spec: EdgionGatewayConfigSpec = serde_json::from_str("{}").unwrap();
    assert!(spec.tcp_timeout.is_none());
}

#[test]
fn tcp_timeout_idle_defaults_to_1h_when_field_omitted() {
    let spec: EdgionGatewayConfigSpec = serde_json::from_str(r#"{"tcpTimeout": {}}"#).unwrap();
    assert_eq!(spec.tcp_timeout.unwrap().idle_timeout, "1h");
}

#[test]
fn tcp_timeout_idle_explicit_value_round_trips() {
    let spec: EdgionGatewayConfigSpec = serde_json::from_str(r#"{"tcpTimeout": {"idleTimeout": "300s"}}"#).unwrap();
    assert_eq!(spec.tcp_timeout.unwrap().idle_timeout, "300s");
}

#[test]
fn default_spec_has_no_tcp_timeout() {
    assert!(EdgionGatewayConfigSpec::default().tcp_timeout.is_none());
}

#[test]
fn resolve_absent_section_uses_secure_default() {
    assert_eq!(resolve_tcp_idle_timeout(None), Some(DEFAULT_TCP_IDLE));
}

#[test]
fn resolve_omitted_field_uses_serde_default_1h() {
    let spec: EdgionGatewayConfigSpec = serde_json::from_str(r#"{"tcpTimeout": {}}"#).unwrap();
    assert_eq!(
        resolve_tcp_idle_timeout(spec.tcp_timeout.as_ref()),
        Some(Duration::from_secs(3600))
    );
}

#[test]
fn resolve_explicit_value() {
    let t = TcpTimeout {
        idle_timeout: "300s".to_string(),
        connect_timeout: default_tcp_connect_timeout(),
    };
    assert_eq!(resolve_tcp_idle_timeout(Some(&t)), Some(Duration::from_secs(300)));
}

#[test]
fn resolve_zero_is_unbounded_optout() {
    // Any zero-valued GEP-2257 duration opts out; a bare "0" is not a
    // valid duration (see resolve_parse_error_fails_safe_to_default).
    for s in ["0s", "0m", "0h", "0ms"] {
        let t = TcpTimeout {
            idle_timeout: s.to_string(),
            connect_timeout: default_tcp_connect_timeout(),
        };
        assert_eq!(resolve_tcp_idle_timeout(Some(&t)), None, "{s} should opt out");
    }
}

#[test]
fn resolve_parse_error_fails_safe_to_default() {
    // "0" (bare number) and "0.0s" (decimal) are NOT valid GEP-2257 and
    // must NOT opt out — they fail safe to the bounded default.
    for s in ["not-a-duration", "0", "0.0s"] {
        let t = TcpTimeout {
            idle_timeout: s.to_string(),
            connect_timeout: default_tcp_connect_timeout(),
        };
        assert_eq!(
            resolve_tcp_idle_timeout(Some(&t)),
            Some(DEFAULT_TCP_IDLE),
            "{s} should fail safe to default"
        );
    }
}

#[test]
fn default_const_and_serde_default_stay_in_sync() {
    assert_eq!(parse_duration(&default_tcp_idle_timeout()).unwrap(), DEFAULT_TCP_IDLE);
}

// --- connect_timeout (mirrors the idle_timeout cases above) ---

#[test]
fn tcp_timeout_connect_defaults_to_10s_when_field_omitted() {
    let spec: EdgionGatewayConfigSpec = serde_json::from_str(r#"{"tcpTimeout": {}}"#).unwrap();
    assert_eq!(spec.tcp_timeout.unwrap().connect_timeout, "10s");
}

#[test]
fn tcp_timeout_connect_explicit_value_round_trips() {
    let spec: EdgionGatewayConfigSpec = serde_json::from_str(r#"{"tcpTimeout": {"connectTimeout": "3s"}}"#).unwrap();
    assert_eq!(spec.tcp_timeout.unwrap().connect_timeout, "3s");
}

#[test]
fn resolve_connect_absent_section_uses_secure_default() {
    assert_eq!(resolve_tcp_connect_timeout(None), Some(DEFAULT_TCP_CONNECT));
}

#[test]
fn resolve_connect_omitted_field_uses_serde_default_10s() {
    let spec: EdgionGatewayConfigSpec = serde_json::from_str(r#"{"tcpTimeout": {}}"#).unwrap();
    assert_eq!(
        resolve_tcp_connect_timeout(spec.tcp_timeout.as_ref()),
        Some(Duration::from_secs(10))
    );
}

#[test]
fn resolve_connect_explicit_value() {
    let t = TcpTimeout {
        idle_timeout: default_tcp_idle_timeout(),
        connect_timeout: "3s".to_string(),
    };
    assert_eq!(resolve_tcp_connect_timeout(Some(&t)), Some(Duration::from_secs(3)));
}

#[test]
fn resolve_connect_zero_is_unbounded_optout() {
    // Any zero-valued GEP-2257 duration opts out; a bare "0" is not a
    // valid duration (see resolve_connect_parse_error_fails_safe_to_default).
    for s in ["0s", "0m", "0h", "0ms"] {
        let t = TcpTimeout {
            idle_timeout: default_tcp_idle_timeout(),
            connect_timeout: s.to_string(),
        };
        assert_eq!(resolve_tcp_connect_timeout(Some(&t)), None, "{s} should opt out");
    }
}

#[test]
fn resolve_connect_parse_error_fails_safe_to_default() {
    // "0" (bare number) and "0.0s" (decimal) are NOT valid GEP-2257 and
    // must NOT opt out — they fail safe to the bounded default.
    for s in ["not-a-duration", "0", "0.0s"] {
        let t = TcpTimeout {
            idle_timeout: default_tcp_idle_timeout(),
            connect_timeout: s.to_string(),
        };
        assert_eq!(
            resolve_tcp_connect_timeout(Some(&t)),
            Some(DEFAULT_TCP_CONNECT),
            "{s} should fail safe to default"
        );
    }
}

#[test]
fn connect_default_const_and_serde_default_stay_in_sync() {
    assert_eq!(
        parse_duration(&default_tcp_connect_timeout()).unwrap(),
        DEFAULT_TCP_CONNECT
    );
}
