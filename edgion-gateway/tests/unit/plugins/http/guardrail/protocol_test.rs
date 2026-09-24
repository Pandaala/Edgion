use super::*;

#[test]
fn strict_decisions_and_bounds() {
    assert_eq!(
        parse_decision(br#"{"version":"v1","action":"pass"}"#, 10),
        Ok(ParsedDecision::Pass)
    );
    assert!(parse_decision(br#"{"version":"v1","version":"v1","action":"pass"}"#, 10).is_err());
    assert!(parse_decision(br#"{"version":"v1","action":"pass","action":"pass"}"#, 10).is_err());
    assert!(parse_decision(br#"{"version":"v1"}"#, 10).is_err());
    assert!(parse_decision(br#"{"version":"v1","action":"pass","extra":1}"#, 10).is_err());
    assert!(parse_decision(br#"{"version":"v2","action":"pass"}"#, 10).is_err());
    assert!(parse_decision(br#"{"version":"v1","action":"reject","replacements":[]}"#, 10).is_err());
    assert!(parse_decision(
            br#"{"version":"v1","action":"replace","replacements":[{"choiceIndex":0,"field":{"type":"content"},"current":"123"}]}"#,
            2
        )
        .is_err());
}

#[test]
fn capped_encoding_accepts_exact_and_rejects_plus_one() {
    let windows = vec![RequestWindow {
        choice_index: 0,
        field: WireField::Content,
        context: String::new(),
        current: "hello".to_string(),
    }];
    let encoded = encode_request(&windows, 1024).unwrap();
    assert_eq!(encode_request(&windows, encoded.len()).unwrap(), encoded);
    assert!(encode_request(&windows, encoded.len() - 1).is_err());
}

#[test]
fn response_requires_one_json_content_type_and_success_status() {
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
