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

#[test]
fn plain_terminal_decisions_do_not_allocate() {
    for (body, expected) in [
        (br#"{"version":"v1","action":"pass"}"#.as_slice(), ParsedDecision::Pass),
        (
            br#"{"version":"v1","action":"reject"}"#.as_slice(),
            ParsedDecision::Reject,
        ),
    ] {
        let (result, stats) = crate::test_allocation_counter::count_allocation_stats(|| parse_decision(body, 10));
        assert_eq!(result, Ok(expected));
        assert_eq!(stats.calls, 0);
        assert_eq!(stats.requested_bytes, 0);
    }
}

#[test]
fn escaped_decision_tokens_preserve_decoded_meaning() {
    for (plain, escaped) in [
        (br#"{"version":"v1","action":"pass"}"#.as_slice(),
         br#"{"version":"\u00761","action":"p\u0061ss"}"#.as_slice()),
        (br#"{"version":"v1","action":"reject","code":"policy","reason":"blocked"}"#,
         br#"{"version":"\u00761","action":"r\u0065ject","code":"policy","reason":"blocked"}"#),
        (br#"{"version":"v1","action":"replace","replacements":[{"choiceIndex":0,"field":{"type":"content"},"current":"hello"}]}"#,
         br#"{"version":"\u00761","action":"r\u0065place","replacements":[{"choiceIndex":0,"field":{"type":"content"},"current":"hello"}]}"#),
        (br#"{"version":"v2","action":"pass"}"#,
         br#"{"version":"\u00762","action":"p\u0061ss"}"#),
        (br#"{"version":"v1","action":"unknown"}"#,
         br#"{"version":"\u00761","action":"\u0075nknown"}"#),
    ] {
        assert_eq!(parse_decision(escaped, 10), parse_decision(plain, 10));
    }
}

#[test]
fn decision_token_errors_keep_their_categories_and_precedence() {
    for body in [
        br#"{"action":"pass"}"#.as_slice(),
        br#"{"version":"v1"}"#,
        br#"{"version":null,"action":"pass"}"#,
        br#"{"version":"v1","action":1}"#,
        br#"{"version":"v1","action":null}"#,
        br#"{"version":1,"action":"pass"}"#,
        br#"{"version":"v1","version":"v1","action":"pass"}"#,
        br#"{"version":"v1","action":"pass","action":"pass"}"#,
        br#"{"version":"v1","action":"pass","extra":1}"#,
        br#"{"version":"v1","action":"\q"}"#,
    ] {
        assert_eq!(parse_decision(body, 10), Err("invalid-json"));
    }
    assert_eq!(
        parse_decision(br#"{"version":"v2","action":"pass"} {}"#, 10),
        Err("trailing-json")
    );
    assert_eq!(
        parse_decision(br#"{"version":"v2","action":"unknown"}"#, 10),
        Err("unsupported-version")
    );
    assert_eq!(
        parse_decision(br#"{"version":"v1","action":"unknown"}"#, 10),
        Err("unsupported-action")
    );
    assert_eq!(
        parse_decision(br#"{"version":"v1","action":"pass","code":"policy"}"#, 10),
        Err("invalid-action-fields")
    );
}

#[test]
fn replacement_order_preserves_keys_text_and_exact_byte_cap() {
    let mut expected = Vec::new();
    let mut replacements = Vec::new();
    let mut total = 0;
    for index in 0..256u32 {
        let field = match index % 4 {
            0 => WireField::Content,
            1 => WireField::ReasoningContent,
            value => WireField::ToolArguments {
                tool_call_index: value - 2,
            },
        };
        let current = format!("text-{index}");
        total += current.len();
        expected.push((field.key(index / 4), current.clone()));
        replacements.push(serde_json::json!({
            "choiceIndex": index / 4,
            "field": field,
            "current": current,
        }));
    }
    let scrambled: Vec<_> = (0..256).map(|index| &replacements[(index * 73) % 256]).collect();
    let body = serde_json::to_vec(&serde_json::json!({
        "version": "v1", "action": "replace", "replacements": scrambled,
    }))
    .unwrap();
    assert_eq!(parse_decision(&body, total), Ok(ParsedDecision::Replace(expected)));
    assert_eq!(parse_decision(&body, total - 1), Err("replacement-too-large"));
}

#[test]
fn duplicate_replacement_keys_reject_distinct_text_for_every_field() {
    for field in [
        WireField::Content,
        WireField::ReasoningContent,
        WireField::ToolArguments { tool_call_index: 7 },
    ] {
        let body = serde_json::to_vec(&serde_json::json!({
            "version": "v1", "action": "replace", "replacements": [
                {"choiceIndex": 2, "field": field, "current": "first"},
                {"choiceIndex": 2, "field": field, "current": "second"},
            ],
        }))
        .unwrap();
        assert_eq!(parse_decision(&body, 11), Err("duplicate-replacement"));
        assert_eq!(parse_decision(&body, 10), Err("replacement-too-large"));
    }
}
