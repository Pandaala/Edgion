use super::*;

#[test]
fn native_message_array_and_nested_block_budgets_classify_conservatively() {
    use serde_json::json;

    for count in [4096, 4097] {
        let text = json!({"type":"text","text":"a"});
        let nested = json!({"type":"tool_result","content":[text]});
        for messages in [
            vec![json!({"role":"user","content":"a"}); count],
            // The outer tool_result and its nested text each consume one block.
            vec![
                json!({"content":vec![text.clone(); count - 2]}),
                json!({"content":[nested]}),
            ],
        ] {
            let body =
                serde_json::to_vec(&json!({"model":"m","max_tokens":7,"stream":true,"messages":messages})).unwrap();
            let result = metadata(&body).unwrap();
            assert_eq!((result.binding, result.composite), (count > 4096, count > 4096));
            assert_eq!(
                (result.model.as_str(), result.streaming, result.output_bound),
                ("m", true, 7)
            );
        }
    }
}

#[test]
fn native_item_limit_preserves_parse_and_depth_errors() {
    use serde_json::json;

    let mut deep = json!({"type":"text"});
    for _ in 0..17 {
        deep = json!({"type":"tool_result","content":[deep]});
    }
    for block in [deep, json!({"type":42})] {
        let body = serde_json::to_vec(&json!({"model":"m","max_tokens":1,"messages":[{"content":[block]}]})).unwrap();
        assert!(matches!(metadata(&body), Err(AiProxyError::MalformedBody)));
    }
    let invalid = format!(r#"{{"model":"m","max_tokens":1,"messages":[{}?]}}"#, "{},".repeat(4097));
    assert!(matches!(metadata(invalid.as_bytes()), Err(AiProxyError::MalformedBody)));
}

#[test]
fn native_zero_and_extensions_are_borrowed_and_model_rewrite_is_local() {
    let body = Bytes::from(format!(
        r#" {{ "model" : "claude-fixture", "max_tokens":0,"stream":true,"messages":[{{"role":"user","content":[{{"type":"text","text":"{}"}}]}}],"cache_control":{{"type":"ephemeral"}},"native":{{"keep":true}} }} "#,
        "x".repeat(256 * 1024)
    ));
    let m = metadata(&body).unwrap();
    assert_eq!(m.output_bound, 0);
    assert!(m.streaming);
    assert!(!m.binding);
    assert!(rewrite_model(&body, "claude-fixture").unwrap().is_none());
    let out = rewrite_model(&body, "other").unwrap().unwrap();
    let expected = String::from_utf8(body.to_vec())
        .unwrap()
        .replacen("\"claude-fixture\"", "\"other\"", 1);
    assert_eq!(out, Bytes::from(expected));
}
#[test]
fn selected_fields_reject_invalid_types_duplicates_and_missing_output() {
    for tail in [
        r#""max_tokens":-1"#,
        r#""max_tokens":1.0"#,
        r#""max_tokens":null"#,
        r#""max_tokens":18446744073709551616"#,
        r#""max_tokens":1,"stream":"true""#,
        r#""max_tokens":1,"model":"again""#,
    ] {
        assert!(
            metadata(format!(r#"{{"model":"m",{tail}}}"#).as_bytes()).is_err(),
            "{tail}"
        );
    }
    assert!(matches!(
        metadata(br#"{"model":"m"}"#),
        Err(AiProxyError::MissingRequiredParameter)
    ));
}
#[test]
fn declared_native_state_paths_require_binding_without_decoding_history() {
    for body in [
        r#""container":"container-id""#,
        r#""container":{"id":"container-id"}"#,
        r#""messages":[{"content":[{"type":"thinking","thinking":"private","signature":"opaque"}]}]"#,
        r#""messages":[{"content":[{"type":"redacted_thinking","data":"opaque"}]}]"#,
        r#""messages":[{"content":[{"type":"document","source":{"type":"file","file_id":"file-id"}}]}]"#,
        r#""messages":[{"content":[{"type":"container_upload","file_id":"file-id"}]}]"#,
    ] {
        assert!(
            metadata(format!(r#"{{"model":"m","max_tokens":0,{body}}}"#).as_bytes())
                .unwrap()
                .binding,
            "{body}"
        );
    }
    assert!(!metadata(br#"{"model":"m","max_tokens":1,"container":{"skills":[]},"messages":[{"content":[{"type":"tool_use","id":"tool","input":{"file_id":"ordinary"}}]}]}"#).unwrap().binding);
}
#[test]
fn composite_markers_distinguish_enabled_inference_from_replayed_history() {
    for extra in [
        r#""fallbacks":"default""#,
        r#""fallbacks":["m"]"#,
        r#""tools":[{"type":"advisor_20260301","model":"advisor-model"}]"#,
        r#""context_management":{"edits":[{"type":"compact_20260112"}]}"#,
        r#""compaction":{"type":"summarize"}"#,
        r#""messages":[{"content":[{"type":"tool_addition","tool":{"type":"tool_definition","definition":{"type":"advisor_20260301"}}}]}]"#,
    ] {
        assert!(
            metadata(format!(r#"{{"model":"m","max_tokens":1,{extra}}}"#).as_bytes())
                .unwrap()
                .composite,
            "{extra}"
        );
    }
    assert!(!metadata(br#"{"model":"m","max_tokens":1,"fallbacks":[],"tools":[{"name":"advisor","input_schema":{}}],"context_management":{"edits":[{"type":"clear_tool_uses_20250919"}]},"messages":[{"content":[{"type":"compaction","content":"old"}]}]}"#).unwrap().composite);
}

#[test]
fn nested_tool_result_file_reference_requires_one_binding() {
    let body=br#"{"model":"m","max_tokens":1,"messages":[{"role":"user","content":[{"type":"tool_result","tool_use_id":"tool_fixture","content":[{"type":"document","source":{"type":"file","file_id":"f"}}]}]}]}"#;
    assert!(metadata(body).unwrap().binding);
}

#[test]
fn messages_metadata_and_rewrite_reject_invalid_utf8_in_ignored_strings() {
    for body in [
        b"{\"model\":\"m\",\"max_tokens\":1,\"extension\":\"\xff\"}".as_slice(),
        b"{\"model\":\"m\",\"max_tokens\":1,\"extension\":{\"items\":[\"\xff\"]}}",
        b"{\"model\":\"m\",\"max_tokens\":1,\"messages\":[{\"content\":\"\xff\"}]}",
    ] {
        assert!(matches!(metadata(body), Err(AiProxyError::MalformedBody)));
        for selected in ["m", "alias"] {
            assert!(matches!(
                rewrite_model(&Bytes::copy_from_slice(body), selected),
                Err(AiProxyError::MalformedBody)
            ));
        }
    }
}

#[test]
fn messages_fields_borrow_original_multibyte_native_spans() {
    let padded = Bytes::copy_from_slice(r#"xx {"model":"m","max_tokens":1,"extension":{"text":"é€"}} yy"#.as_bytes());
    let body = padded.slice(2..padded.len() - 2);
    let parsed = fields(&body).unwrap();
    let raw = parsed.model.unwrap();
    let range = span(&body, raw).unwrap();
    assert_eq!(raw.get().as_ptr(), body[range].as_ptr());
    assert!(rewrite_model(&body, "m").unwrap().is_none());
    let changed = rewrite_model(&body, "alias").unwrap().unwrap();
    assert_eq!(
        changed.as_ref(),
        r#" {"model":"alias","max_tokens":1,"extension":{"text":"é€"}} "#.as_bytes()
    );
}

fn previous_kind(raw: Option<&RawValue>) -> Result<Option<String>, AiProxyError> {
    raw.filter(|v| v.get() != "null")
        .map(|v| {
            if v.get().len() > 386 {
                return Err(AiProxyError::UnsupportedParameter);
            }
            serde_json::from_str(v.get()).map_err(|_| AiProxyError::MalformedBody)
        })
        .transpose()
}

#[test]
fn native_kind_decoding_preserves_values_limits_and_error_precedence() {
    assert_eq!(kind(None).unwrap().as_deref(), previous_kind(None).unwrap().as_deref());
    let mut tokens = [
        "null",
        "true",
        "0",
        "[]",
        "{}",
        r#""""#,
        r#""text""#,
        r#""thinking""#,
        r#""advisor_unknown""#,
        r#""café €""#,
        r#""t\u0065xt""#,
        r#""quote\"slash\\line\n""#,
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    for length in [383, 384, 385] {
        tokens.push(format!("\"{}\"", "a".repeat(length)));
    }
    for length in [63, 64, 65] {
        tokens.push(format!("\"{}\"", r"\u0061".repeat(length)));
    }
    tokens.push(format!("[{}0]", "0,".repeat(200)));
    tokens.push(format!(r#"{{"extension":"{}"}}"#, "a".repeat(400)));
    // RawValue may retain invalid decoded surrogate values for later inspection.
    for token in [r#""\uD800""#, r#""\uDC00""#] {
        if serde_json::from_str::<&RawValue>(token).is_ok() {
            tokens.push(token.to_owned());
        }
    }
    for token in tokens {
        let raw: &RawValue = serde_json::from_str(&token).unwrap();
        let expected = previous_kind(Some(raw));
        let actual = kind(Some(raw));
        assert_eq!(
            actual.as_ref().map(|v| v.as_deref()),
            expected.as_ref().map(|v| v.as_deref()),
            "{token}"
        );
    }
}

#[test]
fn escaped_native_kinds_preserve_nested_binding_and_composite_detection() {
    let body = br#"{"model":"m","max_tokens":1,"messages":[{"content":[{"type":"tool_\u0072esult","content":[{"type":"document","source":{"type":"\u0066ile"}}]},{"type":"tool_\u0061ddition","tool":{"type":"tool_\u0064efinition","definition":{"type":"\u0061dvisor_20260301"}}}]}]}"#;
    let result = metadata(body).unwrap();
    assert!(result.binding);
    assert!(result.composite);
}

#[test]
fn native_plain_kinds_borrow_without_decoding_allocations() {
    use crate::test_allocation_counter::count_allocation_stats;

    for text in ["text", "thinking", "advisor_20260301", "café €"] {
        let token = format!("\"{text}\"");
        let raw: &RawValue = serde_json::from_str(&token).unwrap();
        let (previous, previous_stats) = count_allocation_stats(|| previous_kind(Some(raw)).unwrap().unwrap());
        let (current, current_stats) = count_allocation_stats(|| kind(Some(raw)).unwrap().unwrap());
        assert_eq!(current, previous);
        assert!(matches!(current, Cow::Borrowed(_)));
        assert_eq!(current.as_ptr(), raw.get().as_bytes()[1..].as_ptr());
        assert!(previous_stats.calls > 0);
        assert_eq!(current_stats.calls, 0);
        assert_eq!(current_stats.requested_bytes, 0);
    }
    let raw: &RawValue = serde_json::from_str(r#""t\u0065xt""#).unwrap();
    let current = kind(Some(raw)).unwrap().unwrap();
    assert_eq!(current, "text");
    assert!(matches!(current, Cow::Owned(_)));
}
