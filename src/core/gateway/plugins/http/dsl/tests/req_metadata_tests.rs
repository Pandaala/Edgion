//! Tests for request metadata builtins: protocol, grpc_service, grpc_method, content_length, client cert.

use super::*;
use crate::types::ctx::ClientCertInfo;

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
        Some(ClientCertInfo {
            subject: "CN=my-service".to_string(),
            sans: vec![],
            cn: Some("my-service".to_string()),
            fingerprint: "abc123".to_string(),
        })
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
        Some(ClientCertInfo {
            subject: "CN=svc".to_string(),
            sans: vec![],
            cn: Some("svc".to_string()),
            fingerprint: "sha256:deadbeef".to_string(),
        })
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
