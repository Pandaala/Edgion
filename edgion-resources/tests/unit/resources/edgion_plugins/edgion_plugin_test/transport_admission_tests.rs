use super::*;
use serde_json::json;

#[test]
fn typed_ai_conversion_category_excludes_other_transport_plugins() {
    for (wire, expected) in [
        (json!({"type":"AiProxy","config":{"api":"openaiChat"}}), true),
        (json!({"type":"AiGuard","config":{}}), true),
        (
            json!({"type":"Guardrail","config":{"target":{"name":"guard","port":8080},"request":{"path":{"template":"/decision"}}}}),
            true,
        ),
        (
            json!({"type":"ExtProc","config":{"grpcService":{"target":"processor.example:9000"}}}),
            false,
        ),
        (json!({"type":"BandwidthLimit","config":{"rate":"100kb"}}), false),
        (
            json!({"type":"Wasm","config":{"source":{"file":"missing.wasm"}}}),
            false,
        ),
        (
            json!({"type":"GrpcJsonTranscoder","config":{"descriptor":{"type":"configMapRef","name":"schema","key":"test.pb"},"services":["test.TestService"]}}),
            false,
        ),
        (json!({"type":"RequestHeaderModifier","config":{}}), false),
    ] {
        let plugin: EdgionPlugin = serde_json::from_value(wire).unwrap();
        assert_eq!(plugin.is_ai_conversion_owner(), expected, "{}", plugin.type_name());
    }
    assert!(!EdgionPlugin::Unsupported.is_ai_conversion_owner());
}
