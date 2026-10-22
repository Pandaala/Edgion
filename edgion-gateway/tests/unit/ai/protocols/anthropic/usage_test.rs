use super::*;
fn observe(body: &[u8]) -> Observation {
    let mut observer = Observer::new();
    for b in body {
        observer.feed(std::slice::from_ref(b));
    }
    observer.finish("m")
}
#[test]
fn complete_json_normalizes_cache_once_and_discards_large_content() {
    let body = format!(
        r#"{{"type":"message","model":"m","stop_reason":"end_turn","content":[{{"type":"thinking","thinking":"{}"}}],"usage":{{"input_tokens":5,"output_tokens":7,"cache_creation_input_tokens":3,"cache_read_input_tokens":2,"cache_creation":{{"ephemeral_5m_input_tokens":3}},"output_tokens_details":{{"thinking_tokens":4}}}}}}"#,
        "x".repeat(256 * 1024)
    );
    let o = observe(body.as_bytes());
    assert!(!o.invalid);
    assert!(o.stop);
    assert_eq!(
        o.components.normalized(),
        Some(ReportedAiUsage {
            input_tokens: 10,
            output_tokens: 7,
            cache_read_input_tokens: 2,
            cache_write_input_tokens: 3
        })
    );
}
#[test]
fn single_document_parent_and_decoded_key_replacement_are_last_wins() {
    let o=observe(br#"{"type":"message","usage":{"input_tokens":999,"output_tokens":3,"cache_read_input_tokens":8},"usage":{"input_tokens":"invalid","input_\u0074okens":2,"output_tokens":4},"stop_reason":"tool_use"}"#);
    assert!(!o.invalid);
    assert_eq!(o.components.normalized().unwrap().input_tokens, 2);
    assert_eq!(o.components.normalized().unwrap().cache_read_input_tokens, 0);
    assert!(
        observe(br#"{"type":"message","usage":{"input_tokens":2,"output_tokens":1},"usage":null}"#)
            .components
            .normalized()
            .is_none()
    );
}
#[test]
fn overflow_invalid_counts_and_malformed_tail_never_become_totals() {
    for body in [
        br#"{"usage":{"input_tokens":18446744073709551615,"output_tokens":1}}"#.as_slice(),
        br#"{"usage":{"input_tokens":18446744073709551615,"output_tokens":0,"cache_creation_input_tokens":1}}"#,
        br#"{"usage":{"input_tokens":-1,"output_tokens":1}}"#,
        br#"{"usage":{"input_tokens":1,"output_tokens":1.0}}"#,
    ] {
        assert!(observe(body).components.normalized().is_none());
    }
    assert!(observe(br#"{"usage":{"input_tokens":1,"output_tokens":2}}tail"#).invalid);
}

#[test]
fn numeric_extensions_preserve_lexical_validation_and_metadata_shapes() {
    for number in ["0", "-1", "1.5", "1e10", "18446744073709551616"] {
        let body = format!(
            r#"{{"type":"message","stop_reason":"end_turn","extension":{number},"usage":{{"input_tokens":5,"output_tokens":7,"extension":{number}}}}}"#
        );
        let observed = observe(body.as_bytes());
        assert!(!observed.invalid && !observed.composite, "{number}");
        assert!(observed.stop);
        let usage = observed.components.normalized().unwrap();
        assert_eq!((usage.input_tokens, usage.output_tokens), (5, 7));

        for metadata in [format!(r#""usage":{number}"#), format!(r#""stop_reason":{number}"#)] {
            assert!(observe(format!("{{{metadata}}}").as_bytes()).invalid, "{metadata}");
        }
        for iterations in [
            number.to_owned(),
            format!("[{number}]"),
            format!(r#"[{{"type":"message","model":{number}}}]"#),
        ] {
            let body = format!(r#"{{"usage":{{"iterations":{iterations}}}}}"#);
            assert!(observe(body.as_bytes()).composite, "{iterations}");
        }
    }
    for number in ["01", "1.", "1e", "--1"] {
        let body = format!(r#"{{"extension":{number}}}"#);
        assert!(observe(body.as_bytes()).invalid, "{number}");
    }
}
#[test]
fn iteration_inspection_distinguishes_subtotals_from_cumulative_usage() {
    for iterations in [
        "null",
        "[]",
        r#"[{"type":"message","model":"m","usage":{"input_tokens":99}}]"#,
    ] {
        let o = observe(
            format!(r#"{{"model":"m","usage":{{"input_tokens":1,"output_tokens":2,"iterations":{iterations}}}}}"#)
                .as_bytes(),
        );
        assert!(!o.invalid);
        assert!(!o.composite);
        assert_eq!(o.components.normalized().unwrap().input_tokens, 1);
    }
    for iterations in [
        r#"[{"type":"compaction"}]"#,
        r#"[{"type":"advisor_message"}]"#,
        r#"[{"type":"fallback_message"}]"#,
        r#"[{"type":"message","model":"other"}]"#,
        r#"[{"type":"future"}]"#,
        "[false]",
        "{}",
    ] {
        assert!(
            observe(
                format!(r#"{{"model":"m","usage":{{"input_tokens":1,"output_tokens":2,"iterations":{iterations}}}}}"#)
                    .as_bytes()
            )
            .composite,
            "{iterations}"
        );
    }
}
#[test]
fn cumulative_component_patches_preserve_late_cache_and_zero() {
    let mut components = Components {
        input: Some(5),
        output: Some(0),
        ..Default::default()
    };
    components.merge(Components {
        read: Some(2),
        write: Some(3),
        output: Some(7),
        ..Default::default()
    });
    components.merge(Components {
        input: Some(8),
        output: Some(4),
        ..Default::default()
    });
    assert_eq!(components.normalized().unwrap().input_tokens, 13);
    assert_eq!(components.normalized().unwrap().output_tokens, 7);
}

#[test]
fn envelope_extensions_cannot_replace_native_accounting() {
    let o=observe(br#"{"type":"message","usage":{"input_tokens":5,"output_tokens":7,"iterations":[{"type":"advisor_message"}]},"message":{"usage":{"input_tokens":1,"output_tokens":2}},"delta":{"stop_reason":"end_turn"},"stop_reason":"end_turn"}"#);
    assert!(o.composite);
    assert_eq!(o.components.normalized().unwrap().input_tokens, 5);
    let mut start = Observer::for_envelope(Kind::Start);
    start.feed(br#"{"type":"message_start","usage":{"input_tokens":99,"output_tokens":99},"message":{"type":"message","model":"m","usage":{"input_tokens":5,"output_tokens":0}}}"#);
    let o = start.finish("m");
    assert!(!o.invalid);
    assert_eq!(o.components.normalized().unwrap().input_tokens, 5);
}
#[test]
fn usage_and_iteration_model_duplicates_replace_their_owned_state() {
    let o=observe(br#"{"type":"message","usage":{"input_tokens":1,"output_tokens":2,"iterations":[{"type":"advisor_message","model":"other"}]},"usage":{"input_tokens":3,"output_tokens":4,"iterations":[{"type":"message","model":"other","model":"m"}]}}"#);
    assert!(!o.invalid && !o.composite);
    assert_eq!(o.components.normalized().unwrap().input_tokens, 3);
    for model in ["7", "null", "true", "{}", "[]"] {
        let o=observe(format!(r#"{{"usage":{{"input_tokens":1,"output_tokens":2,"iterations":[{{"type":"message","model":{model}}}]}}}}"#).as_bytes());
        assert!(o.composite, "{model}");
    }
}

#[test]
fn every_message_iteration_requires_its_own_model_attribution() {
    for iterations in [
        r#"[{"type":"message"}]"#,
        r#"[{"type":"message","model":"m"},{"type":"message"}]"#,
        r#"[{"type":"message"},{"type":"message","model":"m"}]"#,
    ] {
        let body = format!(
            r#"{{"type":"message","model":"m","usage":{{"input_tokens":5,"output_tokens":7,"iterations":{iterations}}}}}"#
        );
        assert!(observe(body.as_bytes()).composite, "{iterations}");
    }
    for iterations in ["null", "[]", r#"[{"type":"message","model":null,"model":"m"}]"#] {
        let body = format!(
            r#"{{"type":"message","model":"m","usage":{{"input_tokens":5,"output_tokens":7,"iterations":{iterations}}}}}"#
        );
        assert!(!observe(body.as_bytes()).composite, "{iterations}");
    }
}

#[test]
fn required_native_nulls_are_invalid_but_duplicates_remain_last_wins() {
    let o = observe(br#"{"type":"message","stop_reason":"end_turn","usage":{"input_tokens":null,"output_tokens":7}}"#);
    assert!(o.invalid);
    let o=observe(br#"{"type":"message","stop_reason":"end_turn","usage":{"input_tokens":5,"output_tokens":null,"output_tokens":7}}"#);
    assert!(!o.invalid);
    assert_eq!(o.components.normalized().unwrap().output_tokens, 7);
    let mut delta = Observer::for_envelope(Kind::Delta);
    delta.feed(br#"{"type":"message_delta","usage":{"input_tokens":null,"cache_read_input_tokens":null,"output_tokens":7},"delta":{"stop_reason":null}}"#);
    let o = delta.finish("m");
    assert!(!o.invalid);
    assert_eq!(o.components.output, Some(7));
}

#[test]
fn selected_numbers_preserve_strict_unsigned_grammar_and_invalid_bits() {
    let mut cases: Vec<String> = [
        "",
        "0",
        "00",
        "00042",
        "+1",
        "-0",
        "-1",
        "1.0",
        "1e2",
        "1E+2",
        "18446744073709551615",
        "18446744073709551616",
        "999999999999999999999x",
        "é",
        "1π",
        " 1",
        "1 ",
        "1_0",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    cases.push("0".repeat(128));
    cases.push(format!("{}18446744073709551615", "0".repeat(64)));
    for byte in 0..=127u8 {
        let value = char::from(byte);
        cases.extend([
            value.to_string(),
            format!("{value}12"),
            format!("1{value}2"),
            format!("12{value}"),
        ]);
    }
    for raw in cases {
        let expected = raw
            .bytes()
            .all(|byte| byte.is_ascii_digit())
            .then(|| raw.parse::<u64>().ok())
            .flatten();
        for (key, bit) in [(Key::Input, 1), (Key::Output, 2), (Key::Read, 4), (Key::Write, 8)] {
            let mut selector = Selector {
                depth: 1,
                components: Components {
                    input: Some(7),
                    output: Some(7),
                    read: Some(7),
                    write: Some(7),
                    invalid: 15,
                },
                ..Default::default()
            };
            selector.frames[0].role = Role::Usage;
            selector.frames[0].key = key;
            selector.on_number(&raw).unwrap();
            let actual = match key {
                Key::Input => selector.components.input,
                Key::Output => selector.components.output,
                Key::Read => selector.components.read,
                Key::Write => selector.components.write,
                _ => unreachable!(),
            };
            assert_eq!(actual, expected, "raw {raw:?}");
            assert_eq!(
                selector.components.invalid,
                if expected.is_some() { 15 & !bit } else { 15 }
            );
            assert!(selector.frames[0].key == Key::Other);
        }
    }
}
