use super::*;

#[test]
fn strict_envelope_rejects_malformed_decisions() {
    assert_eq!(
        parse_decision(br#"{"version":"v1","action":"pass"}"#, 64),
        Ok(GuardDecision::Pass)
    );
    assert!(parse_decision(br#"{"version":"v2","action":"pass"}"#, 64).is_err());
    assert!(parse_decision(br#"{"version":"v1","action":"pass","extra":1}"#, 64).is_err());
    assert!(parse_decision(br#"{"version":"v1","action":"pass"} {}"#, 64).is_err());
    assert!(parse_decision(br#"{"version":"v1","action":"pass","code":"x"}"#, 64).is_err());
    assert!(parse_decision(br#"{"version":"v1","action":"replace","replacements":[]}"#, 64).is_err());
    assert!(parse_decision(br#"{"version":"v1","action":"nope"}"#, 64).is_err());
}

#[test]
fn replacement_cap_is_inclusive_and_rejects_plus_one() {
    let body = br#"{"version":"v1","action":"replace","replacements":[{"messageIndex":0,"part":{"type":"content"},"current":"abc"}]}"#;
    assert!(parse_decision(body, 3).is_ok());
    assert!(parse_decision(body, 2).is_err());
}

#[test]
fn duplicate_replacement_keys_are_rejected() {
    let body = br#"{"version":"v1","action":"replace","replacements":[{"messageIndex":0,"part":{"type":"content"},"current":"a"},{"messageIndex":0,"part":{"type":"content"},"current":"b"}]}"#;
    assert!(parse_decision(body, 64).is_err());
}

#[test]
fn request_encoding_is_capped_inclusively() {
    let spans = vec![RequestSpan {
        message_index: 0,
        part: WirePart::Content,
        current: "hello",
    }];
    let encoded = encode_request(&spans, 1024).expect("encodes");
    assert_eq!(encode_request(&spans, encoded.len()).expect("exact cap"), encoded);
    assert!(encode_request(&spans, encoded.len() - 1).is_err());
}

#[test]
fn only_single_json_success_responses_are_accepted() {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.append(
        reqwest::header::CONTENT_TYPE,
        "application/json; charset=utf-8".parse().unwrap(),
    );
    assert!(is_json_response(200, &headers));
    assert!(!is_json_response(500, &headers));
    headers.append(reqwest::header::CONTENT_TYPE, "application/json".parse().unwrap());
    assert!(!is_json_response(200, &headers));
}

#[test]
fn request_payload_carries_only_span_text_and_keys() {
    let spans = vec![RequestSpan {
        message_index: 2,
        part: WirePart::TextPart { part_index: 1 },
        current: "prompt",
    }];
    let encoded = encode_request(&spans, 4096).expect("encodes");
    let text = String::from_utf8(encoded.to_vec()).expect("utf8");
    assert!(text.contains("\"version\":\"v1\""));
    assert!(text.contains("\"messageIndex\":2"));
    assert!(text.contains("\"partIndex\":1"));
    assert!(text.contains("prompt"));
    // No model, provider, backend, header, or credential material.
    assert!(!text.contains("model"));
    assert!(!text.contains("authorization"));
}
