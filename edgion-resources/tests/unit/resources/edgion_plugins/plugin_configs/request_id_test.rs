use super::*;
use serde_json::json;

#[test]
fn defaults_and_partial_patch_preserve_omissions() {
    let config: RequestIdConfig =
        serde_json::from_value(json!({"defaultId":{"enabled":false},"ids":[{"header":"x-order-id"}]})).unwrap();
    let patch = config.default_id.as_ref().unwrap();
    assert_eq!(patch.enabled, Some(false));
    assert_eq!(patch.echo_response, None);
    assert_eq!(patch.forward_upstream, None);
    assert!(config.ids[0].generate_when_missing && config.ids[0].echo_response && config.ids[0].forward_upstream);
    assert_eq!(config.ids[0].generator.kind, RequestIdGeneratorKind::UuidV4);
    assert!(config.validation_error().is_none());
}

#[test]
fn renamed_sensitive_sources_and_reserved_destinations_are_rejected() {
    for source in [
        json!({"type":"header","name":"Authorization"}),
        json!({"type":"header","name":"X-API-Key"}),
        json!({"type":"ctx","name":"jwt_claims"}),
        json!({"type":"cookie","name":"session"}),
    ] {
        let config: RequestIdConfig =
            serde_json::from_value(json!({"ids":[{"header":"x-safe-id","source":source}]})).unwrap();
        assert!(config.validation_error().is_some());
    }
    for header in [
        "Host",
        "OpenAI-Organization",
        "OPENAI-PROJECT",
        "openai-project",
        "traceparent",
        "tracestate",
        "x-trace-id",
        "X-Trace-Id",
        "Connection",
        "X-Forwarded-For",
        "Cookie",
        ":path",
        "grpc-status",
        "x-id\n",
    ] {
        assert!(!valid_request_id_header(header), "{header}");
    }
}

#[test]
fn bounds_duplicates_and_algorithm_parameters_are_enforced() {
    for ids in [
        json!([{"header":"x-id"},{"header":"X-ID"}]),
        json!([{"header":"x-id","generator":{"type":"uuidV4","bytes":16}}]),
        json!([{"header":"x-id","generator":{"type":"randomHex","bytes":7}}]),
        json!([{"header":"x-id","generator":{"type":"nanoId","length":129}}]),
        json!([{"header":"x-id","generator":{"prefix":"bad prefix"}}]),
    ] {
        let config: RequestIdConfig = serde_json::from_value(json!({"ids":ids})).unwrap();
        assert!(config.validation_error().is_some());
    }
    for bytes in [b"".as_slice(), b"a b", b"a,b", b"a\r\nb"] {
        assert!(!valid_request_id_value(bytes));
    }
    assert!(!valid_request_id_value(&[b'a'; 257]));
    assert!(valid_request_id_value(&[b'a'; 256]));
}

#[test]
fn anthropic_principal_and_protocol_headers_cannot_be_trace_destinations() {
    for name in [
        "anthropic-workspace-id",
        "Anthropic-User-Profile-Id",
        "ANTHROPIC-VERSION",
        "anthropic-beta",
    ] {
        assert!(!valid_request_id_header(name));
    }
}
