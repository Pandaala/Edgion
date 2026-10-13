use super::*;

fn valid_base() -> ExtProcConfig {
    ExtProcConfig {
        grpc_service: GrpcServiceConfig {
            target: "ext-proc:9000".to_string(),
            timeout_ms: 200,
            tls: TlsConfig::default(),
        },
        ..Default::default()
    }
}

#[test]
fn validation_error_returns_field_error_without_validate_call() {
    // Default config has an empty target — must be rejected statelessly.
    let config = ExtProcConfig::default();
    assert!(config.validation_error().is_some());
}

#[test]
fn rejects_empty_target() {
    let config = ExtProcConfig::default();
    assert_eq!(
        config.validation_error().as_deref(),
        Some("grpcService.target is required")
    );
}

#[test]
fn rejects_url_target() {
    let mut config = valid_base();
    config.grpc_service.target = "http://ext-proc:9000".to_string();
    assert!(config.validation_error().unwrap().contains("not a URL"));
}

#[test]
fn rejects_target_without_port() {
    let mut config = valid_base();
    config.grpc_service.target = "ext-proc".to_string();
    assert!(config.validation_error().unwrap().contains("host:port"));
}

#[test]
fn rejects_malformed_targets() {
    for bad in [
        ":9000",
        "host:",
        "host:9000/path",
        "host:abc",
        "host:0",
        "host:70000",
        "[::1",
    ] {
        let mut config = valid_base();
        config.grpc_service.target = bad.to_string();
        assert!(
            config.validation_error().is_some(),
            "expected target {bad:?} to be rejected"
        );
    }
}

#[test]
fn accepts_ipv6_target() {
    let mut config = valid_base();
    config.grpc_service.target = "[::1]:9000".to_string();
    assert!(config.validation_error().is_none(), "IPv6 host:port should be valid");
}

#[test]
fn rejects_zero_timeout() {
    let mut config = valid_base();
    config.grpc_service.timeout_ms = 0;
    assert!(config.validation_error().unwrap().contains("timeoutMs"));
}

#[test]
fn rejects_timeout_over_ceiling() {
    let mut config = valid_base();
    config.grpc_service.timeout_ms = MAX_TIMEOUT_MS + 1;
    assert!(config.validation_error().unwrap().contains("timeoutMs"));
    // At the ceiling is allowed.
    config.grpc_service.timeout_ms = MAX_TIMEOUT_MS;
    assert!(config.validation_error().is_none());
}

#[test]
fn accepts_streamed_request_body() {
    let mut config = valid_base();
    config.processing_mode.request_body_mode = BodySendMode::Streamed;
    assert!(config.validation_error().is_none());
}

#[test]
fn accepts_streamed_response_body() {
    let mut config = valid_base();
    config.processing_mode.response_body_mode = BodySendMode::Streamed;
    assert!(config.validation_error().is_none());
}

#[test]
fn accepts_buffered_request_body() {
    let mut config = valid_base();
    config.processing_mode.request_body_mode = BodySendMode::Buffered;
    assert!(config.validation_error().is_none());
}

#[test]
fn rejects_buffered_response_body() {
    let mut config = valid_base();
    config.processing_mode.response_body_mode = BodySendMode::Buffered;
    let err = config.validation_error().unwrap();
    assert!(err.contains("responseBodyMode"), "got: {err}");
}

#[test]
fn rejects_buffered_partial_response_body() {
    let mut config = valid_base();
    config.processing_mode.response_body_mode = BodySendMode::BufferedPartial;
    let err = config.validation_error().unwrap();
    assert!(err.contains("responseBodyMode"), "got: {err}");
}

#[test]
fn accepts_valid_header_only_config() {
    let mut config = valid_base();
    config.processing_mode.response_header_mode = HeaderSendMode::Send;
    assert!(config.validation_error().is_none());
}

#[test]
fn deserializes_defaults() {
    let json = r#"{"grpcService":{"target":"ext-proc:9000"}}"#;
    let config: ExtProcConfig = serde_json::from_str(json).unwrap();
    assert_eq!(config.grpc_service.timeout_ms, 200);
    assert!(!config.grpc_service.tls.enabled);
    assert_eq!(config.processing_mode.request_header_mode, HeaderSendMode::Send);
    assert_eq!(config.processing_mode.response_header_mode, HeaderSendMode::Skip);
    assert_eq!(config.processing_mode.response_trailer_mode, HeaderSendMode::Skip);
    assert!(!config.failure_mode_allow);
    assert!(!config.disable_immediate_response);
    assert!(!config.allow_mode_override);
    assert!(config.allowed_override_modes.is_empty());
}

#[test]
fn deserializes_disable_immediate_response() {
    let config: ExtProcConfig = serde_json::from_value(serde_json::json!({
        "grpcService": {"target": "ext-proc:9000"},
        "disableImmediateResponse": true
    }))
    .unwrap();
    assert!(config.disable_immediate_response);
}

#[test]
fn rejects_unknown_extproc_keys_at_each_closed_boundary() {
    for json in [
        serde_json::json!({
            "grpcService": {"target": "ext-proc:9000"},
            "disableImmediateReponse": true
        }),
        serde_json::json!({
            "grpcService": {"target": "ext-proc:9000", "timeoutMillis": 10}
        }),
        serde_json::json!({
            "grpcService": {"target": "ext-proc:9000"},
            "processingMode": {"responseHeadersMode": "SEND"}
        }),
        serde_json::json!({
            "grpcService": {"target": "ext-proc:9000"},
            "allowModeOverride": true,
            "allowedOverrideModes": [{"responseBodyModes": "NONE"}]
        }),
    ] {
        assert!(serde_json::from_value::<ExtProcConfig>(json).is_err());
    }
}

