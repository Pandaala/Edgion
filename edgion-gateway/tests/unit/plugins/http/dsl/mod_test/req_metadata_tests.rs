//! Tests for request metadata builtins: protocol, grpc_service, grpc_method, content_length, client cert.

use super::*;
use crate::ctx::ClientCertInfo;

#[test]
fn test_req_protocol_default() {
    let source = r#"req.set_header("X-Proto", req.protocol)"#;
    let mut session = MockPluginSession::new();
    session.expect_discover_protocol().returning(|| "http".to_string());
    session
        .expect_set_request_header()
        .withf(|n, v| n == "X-Proto" && v == "http")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(source, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_req_protocol_grpc() {
    let source = r#"
        if req.protocol == "grpc" {
            req.set_header("X-Type", "grpc")
        }
    "#;
    let mut session = MockPluginSession::new();
    session.expect_discover_protocol().returning(|| "grpc".to_string());
    session
        .expect_set_request_header()
        .withf(|n, v| n == "X-Type" && v == "grpc")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(source, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_req_grpc_service() {
    let source = r#"
        let svc = req.grpc_service
        if svc != nil {
            req.set_header("X-Service", svc)
        }
    "#;
    let mut session = MockPluginSession::new();
    session
        .expect_grpc_service()
        .returning(|| Some("mypackage.MyService".to_string()));
    session
        .expect_set_request_header()
        .withf(|n, v| n == "X-Service" && v == "mypackage.MyService")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(source, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_req_grpc_method() {
    let source = r#"
        let m = req.grpc_method
        if m == nil {
            req.set_header("X-Has-Method", "no")
        }
    "#;
    let mut session = MockPluginSession::new();
    session.expect_grpc_method().returning(|| None);
    session
        .expect_set_request_header()
        .withf(|n, v| n == "X-Has-Method" && v == "no")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(source, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_req_content_length() {
    let source = r#"
        if req.content_length > 1048576 {
            return deny(413, "too large")
        }
    "#;
    let mut session = MockPluginSession::new();
    session
        .expect_header_value()
        .withf(|n| n == "Content-Length")
        .returning(|_| Some("2097152".to_string()));
    let result = run(source, session);
    assert_eq!(result, err_response_body(413, "too large"));
}

#[test]
fn test_req_content_length_missing() {
    let source = r#"
        if req.content_length == nil {
            req.set_header("X-Has-CL", "no")
        }
    "#;
    let mut session = MockPluginSession::new();
    session
        .expect_header_value()
        .withf(|n| n == "Content-Length")
        .returning(|_| None);
    session
        .expect_set_request_header()
        .withf(|n, v| n == "X-Has-CL" && v == "no")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(source, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_req_client_cert_cn() {
    let source = r#"
        let cn = req.client_cert_cn
        if cn == "my-service" {
            req.set_header("X-Auth", "cert-ok")
        }
    "#;
    let mut session = MockPluginSession::new();
    session.expect_client_cert_info().returning(|| {
        static CERT: std::sync::OnceLock<ClientCertInfo> = std::sync::OnceLock::new();
        Some(CERT.get_or_init(|| ClientCertInfo {
            subject: "CN=my-service".to_string(),
            sans: vec![],
            cn: Some("my-service".to_string()),
            fingerprint: "abc123".to_string(),
        }))
    });
    session
        .expect_set_request_header()
        .withf(|n, v| n == "X-Auth" && v == "cert-ok")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(source, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_req_client_cert_fingerprint() {
    let source = r#"req.set_header("X-FP", req.client_cert_fingerprint)"#;
    let mut session = MockPluginSession::new();
    session.expect_client_cert_info().returning(|| {
        static CERT: std::sync::OnceLock<ClientCertInfo> = std::sync::OnceLock::new();
        Some(CERT.get_or_init(|| ClientCertInfo {
            subject: "CN=svc".to_string(),
            sans: vec![],
            cn: Some("svc".to_string()),
            fingerprint: "sha256:deadbeef".to_string(),
        }))
    });
    session
        .expect_set_request_header()
        .withf(|n, v| n == "X-FP" && v == "sha256:deadbeef")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(source, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_req_client_cert_cn_no_cert() {
    let source = r#"
        if req.client_cert_cn == nil {
            return deny(403, "cert required")
        }
    "#;
    let mut session = MockPluginSession::new();
    session.expect_client_cert_info().returning(|| None);
    let result = run(source, session);
    assert_eq!(result, err_response_body(403, "cert required"));
}

#[test]
fn test_req_metadata_string_byte_limits() {
    for (property, value) in [
        ("protocol", "grpc-web"),
        ("grpc_service", "example.Service"),
        ("grpc_method", "SayHello"),
        ("client_cert_cn", "éñ"),
        ("client_cert_fingerprint", "sha256:deadbeef"),
    ] {
        for limit in [value.len(), value.len() - 1] {
            let mut session = MockPluginSession::new();
            match property {
                "protocol" => {
                    session
                        .expect_discover_protocol()
                        .times(1)
                        .returning(move || value.to_string());
                }
                "grpc_service" => {
                    session
                        .expect_grpc_service()
                        .times(1)
                        .returning(move || Some(value.to_string()));
                }
                "grpc_method" => {
                    session
                        .expect_grpc_method()
                        .times(1)
                        .returning(move || Some(value.to_string()));
                }
                "client_cert_cn" => {
                    session.expect_client_cert_info().times(1).returning(move || {
                        static CERT: std::sync::OnceLock<ClientCertInfo> = std::sync::OnceLock::new();
                        Some(CERT.get_or_init(|| ClientCertInfo {
                            subject: "CN=test".to_string(),
                            sans: vec![],
                            cn: Some(value.to_string()),
                            fingerprint: "unrelated-fingerprint".to_string(),
                        }))
                    });
                }
                "client_cert_fingerprint" => {
                    session.expect_client_cert_info().times(1).returning(move || {
                        static CERT: std::sync::OnceLock<ClientCertInfo> = std::sync::OnceLock::new();
                        Some(CERT.get_or_init(|| ClientCertInfo {
                            subject: "CN=test".to_string(),
                            sans: vec![],
                            cn: Some("unrelated-cn".to_string()),
                            fingerprint: value.to_string(),
                        }))
                    });
                }
                _ => unreachable!("the table contains only the five string metadata getters"),
            }
            let limits = VmLimits {
                max_string_len: limit,
                ..VmLimits::default()
            };
            if limit == value.len() {
                session
                    .expect_set_request_header()
                    .withf(move |name, actual| name == "X" && actual == value)
                    .times(1)
                    .returning(|_, _| Ok(()));
                let source = format!(r#"req.set_header("X", req.{property})"#);
                assert_eq!(run_with_limits(&source, session, limits), good_next(), "{property}");
            } else {
                let error = run_expect_runtime_err_with_limits(&format!("req.{property}"), session, limits);
                assert!(
                    matches!(error, RuntimeError::StringTooLong { len, limit: actual_limit }
                        if len == value.len() && actual_limit == limit),
                    "{property}: {error:?}"
                );
            }
        }
    }
}

#[test]
fn test_req_grpc_method_present_service_missing() {
    let source = r#"
        let method = req.grpc_method
        let service = req.grpc_service
        if service == nil {
            req.set_header("X-Method", method)
        }
    "#;
    let mut session = MockPluginSession::new();
    session
        .expect_grpc_method()
        .times(1)
        .returning(|| Some("SayHello".to_string()));
    session.expect_grpc_service().times(1).returning(|| None);
    session
        .expect_set_request_header()
        .withf(|name, value| name == "X-Method" && value == "SayHello")
        .times(1)
        .returning(|_, _| Ok(()));
    assert_eq!(run(source, session), good_next());
}

#[test]
fn test_req_client_cert_present_without_cn() {
    let source = r#"
        if req.client_cert_cn == nil {
            req.set_header("X-CN", "missing")
        }
    "#;
    let mut session = MockPluginSession::new();
    session.expect_client_cert_info().times(1).returning(|| {
        static CERT: std::sync::OnceLock<ClientCertInfo> = std::sync::OnceLock::new();
        Some(CERT.get_or_init(|| ClientCertInfo {
            subject: "O=example".to_string(),
            sans: vec!["service.example".to_string()],
            cn: None,
            fingerprint: "sha256:deadbeef".to_string(),
        }))
    });
    session
        .expect_set_request_header()
        .withf(|name, value| name == "X-CN" && value == "missing")
        .times(1)
        .returning(|_, _| Ok(()));
    assert_eq!(run(source, session), good_next());
}

#[test]
fn test_req_client_cert_fingerprint_no_cert() {
    let source = r#"
        if req.client_cert_fingerprint == nil {
            req.set_header("X-FP", "missing")
        }
    "#;
    let mut session = MockPluginSession::new();
    session.expect_client_cert_info().times(1).returning(|| None);
    session
        .expect_set_request_header()
        .withf(|name, value| name == "X-FP" && value == "missing")
        .times(1)
        .returning(|_, _| Ok(()));
    assert_eq!(run(source, session), good_next());
}

#[test]
fn test_req_content_length_malformed_or_overflow_returns_nil() {
    for header in ["invalid", "9223372036854775808"] {
        let source = r#"
            if req.content_length == nil {
                req.set_header("X-CL", "invalid")
            }
        "#;
        let mut session = MockPluginSession::new();
        session
            .expect_header_value()
            .withf(|name| name == "Content-Length")
            .times(1)
            .returning(move |_| Some(header.to_string()));
        session
            .expect_set_request_header()
            .withf(|name, value| name == "X-CL" && value == "invalid")
            .times(1)
            .returning(|_, _| Ok(()));
        assert_eq!(run(source, session), good_next(), "{header}");
    }
}

#[test]
fn test_req_content_length_zero_returns_integer() {
    let source = r#"
        if req.content_length == 0 {
            req.set_header("X-CL", "zero")
        }
    "#;
    let mut session = MockPluginSession::new();
    session
        .expect_header_value()
        .withf(|name| name == "Content-Length")
        .times(1)
        .returning(|_| Some("0".to_string()));
    session
        .expect_set_request_header()
        .withf(|name, value| name == "X-CL" && value == "zero")
        .times(1)
        .returning(|_, _| Ok(()));
    assert_eq!(run(source, session), good_next());
}
