use super::*;

// ========== KeyGet Tests ==========

#[test]
fn key_get_log_labels_preserve_bytes_and_ownership() {
    for (key, expected) in [
        (KeyGet::RemoteIp, "remote-ip"),
        (KeyGet::DirectPeerIp, "direct-peer-ip"),
        (KeyGet::Host, "host"),
        (KeyGet::TraceId, "trace-id"),
        (KeyGet::Path, "path"),
        (KeyGet::Method, "method"),
        (KeyGet::RemoteIpAndPath, "remote-ip+path"),
        (KeyGet::RequestPath, "request-path"),
    ] {
        let label = key.as_log_str();
        assert_eq!(label, expected);
        assert!(matches!(label, Cow::Borrowed(_)));
    }
    for name in ["", "X-Mixed-Case", "café"] {
        for (key, prefix) in [
            (KeyGet::Header { name: name.into() }, "hdr:"),
            (KeyGet::Cookie { name: name.into() }, "cookie:"),
            (KeyGet::Query { name: name.into() }, "query:"),
            (KeyGet::Ctx { name: name.into() }, "ctx:"),
        ] {
            let label = key.as_log_str();
            drop(key);
            assert_eq!(label, format!("{prefix}{name}"));
            assert!(matches!(label, Cow::Owned(_)));
        }
    }
}

#[test]
fn test_key_get_as_log_str() {
    assert_eq!(KeyGet::RemoteIp.as_log_str(), "remote-ip");
    assert_eq!(KeyGet::DirectPeerIp.as_log_str(), "direct-peer-ip");
    assert_eq!(KeyGet::Host.as_log_str(), "host");
    assert_eq!(
        KeyGet::Header {
            name: "X-Api-Key".to_string()
        }
        .as_log_str(),
        "hdr:X-Api-Key"
    );
    assert_eq!(KeyGet::Path.as_log_str(), "path");
    assert_eq!(KeyGet::RemoteIpAndPath.as_log_str(), "remote-ip+path");
    assert_eq!(KeyGet::RequestPath.as_log_str(), "request-path");
}

#[test]
fn test_key_get_default() {
    assert_eq!(KeyGet::default(), KeyGet::RemoteIp);
}

#[test]
fn test_key_get_source_type() {
    assert_eq!(KeyGet::RemoteIp.source_type(), "remoteIp");
    assert_eq!(KeyGet::DirectPeerIp.source_type(), "directPeerIp");
    assert_eq!(KeyGet::Host.source_type(), "host");
    assert_eq!(
        KeyGet::Header {
            name: "X-Test".to_string()
        }
        .source_type(),
        "header"
    );
    assert_eq!(KeyGet::Path.source_type(), "path");
    assert_eq!(KeyGet::RequestPath.source_type(), "requestPath");
}

#[test]
fn test_key_get_name() {
    assert_eq!(KeyGet::RemoteIp.name(), None);
    assert_eq!(
        KeyGet::Header {
            name: "X-Test".to_string()
        }
        .name(),
        Some("X-Test")
    );
    assert_eq!(
        KeyGet::Ctx {
            name: "user_id".to_string()
        }
        .name(),
        Some("user_id")
    );
}

// ========== KeySet Tests ==========

#[test]
fn test_key_set_as_log_str() {
    assert_eq!(
        KeySet::Header {
            name: "X-Test".to_string()
        }
        .as_log_str(),
        "hdr:X-Test"
    );
    assert_eq!(
        KeySet::ResponseHeader {
            name: "X-Time".to_string()
        }
        .as_log_str(),
        "res_hdr:X-Time"
    );
    assert_eq!(
        KeySet::Ctx {
            name: "user_id".to_string()
        }
        .as_log_str(),
        "ctx:user_id"
    );
}

#[test]
fn test_key_set_name() {
    assert_eq!(
        KeySet::Header {
            name: "X-Test".to_string()
        }
        .name(),
        "X-Test"
    );
    assert_eq!(
        KeySet::Ctx {
            name: "user_id".to_string()
        }
        .name(),
        "user_id"
    );
}

// ========== Serde Tests ==========

#[test]
fn test_key_get_serde_header() {
    let key = KeyGet::Header {
        name: "X-Api-Key".to_string(),
    };

    let json = serde_json::to_string(&key).unwrap();
    assert!(json.contains("\"type\":\"header\""));
    assert!(json.contains("\"name\":\"X-Api-Key\""));

    let deserialized: KeyGet = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized, key);
}

#[test]
fn test_key_get_serde_remote_ip() {
    let key = KeyGet::RemoteIp;
    let json = serde_json::to_string(&key).unwrap();
    assert_eq!(json, "{\"type\":\"remoteIp\"}");

    let deserialized: KeyGet = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized, key);
}

#[test]
fn test_key_get_serde_request_identity_sources() {
    for (key, expected) in [
        (KeyGet::DirectPeerIp, r#"{"type":"directPeerIp"}"#),
        (KeyGet::Host, r#"{"type":"host"}"#),
    ] {
        let json = serde_json::to_string(&key).unwrap();
        assert_eq!(json, expected);

        let deserialized: KeyGet = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, key);
    }
}

#[test]
fn test_key_get_serde_remote_ip_and_path() {
    let key = KeyGet::RemoteIpAndPath;
    let json = serde_json::to_string(&key).unwrap();
    assert_eq!(json, "{\"type\":\"remoteIpAndPath\"}");

    let deserialized: KeyGet = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized, key);
}

#[test]
fn test_key_get_serde_request_path() {
    let key = KeyGet::RequestPath;
    let json = serde_json::to_string(&key).unwrap();
    assert_eq!(json, "{\"type\":\"requestPath\"}");

    let deserialized: KeyGet = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized, key);
}

#[test]
fn test_key_set_serde() {
    let key = KeySet::ResponseHeader {
        name: "X-Response-Time".to_string(),
    };

    let json = serde_json::to_string(&key).unwrap();
    assert!(json.contains("\"type\":\"responseHeader\""));
    assert!(json.contains("\"name\":\"X-Response-Time\""));

    let deserialized: KeySet = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized, key);
}

#[test]
fn test_key_get_yaml_format() {
    let yaml = r#"
type: header
name: X-Api-Key
"#;
    let key: KeyGet = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(
        key,
        KeyGet::Header {
            name: "X-Api-Key".to_string()
        }
    );
}

#[test]
fn test_key_get_yaml_no_name() {
    let yaml = r#"
type: remoteIp
"#;
    let key: KeyGet = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(key, KeyGet::RemoteIp);
}

#[test]
fn old_client_ip_key_types_are_rejected() {
    for old in ["clientIp", "clientIpAndPath"] {
        let yaml = format!("type: {old}\n");
        assert!(serde_yaml::from_str::<KeyGet>(&yaml).is_err(), "{old} must be rejected");
    }
}

#[test]
fn test_key_set_yaml_format() {
    let yaml = r#"
type: ctx
name: user_id
"#;
    let key: KeySet = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(
        key,
        KeySet::Ctx {
            name: "user_id".to_string()
        }
    );
}
