use super::*;

fn limits() -> SpanLimits {
    SpanLimits {
        max_spans: 64,
        max_span_bytes: 4096,
        max_total_span_bytes: 65536,
    }
}

#[test]
fn selects_string_content_and_text_parts_by_role() {
    let body = br#"{"model":"m","messages":[
            {"role":"system","content":"be nice"},
            {"role":"user","content":[{"type":"text","text":"hello"},{"type":"image_url","image_url":{"url":"x"}},{"type":"text","text":"world"}]}
        ]}"#;
    let spans = select_spans(body, limits()).expect("selects");
    assert_eq!(spans.len(), 3);
    assert_eq!(spans[0].role, Some(AiGuardRole::System));
    assert_eq!(spans[0].current, "be nice");
    assert_eq!(spans[1].key.part, GuardPart::TextPart(0));
    assert_eq!(spans[1].current, "hello");
    assert_eq!(spans[2].current, "world");
    // Spans address the complete quoted token, quotes included.
    assert_eq!(
        &body[spans[0].token.start as usize..spans[0].token.end as usize],
        b"\"be nice\""
    );
}

#[test]
fn spans_are_sorted_and_disjoint() {
    let body = br#"{"messages":[{"role":"user","content":[{"type":"text","text":"a"},{"type":"text","text":"b"}]},{"role":"user","content":"c"}]}"#;
    let spans = select_spans(body, limits()).expect("selects");
    assert!(spans.windows(2).all(|pair| pair[0].key < pair[1].key));
    assert!(spans.windows(2).all(|pair| pair[0].token.end <= pair[1].token.start));
}

#[test]
fn decodes_escapes_and_multibyte_text() {
    let body = r#"{"messages":[{"role":"user","content":"café \"q\" €"}]}"#.as_bytes();
    let spans = select_spans(body, limits()).expect("selects");
    assert_eq!(spans[0].current, "café \"q\" €");
}

#[test]
fn escaped_text_uses_its_decoded_length_for_bounds() {
    let exact = br#"{"messages":[{"role":"user","content":"\u0061"}]}"#;
    let one_byte = SpanLimits {
        max_spans: 1,
        max_span_bytes: 1,
        max_total_span_bytes: 1,
    };
    let spans = select_spans(exact, one_byte).expect("escaped byte fits decoded bound");
    assert_eq!(spans[0].current, "a");

    let plus_one = br#"{"messages":[{"role":"user","content":"\u0061\u0062"}]}"#;
    assert_eq!(select_spans(plus_one, one_byte), Err(SpanError::Bounds));
}

#[test]
fn surrogate_pair_uses_its_decoded_utf8_length_for_bounds() {
    let four_bytes = SpanLimits {
        max_spans: 1,
        max_span_bytes: 4,
        max_total_span_bytes: 4,
    };
    let exact = br#"{"messages":[{"role":"user","content":"\uD83D\uDE00"}]}"#;
    let spans = select_spans(exact, four_bytes).expect("four-byte scalar fits");
    assert_eq!(spans[0].current, "😀");

    let plus_one = br#"{"messages":[{"role":"user","content":"\uD83D\uDE00a"}]}"#;
    assert_eq!(select_spans(plus_one, four_bytes), Err(SpanError::Bounds));
}

#[test]
fn zero_and_overflow_encoded_bounds_are_safe() {
    assert_eq!(max_encoded_json_string_token_bytes(usize::MAX), usize::MAX);

    let zero = SpanLimits {
        max_spans: 1,
        max_span_bytes: 0,
        max_total_span_bytes: 0,
    };
    let empty = br#"{"messages":[{"role":"user","content":""}]}"#;
    assert_eq!(select_spans(empty, zero).expect("empty span fits")[0].current, "");

    let nonempty = br#"{"messages":[{"role":"user","content":"\u0061"}]}"#;
    assert_eq!(select_spans(nonempty, zero), Err(SpanError::Bounds));
}

#[test]
fn escaped_fixed_vocabulary_and_overlong_unknown_values_keep_their_semantics() {
    let escaped = br#"{"messages":[{"role":"\u0075ser","content":[{"type":"t\u0065xt","text":"x"}]}]}"#;
    let spans = select_spans(escaped, limits()).expect("escaped fixed values decode");
    assert_eq!(spans[0].role, Some(AiGuardRole::User));
    assert_eq!(spans[0].current, "x");

    let long_role = "r".repeat(55);
    let long_type = "t".repeat(25);
    let body = format!(
        r#"{{"messages":[{{"role":"{long_role}","content":"unknown role"}},{{"role":"user","content":[{{"type":"{long_type}","text":"skipped"}},{{"type":"text","text":"selected"}}]}}]}}"#
    );
    let spans = select_spans(body.as_bytes(), limits()).expect("overlong unknown values skip decoding");
    assert_eq!(spans.len(), 2);
    assert_eq!(spans[0].role, None);
    assert_eq!(spans[0].current, "unknown role");
    assert_eq!(spans[1].current, "selected");
}

#[test]
fn mixed_non_object_parts_are_skipped_without_hiding_valid_text() {
    let body = br#"{"messages":[{"role":"user","content":["raw",null,[],7,true,{"type":"text","text":"selected"}]}]}"#;
    let spans = select_spans(body, limits()).expect("non-object parts are skipped");
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].key.part, GuardPart::TextPart(5));
    assert_eq!(spans[0].current, "selected");
}

