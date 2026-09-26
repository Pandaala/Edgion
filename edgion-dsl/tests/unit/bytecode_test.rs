#[test]
fn builtin_argument_capacity_rejects_out_of_range_wire_counts() {
    use base64::Engine;
    for count in [-1, 256, 257, 65535] {
        let payload = serde_json::json!({
            "version": super::BYTECODE_VERSION,
            "code": [{"CallBuiltin": ["LogFmt", count]}],
            "constants": [], "local_count": 0, "max_loop_depth": 0
        });
        let encoded = base64::engine::general_purpose::STANDARD.encode(payload.to_string());
        let error = super::CompiledScript::deserialize_base64(&encoded).unwrap_err();
        assert!(error.contains("u8"), "{error}");
    }
}

use super::*;

fn minimal_script() -> CompiledScript {
    CompiledScript {
        version: BYTECODE_VERSION,
        code: vec![OpCode::ReturnNext],
        constants: vec![],
        local_count: 0,
        max_loop_depth: 0,
        source: None,
        source_map: None,
    }
}

#[test]
fn test_removed_body_read_all_opcode_is_rejected() {
    let mut payload = serde_json::to_value(minimal_script()).unwrap();
    payload["code"] = serde_json::json!(["BodyReadAll", "ReturnNext"]);
    let error = serde_json::from_value::<CompiledScript>(payload).unwrap_err();
    assert!(error.to_string().contains("unknown variant `BodyReadAll`"));
}

#[test]
fn test_bytecode_roundtrip_with_version() {
    // Roundtrip with source_map absent.
    let script = minimal_script();
    let b64 = script.serialize_base64().expect("serialize should succeed");
    let decoded = CompiledScript::deserialize_base64(&b64).expect("deserialize should succeed");
    assert_eq!(decoded.version, BYTECODE_VERSION);
    assert_eq!(decoded.code, vec![OpCode::ReturnNext]);
    assert!(decoded.source_map.is_none());

    // Absent source_map must be omitted from the JSON payload (skip_serializing_if).
    let json = serde_json::to_string(&script).expect("json encode");
    assert!(!json.contains("source_map"));

    // Roundtrip with source_map present.
    let with_map = CompiledScript {
        source_map: Some(vec![(2, 1), (1, 2)]),
        ..minimal_script()
    };
    let b64 = with_map.serialize_base64().expect("serialize should succeed");
    let decoded = CompiledScript::deserialize_base64(&b64).expect("deserialize should succeed");
    assert_eq!(decoded.source_map.as_deref(), Some(&[(2u16, 1u32), (1, 2)][..]));
}

#[test]
fn test_json_select_bytecode_roundtrip() {
    let entries = vec![
        JsonSelectEntry {
            name: "id".to_string(),
            path: "$.user.id".to_string(),
        },
        JsonSelectEntry {
            name: "id_copy".to_string(),
            path: "$.user.id".to_string(),
        },
    ];
    let script = CompiledScript {
        code: vec![OpCode::JsonSelect(0), OpCode::ReturnNext],
        constants: vec![Constant::JsonSelect(entries.clone())],
        ..minimal_script()
    };
    let encoded = script.serialize_base64().expect("serialize");
    let decoded = CompiledScript::deserialize_base64(&encoded).expect("deserialize");
    assert_eq!(decoded.code, vec![OpCode::JsonSelect(0), OpCode::ReturnNext]);
    let Constant::JsonSelect(decoded_entries) = &decoded.constants[0] else {
        panic!("expected JSON select constant");
    };
    assert_eq!(decoded_entries, &entries);
}

#[test]
fn test_current_payload_decodes_without_source_map() {
    // Source maps are optional within the current bytecode version.
    use base64::Engine;
    let legacy = serde_json::json!({
        "version": BYTECODE_VERSION,
        "code": ["ReturnNext"],
        "constants": [],
        "local_count": 0,
        "max_loop_depth": 0
    });
    let bytes = serde_json::to_vec(&legacy).expect("json encode");
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    let decoded = CompiledScript::deserialize_base64(&encoded).expect("current payload should decode");
    assert!(decoded.source_map.is_none());
    assert_eq!(decoded.line_of(0), None);
}

