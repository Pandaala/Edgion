#[test]
fn request_id_held_head_replacement_preserves_custom_echo_and_suppression() {
    let mut ids = crate::request_trace_id::RequestTraceId::default();
    ids.initialize(&http::HeaderMap::new());
    ids.apply_default_patch(&edgion_resources::DefaultIdConfig {
        enabled: Some(false),
        ..Default::default()
    });
    let rule: edgion_resources::RequestIdRule = serde_json::from_value(serde_json::json!({
        "header":"x-correlation-id", "source":{"type":"traceId"}
    }))
    .unwrap();
    let compiled = crate::request_trace_id::CompiledRequestIdRule::new(&rule).unwrap();
    let value = http::HeaderValue::from_str(ids.standard_trace_id().unwrap()).unwrap();
    ids.apply_rule(&compiled, None, Some(value.clone()));
    let mut original = pingora_http::ResponseHeader::build(200, None).unwrap();
    original.insert_header("x-trace-id", "stale").unwrap();
    let owner = super::AttachedGuardrail::new();
    owner.prepare_head_replacement(&original, &ids);
    let replacement = owner.replacement_header.get().unwrap();
    assert_eq!(replacement.headers["x-correlation-id"], value);
    assert!(!replacement.headers.contains_key("x-trace-id"));
    assert!(!replacement.headers.contains_key("traceparent"));
    assert_eq!(replacement.status.as_u16(), 403);
}

use super::*;

#[test]
fn summary_is_emitted_exactly_once() {
    let owner = AttachedGuardrail::new();
    owner.publish(GuardrailSummary {
        windows: 2,
        outcome: "pass",
        reason: "normal-eos",
        ..GuardrailSummary::default()
    });
    let first = owner.take_summary_line().unwrap();
    assert!(first.contains("windows=2"));
    assert!(owner.take_summary_line().is_none());
}