#[test]
fn duplicate_part_type_is_unparseable_in_either_order() {
    for part in [
        r#"{"type":"text","type":"image_url","text":"x"}"#,
        r#"{"type":"image_url","type":"text","text":"x"}"#,
    ] {
        let body = format!(r#"{{"messages":[{{"role":"user","content":[{part}]}}]}}"#);
        assert_eq!(select_spans(body.as_bytes(), limits()), Err(SpanError::Unparseable));
    }
}

#[test]
fn duplicate_text_on_a_text_part_is_unparseable_in_either_order() {
    for part in [
        r#"{"type":"text","text":"first","text":"second"}"#,
        r#"{"type":"text","text":"second","text":"first"}"#,
    ] {
        let body = format!(r#"{{"messages":[{{"role":"user","content":[{part}]}}]}}"#);
        assert_eq!(select_spans(body.as_bytes(), limits()), Err(SpanError::Unparseable));
    }
}

#[test]
fn duplicate_text_on_a_non_text_part_is_not_inspected() {
    let body = br#"{"messages":[{"role":"user","content":[{"type":"image_url","text":"first","text":"second"},{"type":"text","text":"selected"}]}]}"#;
    let spans = select_spans(body, limits()).expect("non-text fields are skipped");
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].current, "selected");
}

#[test]
fn span_count_is_checked_before_decoding_the_first_excess_value() {
    let body = br#"{"messages":[{"role":"user","content":"first"},{"role":"user","content":"\q"}]}"#;
    let one_span = SpanLimits {
        max_spans: 1,
        max_span_bytes: 64,
        max_total_span_bytes: 128,
    };
    assert_eq!(select_spans(body, one_span), Err(SpanError::Bounds));
    assert_eq!(
        select_spans(
            body,
            SpanLimits {
                max_spans: 2,
                ..one_span
            }
        ),
        Err(SpanError::Unparseable)
    );
}

#[test]
fn ambiguous_role_or_content_is_unparseable() {
    let duplicate_role = br#"{"messages":[{"role":"user","role":"system","content":"x"}]}"#;
    assert_eq!(select_spans(duplicate_role, limits()), Err(SpanError::Unparseable));

    let duplicate_messages = br#"{"messages":[],"messages":[]}"#;
    assert_eq!(select_spans(duplicate_messages, limits()), Err(SpanError::Unparseable));

    let missing_messages = br#"{"model":"m"}"#;
    assert_eq!(select_spans(missing_messages, limits()), Err(SpanError::Unparseable));
}

#[test]
fn bounds_are_inclusive_and_reject_plus_one() {
    let body = br#"{"messages":[{"role":"user","content":"abcde"}]}"#;
    let exact = SpanLimits {
        max_spans: 1,
        max_span_bytes: 5,
        max_total_span_bytes: 5,
    };
    assert!(select_spans(body, exact).is_ok());

    let short = SpanLimits {
        max_span_bytes: 4,
        ..exact
    };
    assert_eq!(select_spans(body, short), Err(SpanError::Bounds));

    let two = br#"{"messages":[{"role":"user","content":"ab"},{"role":"user","content":"cd"}]}"#;
    let one_span = SpanLimits {
        max_spans: 1,
        max_span_bytes: 8,
        max_total_span_bytes: 64,
    };
    assert_eq!(select_spans(two, one_span), Err(SpanError::Bounds));
}

#[test]
fn unknown_role_selects_without_a_role_scope() {
    let body = br#"{"messages":[{"role":"function","content":"x"}]}"#;
    let spans = select_spans(body, limits()).expect("selects");
    assert_eq!(spans[0].role, None);
}

#[test]
fn fixed_values_borrow_plain_text_and_preserve_encoded_admission() {
    for (text, limit) in [
        ("user", MAX_RECOGNIZED_ROLE_BYTES),
        ("function", MAX_RECOGNIZED_ROLE_BYTES),
        ("text", TEXT_PART_TYPE_BYTES),
    ] {
        let body = format!("\"{text}\"");
        let token = StringToken {
            value: String::new(),
            start: 0,
            end: body.len(),
        };
        let (decoded, stats) = crate::test_allocation_counter::count_allocation_stats(|| {
            decode_fixed_value(body.as_bytes(), &token, limit).unwrap()
        });
        assert!(matches!(decoded, Cow::Borrowed(_)));
        assert_eq!(decoded, text);
        assert_eq!(decoded.as_ptr(), body.as_bytes()[1..].as_ptr());
        assert_eq!(stats.calls, 0);
        assert_eq!(stats.requested_bytes, 0);
    }
    for limit in [MAX_RECOGNIZED_ROLE_BYTES, TEXT_PART_TYPE_BYTES] {
        for length in [limit * 6, limit * 6 + 1] {
            let body = format!("\"{}\"", "x".repeat(length));
            let token = StringToken {
                value: String::new(),
                start: 0,
                end: body.len(),
            };
            let decoded = decode_fixed_value(body.as_bytes(), &token, limit);
            assert_eq!(decoded.is_some(), length == limit * 6);
        }
    }
    let body = br#""\u0075ser""#;
    let token = StringToken {
        value: String::new(),
        start: 0,
        end: body.len(),
    };
    let decoded = decode_fixed_value(body, &token, MAX_RECOGNIZED_ROLE_BYTES).unwrap();
    assert!(matches!(decoded, Cow::Owned(_)));
    assert_eq!(parse_role(&decoded), Some(AiGuardRole::User));
}
