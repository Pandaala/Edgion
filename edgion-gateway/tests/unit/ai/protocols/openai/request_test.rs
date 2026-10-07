use super::*;

#[test]
fn native_metadata_skips_provider_extensions_without_numeric_coercion() {
    let body = Bytes::from_static(br#" { "extension": 1234567890123456789012345678901234567890, "model" : "native", "messages": [{"content":"hello"}], "stream": true } "#);
    let parsed = metadata(&body).unwrap();
    assert_eq!(parsed.model, "native");
    assert!(parsed.streaming);
    assert!(rewrite_model(&body, "native").unwrap().is_none());
    let changed = rewrite_model(&body, "selected").unwrap().unwrap();
    assert_eq!(
        changed,
        Bytes::from(
            String::from_utf8(body.to_vec())
                .unwrap()
                .replace("\"native\"", "\"selected\"")
        )
    );
}

#[test]
fn explicit_usage_mutation_preserves_every_unrelated_byte() {
    for (original, expected) in [
        (
            r#" { "model":"m", "stream":true, "stream_options" : { "provider_extra": 99999999999999999999999999, "include_usage" : false } } "#,
            r#" { "model":"m", "stream":true, "stream_options" : { "provider_extra": 99999999999999999999999999, "include_usage" : true } } "#,
        ),
        (
            r#"{"model":"m","stream":true,"stream_options":{ }}"#,
            r#"{"model":"m","stream":true,"stream_options":{"include_usage":true }}"#,
        ),
        (
            r#"{"model":"m","stream":true}"#,
            r#"{"stream_options":{"include_usage":true},"model":"m","stream":true}"#,
        ),
    ] {
        let changed = inject_usage(&Bytes::copy_from_slice(original.as_bytes()))
            .unwrap()
            .unwrap();
        assert_eq!(changed.as_ref(), expected.as_bytes());
    }
    assert!(inject_usage(&Bytes::from_static(
        br#"{"model":"m","stream":true,"stream_options":{"include_usage":true}}"#
    ))
    .unwrap()
    .is_none());
}

#[test]
fn metadata_rejects_ambiguous_keys_and_conflicting_bounds() {
    for body in [
        r#"{"model":"a","model":"b"}"#,
        r#"{"model":"a","max_tokens":5,"max_completion_tokens":6}"#,
        r#"{"model":null}"#,
        r#"[]"#,
    ] {
        assert!(metadata(body.as_bytes()).is_err(), "accepted {body}");
    }
    assert_eq!(
        metadata(br#"{"model":"a","max_tokens":6,"max_completion_tokens":6}"#)
            .unwrap()
            .output_bound,
        Some(6)
    );
}

#[test]
fn metadata_and_usage_injection_require_json_objects() {
    for body in [b"[\"model\",true]".as_slice(), b"null", b"42"] {
        assert!(matches!(metadata(body), Err(AiProxyError::MalformedBody)));
    }
    for options in ["[false]", "null", "false", "42"] {
        let body = Bytes::from(format!(
            r#"{{"model":"model","stream":true,"stream_options":{options}}}"#
        ));
        assert!(matches!(inject_usage(&body), Err(AiProxyError::MalformedBody)));
        assert_eq!(
            metadata(&body).unwrap().model,
            "model",
            "native metadata leaves unselected provider fields alone"
        );
    }
}

#[test]
fn responses_metadata_preserves_native_items_and_selects_only_routing_fields() {
    let body = Bytes::from_static(br#" { "model" : "model-a", "input":[{"type":"reasoning","encrypted_content":"opaque"}],"tools":[{"type":"web_search"}],"extension":1e99999,"stream":true,"max_output_tokens":91,"conversation":{"id":"opaque-id"},"stream_options":{"include_obfuscation":false} } "#);
    let metadata = responses_metadata(&body).unwrap();
    assert_eq!(metadata.model, "model-a");
    assert!(metadata.streaming && metadata.continuation);
    assert_eq!(metadata.output_bound, Some(91));
    assert!(rewrite_model(&body, "model-a").unwrap().is_none());
    let rewritten = rewrite_model(&body, "model-b").unwrap().unwrap();
    assert_eq!(
        rewritten,
        Bytes::from(String::from_utf8(body.to_vec()).unwrap().replace("model-a", "model-b"))
    );
}

#[test]
fn responses_rejects_unsupported_and_ambiguous_managed_metadata() {
    for body in [
        br#"{"model":"m","background":true}"#.as_slice(),
        br#"{"model":"m","previous_response_id":"id","conversation":"conversation"}"#,
        br#"{"model":"m","max_output_tokens":-1}"#,
        br#"{"model":"m","stream":"true"}"#,
        br#"{"model":"m","model":"n"}"#,
        br#"{"model":"m","conversation":42}"#,
        br#"[]"#,
    ] {
        assert!(responses_metadata(body).is_err(), "{body:?}");
    }
    assert!(
        !responses_metadata(br#"{"model":"m","previous_response_id":null,"conversation":null}"#)
            .unwrap()
            .continuation
    );
    assert!(
        responses_metadata(br#"{"model":"m","previous_response_id":"opaque"}"#)
            .unwrap()
            .continuation
    );
}