#[test]
fn extproc_grpc_accepts_verify_and_validation() {
    let c: GrpcServiceConfig = serde_json::from_value(serde_json::json!({
            "target":"ep:9000","tls":{"enabled":true,"verify":false,"validation":{"caCertificateRefs":[{"kind":"Secret","name":"ca"}]}}
        }))
        .unwrap();
    assert!(c.tls.enabled);
    assert_eq!(c.tls.outbound.verify, Some(false));
}

#[test]
fn deserializes_screaming_snake_modes() {
    let json = r#"{
            "grpcService":{"target":"ext-proc:9000","timeoutMs":150,"tls":{"enabled":true}},
            "processingMode":{
                "requestHeaderMode":"SEND",
                "responseHeaderMode":"SEND",
                "requestBodyMode":"SKIP",
                "responseBodyMode":"NONE"
            },
            "failureModeAllow":true
        }"#;
    let config: ExtProcConfig = serde_json::from_str(json).unwrap();
    assert!(config.grpc_service.tls.enabled);
    assert_eq!(config.grpc_service.timeout_ms, 150);
    assert_eq!(config.processing_mode.response_header_mode, HeaderSendMode::Send);
    assert!(config.failure_mode_allow);
    assert!(config.validation_error().is_none());
}

#[test]
fn override_allow_list_requires_enabled_bounded_nonempty_configuration() {
    let mut config = valid_base();
    config.allowed_override_modes.push(ProcessingMode::default());
    assert!(config.validation_error().unwrap().contains("must be empty"));

    config.allow_mode_override = true;
    config.allowed_override_modes.clear();
    assert!(config.validation_error().unwrap().contains("1 through 8"));

    config.allowed_override_modes = vec![ProcessingMode::default(); 9];
    assert!(config.validation_error().unwrap().contains("1 through 8"));
}

#[test]
fn override_allow_list_rejects_normalized_duplicates() {
    let mut config = valid_base();
    config.allow_mode_override = true;
    let first = ProcessingMode {
        request_header_mode: HeaderSendMode::Send,
        request_body_mode: BodySendMode::Skip,
        ..ProcessingMode::default()
    };
    let mut duplicate = first.clone();
    duplicate.request_header_mode = HeaderSendMode::Skip;
    duplicate.request_body_mode = BodySendMode::None;
    config.allowed_override_modes = vec![first, duplicate];
    assert!(config.validation_error().unwrap().contains("duplicates"));
}

#[test]
fn override_allow_list_accepts_only_supported_reductions() {
    let mut config = valid_base();
    config.processing_mode.request_body_mode = BodySendMode::Buffered;
    config.processing_mode.response_header_mode = HeaderSendMode::Send;
    config.processing_mode.response_body_mode = BodySendMode::Streamed;
    config.allow_mode_override = true;
    config.allowed_override_modes = vec![ProcessingMode {
        request_header_mode: HeaderSendMode::Skip,
        response_header_mode: HeaderSendMode::Skip,
        request_body_mode: BodySendMode::None,
        response_body_mode: BodySendMode::None,
        response_trailer_mode: HeaderSendMode::Skip,
    }];
    assert!(config.validation_error().is_none());

    for invalid in [
        ProcessingMode {
            request_body_mode: BodySendMode::Streamed,
            ..config.processing_mode.clone()
        },
        ProcessingMode {
            response_body_mode: BodySendMode::Buffered,
            ..config.processing_mode.clone()
        },
    ] {
        config.allowed_override_modes = vec![invalid];
        assert!(config.validation_error().is_some());
    }
}

#[test]
fn base_response_trailer_send_is_supported() {
    let mut config = valid_base();
    config.processing_mode.response_trailer_mode = HeaderSendMode::Send;
    assert!(config.validation_error().is_none());
}

#[test]
fn rejects_targets_unrepresentable_as_complete_channel_uris() {
    for target in ["ext proc:9000", "ext\tproc:9000", "ext\rproc:9000", "ext\nproc:9000"] {
        let mut config = valid_base();
        config.grpc_service.target = target.to_string();
        assert_eq!(
            config.validation_error().as_deref(),
            Some("grpcService.target cannot form a valid channel URI")
        );
    }
    let max_host_len = usize::from(u16::MAX) - 1 - "http://".len() - ":9000".len();
    for (host_len, valid) in [(max_host_len, true), (max_host_len + 1, false)] {
        let mut config = valid_base();
        config.grpc_service.target = format!("{}:9000", "a".repeat(host_len));
        assert_eq!(config.validation_error().is_none(), valid);
    }
}

#[test]
fn effective_target_preserves_connector_host_and_existing_normalization() {
    for (raw, host, authority) in [
        ("EXT-PROC:+09000", "EXT-PROC", "ext-proc:9000"),
        ("svc_alias:9000", "svc_alias", "svc_alias:9000"),
        ("192.0.2.1:9000", "192.0.2.1", "192.0.2.1:9000"),
        ("[2001:0DB8::1]:9000", "2001:0DB8::1", "[2001:db8::1]:9000"),
        ("2001:0DB8::1:9000", "2001:0DB8::1", "[2001:db8::1]:9000"),
        ("[ALIAS]:9000", "ALIAS", "alias:9000"),
    ] {
        let mut config = valid_base();
        config.grpc_service.target = raw.to_string();
        assert!(config.validation_error().is_none());
        let parsed = config.grpc_service.parse_target().unwrap();
        assert_eq!(parsed.host, host);
        assert_eq!(parsed.port, 9000);
        assert_eq!(parsed.authority, authority);
        assert_eq!(config.grpc_service.target, raw);
    }
}
