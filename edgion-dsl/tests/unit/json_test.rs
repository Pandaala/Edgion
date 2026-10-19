use super::*;

#[test]
fn raw_json_preserves_numbers_null_and_nested_values() {
    let source = r#"{"n":18446744073709551616000,"v":{"x":[0.123456789012345678901,null]}}"#;
    let mut work = ValueWorkBudget::default();
    let document = parse_raw(source, MAX_DSL_JSON_BYTES, &mut work).unwrap();
    assert_eq!(document.get("n"), Some("18446744073709551616000"));
    assert_eq!(document.get("missing"), None);
    assert_eq!(document.get("v"), Some(r#"{"x":[0.123456789012345678901,null]}"#));
    let output = set_member(
        r#"{"query":"query Q { a }"}"#,
        "variables",
        MemberValue::Raw(source),
        MAX_DSL_JSON_BYTES,
        &mut work,
    )
    .unwrap();
    assert!(output.contains(source));
    let output = set_member(
        &output,
        "operationName",
        MemberValue::String("Q\"\n\\é"),
        MAX_DSL_JSON_BYTES,
        &mut work,
    )
    .unwrap();
    let parsed = parse_raw(&output, MAX_DSL_JSON_BYTES, &mut work).unwrap();
    assert_eq!(
        decode_string(parsed.get("operationName").unwrap(), &mut work)
            .unwrap()
            .unwrap()
            .as_ref(),
        "Q\"\n\\é"
    );
}

#[test]
fn raw_json_rejects_duplicate_decoded_keys_everywhere() {
    for source in [r#"{"a":1,"\u0061":2}"#, r#"{"x":[{"a":1,"a":2}]}"#] {
        assert!(matches!(
            parse_raw(source, MAX_DSL_JSON_BYTES, &mut ValueWorkBudget::default()),
            Err(JsonError::Strict(StrictJsonError::DuplicateKey))
        ));
    }
    assert!(matches!(
        parse_raw("{}tail", MAX_DSL_JSON_BYTES, &mut ValueWorkBudget::default()),
        Err(JsonError::Strict(StrictJsonError::Malformed))
    ));
}

#[test]
fn raw_json_limits_and_repeated_work_fail_closed() {
    assert!(matches!(
        parse_raw("null", 3, &mut ValueWorkBudget::default()),
        Err(JsonError::OutputLimit { limit: 3 })
    ));
    let source = format!("{}0{}", "[".repeat(DSL_JSON_DEPTH), "]".repeat(DSL_JSON_DEPTH));
    assert!(matches!(
        parse_raw(&source, MAX_DSL_JSON_BYTES, &mut ValueWorkBudget::default()),
        Err(JsonError::Strict(StrictJsonError::DepthLimit))
    ));
    let source = format!("{{\"{}\":0}}", "\\u0061".repeat(1000));
    let mut work = ValueWorkBudget::default();
    let mut exhausted = false;
    for _ in 0..300 {
        if matches!(
            parse_raw(&source, MAX_DSL_JSON_BYTES, &mut work),
            Err(JsonError::Work(_))
        ) {
            exhausted = true;
            break;
        }
    }
    assert!(exhausted);
    assert!(matches!(
        set_member(
            "{}",
            "x",
            MemberValue::String("a\n\""),
            8,
            &mut ValueWorkBudget::default()
        ),
        Err(JsonError::OutputLimit { .. })
    ));
}

#[test]
fn dom_profile_and_competing_error_order_remain_stable() {
    let limits = JsonLimits {
        bytes: 1024 * 1024,
        depth: 64,
        nodes: 20_000,
    };
    assert_eq!(
        parse_dom(br#"{"a":0,"a": }"#, limits).unwrap_err(),
        StrictJsonError::DuplicateKey
    );
    assert_eq!(
        parse_dom(br#"{"a":0,"b": }"#, JsonLimits { nodes: 2, ..limits }).unwrap_err(),
        StrictJsonError::NodeLimit
    );
    assert_eq!(
        parse_dom(br#"{"a": }"#, JsonLimits { depth: 1, ..limits }).unwrap_err(),
        StrictJsonError::DepthLimit
    );
    let parsed = parse_dom(br#"{"a":["xyz",2]}"#, limits).unwrap();
    assert_eq!(parsed.node_count(), 4);
    assert_eq!(parsed.text_bytes(), 4);
}

#[test]
fn dom_duplicate_keys_precede_value_errors_and_remain_object_local() {
    let limits = JsonLimits {
        bytes: 1024,
        depth: 4,
        nodes: 8,
    };
    for source in [
        br#"{"a":0,"\u0061":2}"#.as_slice(),
        br#"{"x":[{"a":0,"\u0061":2}]}"#,
        br#"{"x":{"a":0,"a": }}"#,
    ] {
        assert_eq!(parse_dom(source, limits).unwrap_err(), StrictJsonError::DuplicateKey);
    }
    for source in [br#"{"a":0,"a": }"#.as_slice(), br#"{"a":0,"a":[[0]]}"#] {
        assert_eq!(
            parse_dom(
                source,
                JsonLimits {
                    depth: 2,
                    nodes: 2,
                    ..limits
                }
            )
            .unwrap_err(),
            StrictJsonError::DuplicateKey
        );
    }

    // A failed nested parse must not affect a later document or sibling key scope.
    let parsed = parse_dom(br#"{"a":{"a":"x"},"b":[{"a":"yz"}]}"#, limits).unwrap();
    assert_eq!(
        parsed.value(),
        &serde_json::json!({"a": {"a": "x"}, "b": [{"a": "yz"}]})
    );
    assert_eq!(parsed.node_count(), 6);
    assert_eq!(parsed.text_bytes(), 7);
}

#[test]
fn raw_json_node_and_source_caps_are_exact_and_construction_checks_combined_shape() {
    let array = format!(
        "[{}]",
        std::iter::repeat_n("0", DSL_JSON_NODES as usize - 1)
            .collect::<Vec<_>>()
            .join(",")
    );
    assert!(parse_raw(&array, MAX_DSL_JSON_BYTES, &mut ValueWorkBudget::default()).is_ok());
    let array = format!(
        "[{}]",
        std::iter::repeat_n("0", DSL_JSON_NODES as usize)
            .collect::<Vec<_>>()
            .join(",")
    );
    assert!(matches!(
        parse_raw(&array, MAX_DSL_JSON_BYTES, &mut ValueWorkBudget::default()),
        Err(JsonError::Strict(StrictJsonError::NodeLimit))
    ));
    let source = format!("\"{}\"", "a".repeat(MAX_DSL_JSON_BYTES - 2));
    assert!(parse_raw(&source, MAX_DSL_JSON_BYTES, &mut ValueWorkBudget::default()).is_ok());
    let too_large = format!("{source} ");
    assert!(matches!(
        parse_raw(&too_large, MAX_DSL_JSON_BYTES, &mut ValueWorkBudget::default()),
        Err(JsonError::OutputLimit { .. })
    ));
    let nested = format!("{}0{}", "[".repeat(DSL_JSON_DEPTH - 1), "]".repeat(DSL_JSON_DEPTH - 1));
    assert!(parse_raw(&nested, MAX_DSL_JSON_BYTES, &mut ValueWorkBudget::default()).is_ok());
    assert!(matches!(
        set_member(
            "{}",
            "nested",
            MemberValue::Raw(&nested),
            MAX_DSL_JSON_BYTES,
            &mut ValueWorkBudget::default()
        ),
        Err(JsonError::Strict(StrictJsonError::DepthLimit))
    ));
}

#[test]
fn raw_scalar_strings_validate_surrogates_without_changing_lexemes() {
    for source in [r#""\uD800""#, r#""\uDC00""#, r#"{"x":"\uD800"}"#, r#"["\uD800"]"#] {
        assert!(matches!(
            parse_raw(source, MAX_DSL_JSON_BYTES, &mut ValueWorkBudget::default()),
            Err(JsonError::Strict(StrictJsonError::Malformed))
        ));
    }
    let source = r#"{"x":"\uD834\uDD1E"}"#;
    let mut work = ValueWorkBudget::default();
    let document = parse_raw(source, MAX_DSL_JSON_BYTES, &mut work).unwrap();
    assert_eq!(document.get("x"), Some(r#""\uD834\uDD1E""#));
    assert_eq!(
        decode_string(document.get("x").unwrap(), &mut work)
            .unwrap()
            .unwrap()
            .as_ref(),
        "𝄞"
    );
}

#[test]
fn raw_key_admission_moves_owned_keys_and_preserves_rejections() {
    let input = "nested-member".to_owned();
    let original_buffer = input.as_ptr();
    let mut work = ValueWorkBudget::default();
    let mut state = RawState {
        policy: ParseState::default(),
        work: &mut work,
        error: None,
    };
    let mut keys = HashSet::new();
    state.admit_key(&mut keys, Cow::Owned(input)).unwrap();
    assert_eq!(keys.get("nested-member").unwrap().as_ptr(), original_buffer);

    for owned in [false, true] {
        let mut work = ValueWorkBudget::default();
        let mut state = RawState {
            policy: ParseState::default(),
            work: &mut work,
            error: None,
        };
        let mut keys = HashSet::new();
        let key = || {
            if owned {
                Cow::Owned("member".to_owned())
            } else {
                Cow::Borrowed("member")
            }
        };
        state.admit_key(&mut keys, key()).unwrap();
        assert!(matches!(
            state.admit_key(&mut keys, key()),
            Err(JsonError::Strict(StrictJsonError::DuplicateKey))
        ));
        assert_eq!(keys.len(), 1);

        let mut exhausted = ValueWorkBudget::exhausted();
        let mut state = RawState {
            policy: ParseState::default(),
            work: &mut exhausted,
            error: None,
        };
        let mut keys = HashSet::new();
        assert!(matches!(state.admit_key(&mut keys, key()), Err(JsonError::Work(_))));
        assert!(keys.is_empty());
        assert_eq!(keys.capacity(), 0);
    }
}