#[test]
fn test_line_of_walks_rle_runs() {
    let script = CompiledScript {
        source_map: Some(vec![(3, 10), (2, 11), (1, 12)]),
        ..minimal_script()
    };
    // PCs 0..3 → line 10
    assert_eq!(script.line_of(0), Some(10));
    assert_eq!(script.line_of(2), Some(10));
    // PCs 3..5 → line 11
    assert_eq!(script.line_of(3), Some(11));
    assert_eq!(script.line_of(4), Some(11));
    // PC 5 → line 12
    assert_eq!(script.line_of(5), Some(12));
    // Past the end → None
    assert_eq!(script.line_of(6), None);
}

#[test]
fn test_reject_oversized_encoded_payload() {
    let encoded = "A".repeat((MAX_DECODED_BYTECODE_LEN * 4 / 3) + 9);
    let err = CompiledScript::deserialize_base64(&encoded).expect_err("should reject oversized encoded payload");
    assert!(err.contains("bytes encoded"), "{err}");
}

#[test]
fn test_reject_oversized_decoded_payload() {
    use base64::Engine;
    let huge = vec![b'a'; MAX_DECODED_BYTECODE_LEN + 1];
    let encoded = base64::engine::general_purpose::STANDARD.encode(huge);
    let err = CompiledScript::deserialize_base64(&encoded).expect_err("should reject oversized payload");
    assert!(err.contains("bytes decoded"), "{err}");
}

#[test]
fn test_reject_unknown_version() {
    use base64::Engine;
    let invalid = serde_json::json!({
        "version": 999,
        "code": ["ReturnNext"],
        "constants": [],
        "local_count": 0,
        "max_loop_depth": 0
    });
    let bytes = serde_json::to_vec(&invalid).expect("json encode should succeed");
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    let err = CompiledScript::deserialize_base64(&encoded).expect_err("should reject unsupported version");
    assert!(err.contains("unsupported bytecode version"));
}

#[test]
fn test_reject_old_and_unversioned_bytecode_schemas() {
    use base64::Engine;
    for old_version in [None, Some(0), Some(1), Some(2), Some(3), Some(4)] {
        let mut payload = serde_json::json!({
            "code": ["ReturnNext"],
            "constants": [],
            "local_count": 0,
            "max_loop_depth": 0
        });
        if let Some(version) = old_version {
            payload["version"] = serde_json::json!(version);
        }
        let encoded = base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(&payload).unwrap());
        let err = CompiledScript::deserialize_base64(&encoded).expect_err("old body semantics must be rejected");
        assert!(err.contains("unsupported bytecode version"));
    }
}

#[test]
fn is_body_builtin_only_true_for_body_ids() {
    assert!(BuiltinId::ReqBody.is_body_builtin());
    assert!(BuiltinId::RespBody.is_body_builtin());
    assert!(!BuiltinId::ReqHeader.is_body_builtin());
    assert!(!BuiltinId::RedisGet.is_body_builtin());
}

#[test]
fn references_builtin_detects_body_use_per_direction() {
    let limits = crate::validator::ValidationLimits::default();
    // req.body only
    let req_only = crate::resolve_script(Some("let b = req.body"), None, &limits).unwrap();
    assert!(req_only.references_builtin(BuiltinId::ReqBody));
    assert!(!req_only.references_builtin(BuiltinId::RespBody));
    // resp.body only
    let resp_only = crate::resolve_script(Some("let b = resp.body"), None, &limits).unwrap();
    assert!(resp_only.references_builtin(BuiltinId::RespBody));
    assert!(!resp_only.references_builtin(BuiltinId::ReqBody));
    // neither
    let header_only = crate::resolve_script(Some("let h = req.header(\"X\")"), None, &limits).unwrap();
    assert!(!header_only.references_builtin(BuiltinId::ReqBody));
    assert!(!header_only.references_builtin(BuiltinId::RespBody));
}

#[test]
fn request_body_introspection_distinguishes_reads_and_mutations() {
    let limits = crate::validator::ValidationLimits::default();
    let read = crate::resolve_script(Some("let b = req.body.contains(\"x\")"), None, &limits).unwrap();
    assert!(read.references_request_body());
    assert!(!read.references_request_body_mutation());

    let mutation = crate::resolve_script(Some("req.body.append(\"x\")"), None, &limits).unwrap();
    assert!(mutation.references_request_body());
    assert!(mutation.references_request_body_mutation());
}
