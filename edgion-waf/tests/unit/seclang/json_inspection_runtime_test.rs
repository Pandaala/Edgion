use super::*;
use crate::seclang::{
    analyze_one_semantic_draft, compile_validated, parse_syntax, scan_source, validate_semantic_draft, CompilerLimits,
    JsonStreamLimit, PhaseDefaults, SemanticLimits, SemanticValidationContext, SourceLimits, SyntaxLimits,
    ValidationLimits,
};
fn compile(text: &str) -> CompiledGeneration {
    let source = scan_source("json-runtime", text.as_bytes(), SourceLimits::default()).unwrap();
    let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
    let draft = analyze_one_semantic_draft(syntax, SemanticLimits::default()).unwrap();
    let phases = [
        Phase::RequestHeaders,
        Phase::RequestBody,
        Phase::ResponseHeaders,
        Phase::ResponseBody,
        Phase::Logging,
    ]
    .map(|phase| PhaseDefaults::pass(phase, true, true));
    let context = SemanticValidationContext::try_new(Some(Phase::RequestBody), &phases).unwrap();
    let validated = validate_semantic_draft(draft, context, ValidationLimits::default()).unwrap();
    compile_validated(validated, None, CompilerLimits::default()).unwrap()
}
fn head(uri: &[u8]) -> RequestHead<'_> {
    RequestHead::new(
        b"POST / HTTP/1.1",
        b"POST",
        uri,
        uri,
        b"HTTP/1.1",
        b"192.0.2.1",
        b"json-runtime",
        &[],
    )
}

fn no_hit_transaction<'g>(
    generation: &'g CompiledGeneration,
    json: &[u8],
    uri: &[u8],
    limits: RuntimeLimits,
    result_limits: ResultLimits,
    force_cursor: bool,
) -> Transaction<'g> {
    let mut tx = Transaction::new(generation, head(uri), limits).unwrap();
    tx.force_json_cursor = force_cursor;
    tx.execute_request_headers().unwrap();
    tx.publish_request_body_with_json_limits(
        RequestBody::new(Bytes::copy_from_slice(json), BodyProcessorSelection::Json),
        JsonInspectionLimits {
            results: result_limits,
            ..JsonInspectionLimits::default()
        },
    )
    .unwrap();
    tx
}

fn assert_no_hit_cursor_parity(fast: &mut Transaction<'_>, cursor: &mut Transaction<'_>) {
    assert_eq!(fast.execute_request_body(), cursor.execute_request_body());
    assert_eq!(
        fast.json_budget.as_ref().unwrap().usage().work,
        cursor.json_budget.as_ref().unwrap().usage().work
    );
    assert_eq!(
        fast.json_budget.as_ref().unwrap().failure(),
        cursor.json_budget.as_ref().unwrap().failure()
    );
    assert_eq!(fast.detector_work_bytes, cursor.detector_work_bytes);
    assert_eq!(fast.ctl_work_bytes, cursor.ctl_work_bytes);
    assert_eq!(fast.next_phase(), cursor.next_phase());
    assert_eq!(fast.request_body_error(), cursor.request_body_error());
    assert_eq!(
        fast.capture_keys
            .iter()
            .map(|key| key.observed_bytes(&fast.request).unwrap())
            .collect::<Vec<_>>(),
        cursor
            .capture_keys
            .iter()
            .map(|key| key.observed_bytes(&cursor.request).unwrap())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        fast.tx
            .entries
            .iter()
            .map(|entry| (
                entry.key.observed_bytes(&fast.request).unwrap(),
                entry.value.observed_bytes(&fast.request).unwrap()
            ))
            .collect::<Vec<_>>(),
        cursor
            .tx
            .entries
            .iter()
            .map(|entry| (
                entry.key.observed_bytes(&cursor.request).unwrap(),
                entry.value.observed_bytes(&cursor.request).unwrap()
            ))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        fast.tx
            .matched_var
            .as_ref()
            .map(|value| value.observed_bytes(&fast.request).unwrap()),
        cursor
            .tx
            .matched_var
            .as_ref()
            .map(|value| value.observed_bytes(&cursor.request).unwrap())
    );
    assert_eq!(
        fast.tx
            .matched_var_name
            .as_ref()
            .map(|value| value.observed_bytes(&fast.request).unwrap()),
        cursor
            .tx
            .matched_var_name
            .as_ref()
            .map(|value| value.observed_bytes(&cursor.request).unwrap())
    );
    assert_eq!(
        fast.tx
            .matched_vars
            .iter()
            .map(|entry| (
                entry.key.observed_bytes(&fast.request).unwrap(),
                entry.value.observed_bytes(&fast.request).unwrap()
            ))
            .collect::<Vec<_>>(),
        cursor
            .tx
            .matched_vars
            .iter()
            .map(|entry| (
                entry.key.observed_bytes(&cursor.request).unwrap(),
                entry.value.observed_bytes(&cursor.request).unwrap()
            ))
            .collect::<Vec<_>>()
    );
}

#[test]
fn no_hit_summary_skips_candidate_storage_and_preserves_later_actions() {
    for recipe in ["", ",t:none", ",t:none,t:lowercase", ",t:lowercase,t:lowercase"] {
        let generation = compile(&format!(
            "SecRule ARGS \"@contains hit\" \"id:1,phase:2,pass{recipe},setvar:tx.wrong=1\"\nSecAction \"id:2,phase:2,pass,setvar:tx.later=1\""
        ));
        for json in [
            br#"{"a":"miss","b":""}"#.as_slice(),
            br#"{}"#,
            br#"[]"#,
            br#"{"a":"old","a":"miss"}"#,
        ] {
            let mut fast = no_hit_transaction(
                &generation,
                json,
                b"/",
                RuntimeLimits::default(),
                ResultLimits::default(),
                false,
            );
            let mut cursor = no_hit_transaction(
                &generation,
                json,
                b"/",
                RuntimeLimits::default(),
                ResultLimits::default(),
                true,
            );
            let before = fast.json_budget.as_ref().unwrap().usage().work;
            let count = fast.request.body.as_ref().unwrap().json().unwrap().argument_count();
            assert_no_hit_cursor_parity(&mut fast, &mut cursor);
            assert_eq!(fast.json_budget.as_ref().unwrap().usage().work - before, 4 * count);
            assert_eq!(fast.selected_scratch.capacity(), 0);
            assert_eq!(fast.tx_value(b"wrong"), None);
            assert_eq!(fast.tx_value(b"later"), Some(b"1".as_slice()));
            if count != 0 {
                assert!(cursor.selected_scratch.capacity() >= count);
            }
        }
    }
}

#[test]
fn no_hit_summary_exact_and_failing_budgets_match_cursor_counters() {
    let generation = compile("SecRule ARGS \"@contains hit\" \"id:1,phase:2,pass\"");
    let json = br#"{"a":"miss","b":""}"#;
    let baseline = no_hit_transaction(
        &generation,
        json,
        b"/",
        RuntimeLimits::default(),
        ResultLimits::default(),
        true,
    );
    let publication_work = baseline.json_budget.as_ref().unwrap().usage().work;
    for selected in [0, 1, 2, 3] {
        for logical in [0, 7, 8, 9] {
            for physical in [
                publication_work,
                publication_work + 1,
                publication_work + 7,
                publication_work + 8,
            ] {
                let limits = RuntimeLimits {
                    max_selected_values: selected,
                    max_operator_work_bytes: logical,
                    ..RuntimeLimits::default()
                };
                let result_limits = ResultLimits {
                    work: physical,
                    ..ResultLimits::default()
                };
                let mut fast = no_hit_transaction(&generation, json, b"/", limits, result_limits, false);
                let mut cursor = no_hit_transaction(&generation, json, b"/", limits, result_limits, true);
                assert_no_hit_cursor_parity(&mut fast, &mut cursor);
                assert_eq!(fast.match_scratch.capacity(), 0);
            }
        }
    }
}

#[test]
fn no_hit_summary_falls_back_for_changed_profiles_and_discarded_hits() {
    for (recipe, json, uri) in [
        (",t:lowercase", br#"{"a":"MISS"}"#.as_slice(), b"/".as_slice()),
        ("", br#"{"a":"hit","a":"miss"}"#, b"/"),
    ] {
        let generation = compile(&format!("SecRule ARGS \"@contains hit\" \"id:1,phase:2,pass{recipe}\""));
        let mut fast = no_hit_transaction(
            &generation,
            json,
            uri,
            RuntimeLimits::default(),
            ResultLimits::default(),
            false,
        );
        let mut cursor = no_hit_transaction(
            &generation,
            json,
            uri,
            RuntimeLimits::default(),
            ResultLimits::default(),
            true,
        );
        assert_no_hit_cursor_parity(&mut fast, &mut cursor);
        assert!(fast.selected_scratch.capacity() > 0);
    }
}

#[test]
fn no_hit_summary_ineligible_targets_operators_and_controls_keep_cursor() {
    for source in [
        "SecRule ARGS:json.a \"@contains hit\" \"id:1,phase:2,pass\"",
        "SecRule ARGS|!ARGS:json.b \"@contains hit\" \"id:1,phase:2,pass\"",
        "SecRule ARGS \"@contains hit\" \"id:1,phase:2,pass,multiMatch\"",
        "SecRule ARGS \"@rx (hit)\" \"id:1,phase:2,pass,capture\"",
        "SecRule ARGS \"@detectSQLi\" \"id:1,phase:2,pass\"",
        "SecRule ARGS \"@detectXSS\" \"id:1,phase:2,pass\"",
        "SecRule ARGS \"@contains hit\" \"id:1,phase:2,pass,t:length\"",
        "SecRule ARGS \"@contains hit\" \"id:1,phase:2,pass,chain\"\nSecRule TX:a \"@contains hit\" \"\"",
        "SecRule ARGS \"@contains hit\" \"id:1,phase:2,pass\"\nSecRule ARGS_NAMES \"@contains hit\" \"id:2,phase:2,pass\"",
        "SecAction \"id:1,phase:1,pass,ctl:ruleRemoveTargetByTag=check;ARGS:/^json[.]a$/\"\nSecRule ARGS \"@contains hit\" \"id:2,phase:2,pass,tag:check\"",
    ] {
        let generation = compile(source);
        let mut fast = no_hit_transaction(&generation, br#"{"a":"miss","b":"miss"}"#, b"/", RuntimeLimits::default(), ResultLimits::default(), false);
        let mut cursor = no_hit_transaction(&generation, br#"{"a":"miss","b":"miss"}"#, b"/", RuntimeLimits::default(), ResultLimits::default(), true);
        assert_no_hit_cursor_parity(&mut fast, &mut cursor);
        assert!(fast.selected_scratch.capacity() > 0, "{source}");
    }
}

#[test]
fn no_hit_summary_transform_errors_and_reference_mode_keep_cursor() {
    let generation = compile("SecRule ARGS \"@contains hit\" \"id:1,phase:2,pass,t:lowercase\"");
    let limits = RuntimeLimits {
        max_transformation_output_bytes: 3,
        ..RuntimeLimits::default()
    };
    let mut fast = no_hit_transaction(
        &generation,
        br#"{"a":"miss"}"#,
        b"/",
        limits,
        ResultLimits::default(),
        false,
    );
    let mut cursor = no_hit_transaction(
        &generation,
        br#"{"a":"miss"}"#,
        b"/",
        limits,
        ResultLimits::default(),
        true,
    );
    assert_no_hit_cursor_parity(&mut fast, &mut cursor);
    assert!(fast.selected_scratch.capacity() > 0);
    let mut reference = Transaction::new_with_evaluation(
        &generation,
        head(b"/"),
        RuntimeLimits::default(),
        EvaluationMode::Reference,
    )
    .unwrap();
    reference.execute_request_headers().unwrap();
    reference
        .publish_request_body(RequestBody::new(
            Bytes::from_static(br#"{"a":"miss"}"#),
            BodyProcessorSelection::Json,
        ))
        .unwrap();
    assert!(reference.execute_request_body().unwrap().findings().is_empty());
    assert!(reference.selected_scratch.capacity() > 0);
}

#[test]
fn no_hit_summary_retained_boundaries_and_growth_overlap_fall_back_safely() {
    let generation = compile("SecRule ARGS \"@contains hit\" \"id:1,phase:2,pass\"");
    let json = br#"{"a":"miss","b":"miss","c":"miss","d":"miss","e":"miss"}"#;
    let baseline = no_hit_transaction(
        &generation,
        json,
        b"/",
        RuntimeLimits::default(),
        ResultLimits::default(),
        true,
    );
    let fixed = baseline.retained_bytes();
    let item = size_of::<SelectedValue>();
    for extra in [0, item - 1, item, 4 * item, 5 * item - 1, 5 * item, 9 * item] {
        let limits = RuntimeLimits {
            max_retained_bytes: fixed + extra,
            max_selected_values: 5,
            ..RuntimeLimits::default()
        };
        let mut fast = no_hit_transaction(&generation, json, b"/", limits, ResultLimits::default(), false);
        let mut cursor = no_hit_transaction(&generation, json, b"/", limits, ResultLimits::default(), true);
        assert_no_hit_cursor_parity(&mut fast, &mut cursor);
        if extra == 9 * item {
            assert_eq!(fast.selected_scratch.capacity(), 0);
            assert_eq!(cursor.selected_scratch.capacity(), 5);
        } else {
            assert_eq!(fast.selected_scratch.capacity(), cursor.selected_scratch.capacity());
        }
    }
}

#[test]
fn no_hit_summary_reuses_existing_selected_capacity_without_changing_it() {
    let generation = compile(concat!(
        "SecRule ARGS \"@contains hit\" \"id:1,phase:2,pass\"\n",
        "SecRule ARGS \"@contains absent\" \"id:2,phase:2,pass\"\n",
    ));
    let mut fast = no_hit_transaction(
        &generation,
        br#"{"a":"miss"}"#,
        b"/",
        RuntimeLimits::default(),
        ResultLimits::default(),
        true,
    );
    let staged = fast.tx.try_clone_for_unit().unwrap();
    assert!(fast.evaluate_member_matches(0, 0, &staged, false).unwrap().is_empty());
    let capacity = fast.selected_scratch.capacity();
    assert!(capacity > 0);
    fast.force_json_cursor = false;
    let before = fast.json_budget.as_ref().unwrap().usage().work;
    assert!(fast.evaluate_member_matches(1, 1, &staged, false).unwrap().is_empty());
    assert_eq!(fast.selected_scratch.capacity(), capacity);
    assert_eq!(fast.json_budget.as_ref().unwrap().usage().work - before, 4);
}
fn query_tail_json(count: usize) -> Vec<u8> {
    format!(
        "{{{}}}",
        (0..count)
            .map(|index| format!("\"k{index}\":\"miss\""))
            .collect::<Vec<_>>()
            .join(",")
    )
    .into_bytes()
}

#[test]
fn query_no_hit_tail_preserves_all_matches_actions_names_and_argument_counts() {
    let json = query_tail_json(16);
    for (uri, queries, hits) in [
        (b"/".as_slice(), 0, 0),
        (b"/?q=miss".as_slice(), 1, 0),
        (b"/?q=hit".as_slice(), 1, 1),
        (b"/?a=HIT&b=miss&a=hi%74&empty=&novalue".as_slice(), 5, 2),
        (b"/?a=hit&a=HIT&a=hit&a=hit&a=hit".as_slice(), 5, 5),
    ] {
        let generation = compile(&format!(
            "SecRule ARGS \"@contains hit\" \"id:1,phase:2,pass,t:lowercase,setvar:tx.hits=+1\"\nSecRule &ARGS \"@eq {}\" \"id:2,phase:2,pass,setvar:tx.count=1\"\nSecAction \"id:3,phase:2,pass,setvar:tx.later=1\"",
            queries + 16
        ));
        let mut fast = no_hit_transaction(
            &generation,
            &json,
            uri,
            RuntimeLimits::default(),
            ResultLimits::default(),
            false,
        );
        let mut cursor = no_hit_transaction(
            &generation,
            &json,
            uri,
            RuntimeLimits::default(),
            ResultLimits::default(),
            true,
        );
        let before = fast.json_budget.as_ref().unwrap().usage().work;
        assert_no_hit_cursor_parity(&mut fast, &mut cursor);
        assert_eq!(fast.json_budget.as_ref().unwrap().usage().work - before, 4 * 16);
        assert_eq!(fast.tx_value(b"count"), Some(b"1".as_slice()));
        assert_eq!(fast.tx.matched_vars.len(), 0); // Final SecAction hides previous observers.
        let expected = hits.to_string();
        assert_eq!(fast.tx_value(b"hits"), (hits != 0).then_some(expected.as_bytes()));
        assert!(fast.selected_scratch.capacity() < cursor.selected_scratch.capacity());
        assert!(fast.selected_scratch.capacity() >= queries);
    }
}

#[test]
fn query_no_hit_tail_preserves_negation_denial_and_detection_only() {
    for (operator, json, uri) in [
        ("@contains hit", query_tail_json(16), b"/?a=hit&b=miss".as_slice()),
        (
            "!@contains hit",
            br#"{"a":"hit","b":"hit"}"#.to_vec(),
            b"/?q=miss".as_slice(),
        ),
    ] {
        let generation = compile(&format!(
            "SecRule ARGS \"{operator}\" \"id:1,phase:2,deny,status:418,setvar:tx.hit=1\"\nSecAction \"id:2,phase:2,pass,setvar:tx.later=1\""
        ));
        for mode in [RuleEngineMode::On, RuleEngineMode::DetectionOnly] {
            let mut fast = no_hit_transaction(
                &generation,
                &json,
                uri,
                RuntimeLimits::default(),
                ResultLimits::default(),
                false,
            );
            let mut cursor = no_hit_transaction(
                &generation,
                &json,
                uri,
                RuntimeLimits::default(),
                ResultLimits::default(),
                true,
            );
            fast.rule_engine_mode = mode;
            cursor.rule_engine_mode = mode;
            assert_no_hit_cursor_parity(&mut fast, &mut cursor);
            assert_eq!(fast.tx_value(b"hit"), Some(b"1".as_slice()));
            assert_eq!(
                fast.tx_value(b"later"),
                (mode == RuleEngineMode::DetectionOnly).then_some(b"1".as_slice())
            );
        }
    }
}

#[test]
fn query_no_hit_tail_selection_logical_and_physical_boundaries_match_cursor() {
    let generation = compile("SecRule ARGS \"@contains hit\" \"id:1,phase:2,pass\"");
    let json = query_tail_json(5);
    let uri = b"/?q=hit&m=miss";
    let baseline = no_hit_transaction(
        &generation,
        &json,
        uri,
        RuntimeLimits::default(),
        ResultLimits::default(),
        true,
    );
    let publication = baseline.json_budget.as_ref().unwrap().usage().work;
    let body_work = 2 * 5 * 4;
    let combined = body_work + 3 * (3 + 4) + 2 * 6 + 2 * size_of::<MemberMatch>();
    for selected in [0, 1, 6, 7, 8] {
        for logical in [0, body_work - 1, body_work, combined - 1, combined, combined + 1] {
            for physical in [0, 4, 5, 19, 20, 21] {
                let limits = RuntimeLimits {
                    max_selected_values: selected,
                    max_operator_work_bytes: logical,
                    ..RuntimeLimits::default()
                };
                let result_limits = ResultLimits {
                    work: publication + physical,
                    ..ResultLimits::default()
                };
                let mut fast = no_hit_transaction(&generation, &json, uri, limits, result_limits, false);
                let mut cursor = no_hit_transaction(&generation, &json, uri, limits, result_limits, true);
                assert_no_hit_cursor_parity(&mut fast, &mut cursor);
                if selected >= 7 && logical >= combined && physical >= 20 {
                    assert_eq!(fast.selected_scratch.capacity(), 4);
                    assert_eq!(cursor.selected_scratch.capacity(), selected.min(8));
                }
            }
        }
    }
}

#[test]
fn query_no_hit_tail_retained_match_growth_uses_complete_selection_limit() {
    let generation = compile("SecRule ARGS \"@contains hit\" \"id:1,phase:2,pass\"");
    let json = br#"{"a":"miss"}"#;
    let uri = b"/?a=hit&b=hit&c=hit&d=hit&e=hit";
    let baseline = no_hit_transaction(
        &generation,
        json,
        uri,
        RuntimeLimits::default(),
        ResultLimits::default(),
        true,
    );
    let fixed = baseline.retained_bytes();
    let selected = size_of::<SelectedValue>();
    let matched = size_of::<MemberMatch>();
    let full_peak = (10 * selected).max(6 * selected + 10 * matched);
    for extra in [
        6 * selected,
        10 * selected,
        6 * selected + 4 * matched,
        full_peak - 1,
        full_peak,
        full_peak + 1,
    ] {
        let limits = RuntimeLimits {
            max_retained_bytes: fixed + extra,
            max_selected_values: 6,
            ..RuntimeLimits::default()
        };
        let mut fast = no_hit_transaction(&generation, json, uri, limits, ResultLimits::default(), false);
        let mut cursor = no_hit_transaction(&generation, json, uri, limits, ResultLimits::default(), true);
        let left = fast.evaluate_member_matches(0, 0, &TxState::default(), false);
        let right = cursor.evaluate_member_matches(0, 0, &TxState::default(), false);
        assert_eq!(left.as_ref().err(), right.as_ref().err());
        assert_eq!(
            fast.json_budget.as_ref().unwrap().usage().work,
            cursor.json_budget.as_ref().unwrap().usage().work
        );
        if extra >= full_peak {
            let left = left.unwrap();
            let right = right.unwrap();
            assert_eq!(left.len(), 5);
            assert_eq!(right.len(), 5);
            assert_eq!(left.capacity(), 6);
            assert_eq!(right.capacity(), 6);
        }
        assert!(fast.selected_scratch.is_empty());
        assert!(fast.retained_bytes() <= limits.max_retained_bytes);
    }
}

#[test]
fn query_no_hit_tail_virtual_capacity_preserves_query_match_admission() {
    let generation = compile("SecRule ARGS \"@contains hit\" \"id:1,phase:2,pass\"");
    let json = query_tail_json(17);
    let uri = format!(
        "/?{}",
        (0..17)
            .map(|index| format!("q{index}=hit"))
            .collect::<Vec<_>>()
            .join("&")
    );
    let baseline = no_hit_transaction(
        &generation,
        &json,
        uri.as_bytes(),
        RuntimeLimits::default(),
        ResultLimits::default(),
        true,
    );
    let fixed = baseline.retained_bytes();
    let selected = size_of::<SelectedValue>();
    let matched = size_of::<MemberMatch>();
    let actual_peak = (96 * selected).max(32 * selected + 48 * matched);
    let hypothetical_peak = 64 * selected + 48 * matched;
    assert!(actual_peak < hypothetical_peak);
    let limits = RuntimeLimits {
        max_retained_bytes: fixed + actual_peak,
        ..RuntimeLimits::default()
    };
    let mut fast = no_hit_transaction(
        &generation,
        &json,
        uri.as_bytes(),
        limits,
        ResultLimits::default(),
        false,
    );
    let mut cursor = no_hit_transaction(
        &generation,
        &json,
        uri.as_bytes(),
        limits,
        ResultLimits::default(),
        true,
    );
    let before = fast.json_budget.as_ref().unwrap().usage().work;
    let left = fast.evaluate_member_matches(0, 0, &TxState::default(), false);
    let right = cursor.evaluate_member_matches(0, 0, &TxState::default(), false);
    assert_eq!(
        left.as_ref().err(),
        Some(&RuntimeError::RetainedByteLimit {
            limit: limits.max_retained_bytes
        })
    );
    assert_eq!(left.as_ref().err(), right.as_ref().err());
    assert_eq!(fast.json_budget.as_ref().unwrap().usage().work - before, 17);
    assert_eq!(
        fast.json_budget.as_ref().unwrap().usage().work,
        cursor.json_budget.as_ref().unwrap().usage().work
    );
    assert_eq!(fast.selected_scratch.capacity(), cursor.selected_scratch.capacity());
    assert!(fast.selected_scratch.capacity() >= 34);
}

#[test]
fn query_no_hit_tail_reuses_existing_selected_capacity_without_shadow_retention() {
    let generation = compile(
        "SecRule ARGS \"@contains hit\" \"id:1,phase:2,pass\"\nSecRule ARGS \"@contains hit\" \"id:2,phase:2,pass\"",
    );
    let json = query_tail_json(8);
    let mut tx = no_hit_transaction(
        &generation,
        &json,
        b"/?a=hit&b=miss",
        RuntimeLimits::default(),
        ResultLimits::default(),
        true,
    );
    assert_eq!(
        tx.evaluate_member_matches(0, 0, &TxState::default(), false)
            .unwrap()
            .len(),
        1
    );
    let capacity = tx.selected_scratch.capacity();
    let pointer = tx.selected_scratch.as_ptr();
    let retained = tx.retained_bytes();
    let before = tx.json_budget.as_ref().unwrap().usage().work;
    tx.force_json_cursor = false;
    assert_eq!(
        tx.evaluate_member_matches(1, 1, &TxState::default(), false)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(tx.selected_scratch.capacity(), capacity);
    assert_eq!(tx.selected_scratch.as_ptr(), pointer);
    assert_eq!(tx.retained_bytes(), retained);
    assert_eq!(tx.json_budget.as_ref().unwrap().usage().work - before, 4 * 8);
}

#[test]
fn query_no_hit_tail_stop_first_and_query_errors_charge_selection_only() {
    let json = query_tail_json(16);
    let generation = compile("SecRule ARGS \"@contains hit\" \"id:1,phase:2,pass,t:lowercase\"");
    let uri = b"/?a=HIT&b=miss";
    let mut fast = no_hit_transaction(
        &generation,
        &json,
        uri,
        RuntimeLimits::default(),
        ResultLimits::default(),
        false,
    );
    let mut cursor = no_hit_transaction(
        &generation,
        &json,
        uri,
        RuntimeLimits::default(),
        ResultLimits::default(),
        true,
    );
    let fixed = fast.retained_bytes();
    let before = fast.json_budget.as_ref().unwrap().usage().work;
    let left = fast.evaluate_member_matches(0, 0, &TxState::default(), true).unwrap();
    let right = cursor.evaluate_member_matches(0, 0, &TxState::default(), true).unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(right.len(), 1);
    assert_eq!(
        left[0]
            .matched_var
            .as_ref()
            .unwrap()
            .observed_bytes(&fast.request)
            .unwrap(),
        b"hit"
    );
    assert_eq!(
        left[0]
            .matched_var_name
            .as_ref()
            .unwrap()
            .observed_bytes(&fast.request)
            .unwrap(),
        b"ARGS:a"
    );
    assert_eq!(fast.json_budget.as_ref().unwrap().usage().work - before, 16);
    assert_eq!(
        fast.json_budget.as_ref().unwrap().usage().work,
        cursor.json_budget.as_ref().unwrap().usage().work
    );
    assert_eq!(
        fast.retained_bytes(),
        fixed + fast.selected_scratch.capacity() * size_of::<SelectedValue>()
    );
    assert!(fast.selected_scratch.capacity() < cursor.selected_scratch.capacity());
    for selected in [17, 18] {
        for physical in [15, 16, 64] {
            let limits = RuntimeLimits {
                max_selected_values: selected,
                ..RuntimeLimits::default()
            };
            let result_limits = ResultLimits {
                work: before + physical,
                ..ResultLimits::default()
            };
            let mut fast = no_hit_transaction(&generation, &json, uri, limits, result_limits, false);
            let mut cursor = no_hit_transaction(&generation, &json, uri, limits, result_limits, true);
            let left = fast.evaluate_member_matches(0, 0, &TxState::default(), true);
            let right = cursor.evaluate_member_matches(0, 0, &TxState::default(), true);
            assert_eq!(left.as_ref().err(), right.as_ref().err());
            assert_eq!(left.is_ok(), selected == 18 && physical >= 16);
            assert_eq!(
                fast.json_budget.as_ref().unwrap().usage().work,
                cursor.json_budget.as_ref().unwrap().usage().work
            );
        }
    }

    for (operator, recipe, body, limits, error) in [
        (
            "@contains hit",
            ",t:lowercase",
            br#"{"a":"x","b":"x","c":"x","d":"x","e":"x"}"#.as_slice(),
            RuntimeLimits {
                max_transformation_output_bytes: 3,
                ..RuntimeLimits::default()
            },
            None,
        ),
        (
            "@gt 100",
            "",
            br#"{"a":0,"b":0,"c":0,"d":0,"e":0}"#.as_slice(),
            RuntimeLimits::default(),
            Some(RuntimeError::IntegerConversion),
        ),
    ] {
        let generation = compile(&format!("SecRule ARGS \"{operator}\" \"id:1,phase:2,pass{recipe}\""));
        let mut fast = no_hit_transaction(
            &generation,
            body,
            b"/?q=TOOLONG",
            limits,
            ResultLimits::default(),
            false,
        );
        let mut cursor = no_hit_transaction(&generation, body, b"/?q=TOOLONG", limits, ResultLimits::default(), true);
        let before = fast.json_budget.as_ref().unwrap().usage().work;
        let left = fast.evaluate_member_matches(0, 0, &TxState::default(), false);
        let right = cursor.evaluate_member_matches(0, 0, &TxState::default(), false);
        assert_eq!(left.as_ref().err(), right.as_ref().err());
        if let Some(error) = error {
            assert_eq!(left.as_ref().err(), Some(&error));
        } else {
            assert!(matches!(left, Err(RuntimeError::Transformation { .. })));
        }
        assert_eq!(fast.json_budget.as_ref().unwrap().usage().work - before, 5);
        assert_eq!(
            fast.json_budget.as_ref().unwrap().usage().work,
            cursor.json_budget.as_ref().unwrap().usage().work
        );
        assert_eq!(fast.selected_scratch.capacity(), 4);
        assert_eq!(cursor.selected_scratch.capacity(), 8);
        assert_eq!(fast.match_scratch_bytes, 0);
    }
}

#[test]
fn query_no_hit_tail_replacement_malformed_reference_and_exclusions_keep_semantics() {
    for (source, json) in [
        ("SecRule ARGS \"@contains hit\" \"id:1,phase:2,pass,setvar:tx.hit=+1\"", br#"{"same":"old","same":"miss","b":"miss","c":"miss","d":"miss","e":"miss"}"#.as_slice()),
        ("SecAction \"id:1,phase:1,pass,ctl:ruleRemoveTargetByTag=check;ARGS:q\"\nSecRule ARGS \"@contains hit\" \"id:2,phase:2,pass,tag:check,setvar:tx.hit=+1\"", br#"{"a":"miss","b":"miss","c":"miss","d":"miss","e":"miss"}"#.as_slice()),
    ] {
        let generation = compile(source);
        let mut fast = no_hit_transaction(&generation, json, b"/?q=hit", RuntimeLimits::default(), ResultLimits::default(), false);
        let mut cursor = no_hit_transaction(&generation, json, b"/?q=hit", RuntimeLimits::default(), ResultLimits::default(), true);
        assert_no_hit_cursor_parity(&mut fast, &mut cursor);
        if source.contains("ruleRemoveTarget") {
            assert_eq!(fast.tx_value(b"hit"), None);
            assert!(fast.selected_scratch.capacity() >= 5);
        }
    }
    let malformed_generation = compile("SecRule ARGS \"@contains hit\" \"id:1,phase:2,pass,setvar:tx.hit=1\"");
    let mut fast = Transaction::new(&malformed_generation, head(b"/?q=hit"), RuntimeLimits::default()).unwrap();
    let mut cursor = Transaction::new(&malformed_generation, head(b"/?q=hit"), RuntimeLimits::default()).unwrap();
    cursor.force_json_cursor = true;
    fast.execute_request_headers().unwrap();
    cursor.execute_request_headers().unwrap();
    let publish = |tx: &mut Transaction<'_>| {
        tx.publish_request_body(RequestBody::new(
            Bytes::from_static(br#"{"a":"miss""#),
            BodyProcessorSelection::Json,
        ))
    };
    assert_eq!(publish(&mut fast), publish(&mut cursor));
    assert_eq!(fast.request_body_error(), cursor.request_body_error());
    assert!(matches!(fast.execute_request_body(), Err(RuntimeError::Body { .. })));
    assert!(matches!(cursor.execute_request_body(), Err(RuntimeError::Body { .. })));
    assert_eq!(fast.tx_value(b"hit"), None);
    assert_eq!(fast.selected_scratch.capacity(), 0);
    let generation = compile("SecRule ARGS \"@contains hit\" \"id:1,phase:2,pass\"");
    let json = query_tail_json(16);
    let mut reference = Transaction::new_with_evaluation(
        &generation,
        head(b"/?q=hit"),
        RuntimeLimits::default(),
        EvaluationMode::Reference,
    )
    .unwrap();
    reference.execute_request_headers().unwrap();
    reference
        .publish_request_body(RequestBody::new(Bytes::from(json), BodyProcessorSelection::Json))
        .unwrap();
    assert_eq!(reference.execute_request_body().unwrap().findings().len(), 1);
    assert!(reference.selected_scratch.capacity() >= 17);
}

#[test]
fn query_no_hit_tail_saved_capacity_adds_charged_post_member_headroom() {
    let payload = "x".repeat(4096);
    let generation = compile(&format!(
        "SecRule ARGS \"@contains hit\" \"id:1,phase:2,pass,setvar:tx.payload={payload}\""
    ));
    let json = query_tail_json(16);
    let uri = b"/?q=hit";
    let mut baseline = no_hit_transaction(
        &generation,
        &json,
        uri,
        RuntimeLimits::default(),
        ResultLimits::default(),
        false,
    );
    baseline.execute_request_body().unwrap();
    let exact = baseline.retained_bytes();
    let mut fast = no_hit_transaction(
        &generation,
        &json,
        uri,
        RuntimeLimits {
            max_retained_bytes: exact,
            ..RuntimeLimits::default()
        },
        ResultLimits::default(),
        false,
    );
    let mut cursor = no_hit_transaction(
        &generation,
        &json,
        uri,
        RuntimeLimits {
            max_retained_bytes: exact,
            ..RuntimeLimits::default()
        },
        ResultLimits::default(),
        true,
    );
    assert!(fast.execute_request_body().is_ok());
    assert_eq!(
        cursor.execute_request_body(),
        Err(RuntimeError::RetainedByteLimit { limit: exact })
    );
    assert_eq!(fast.retained_bytes(), exact);
    assert_eq!(fast.tx_value(b"payload"), Some(payload.as_bytes()));
    assert_eq!(cursor.tx_value(b"payload"), None);
    assert_eq!(
        fast.json_budget.as_ref().unwrap().usage().work,
        cursor.json_budget.as_ref().unwrap().usage().work
    );
    assert_eq!(
        fast.retained_bytes(),
        fast.request.retained_bytes + fast.scratch_bytes + fast.tx.retained_bytes
    );
    for limits in [
        RuntimeLimits {
            max_retained_bytes: exact - 1,
            ..RuntimeLimits::default()
        },
        RuntimeLimits {
            max_tx_bytes: payload.len() - 1,
            ..RuntimeLimits::default()
        },
        RuntimeLimits {
            max_tx_variables: 0,
            ..RuntimeLimits::default()
        },
        RuntimeLimits {
            max_setvar_actions: 0,
            ..RuntimeLimits::default()
        },
    ] {
        let mut fast = no_hit_transaction(&generation, &json, uri, limits, ResultLimits::default(), false);
        let result = fast.execute_request_body();
        assert!(result.is_err());
        assert_eq!(fast.tx_value(b"payload"), None);
        assert!(fast.retained_bytes() <= limits.max_retained_bytes);
        if limits.max_retained_bytes == RuntimeLimits::default().max_retained_bytes {
            let mut cursor = no_hit_transaction(&generation, &json, uri, limits, ResultLimits::default(), true);
            assert_eq!(fast.json_budget.as_ref().unwrap().usage().work, {
                assert_eq!(result, cursor.execute_request_body());
                cursor.json_budget.as_ref().unwrap().usage().work
            });
            assert_eq!(cursor.tx_value(b"payload"), None);
        }
    }
}

fn compare(source: &str, json: &[u8], form: &[u8], uri: &[u8], limits: RuntimeLimits) {
    let generation = compile(source);
    for mode in [EvaluationMode::Accelerated, EvaluationMode::Reference] {
        let mut fused = Transaction::new(&generation, head(uri), limits).unwrap();
        let mut ordinary = Transaction::new(&generation, head(uri), limits).unwrap();
        fused.evaluation_mode = mode;
        ordinary.evaluation_mode = mode;
        fused.execute_request_headers().unwrap();
        ordinary.execute_request_headers().unwrap();
        fused
            .publish_request_body_with_json_limits(
                RequestBody::new(Bytes::copy_from_slice(json), BodyProcessorSelection::Json),
                JsonInspectionLimits::default(),
            )
            .unwrap();
        ordinary
            .publish_request_body(RequestBody::new(
                bytes::Bytes::copy_from_slice(form),
                BodyProcessorSelection::UrlEncoded,
            ))
            .unwrap();
        let actual = fused.execute_request_body();
        let expected = ordinary.execute_request_body();
        assert_eq!(actual, expected);
        assert_eq!(
            fused
                .tx
                .matched_var
                .as_ref()
                .map(|value| value.bytes(&fused.request, fused.json_budget.as_mut()).unwrap()),
            ordinary
                .tx
                .matched_var
                .as_ref()
                .map(|value| value.bytes(&ordinary.request, ordinary.json_budget.as_mut()).unwrap()),
        );
        assert_eq!(
            fused
                .tx
                .matched_var_name
                .as_ref()
                .map(|name| name.bytes(&fused.request, fused.json_budget.as_mut()).unwrap()),
            ordinary
                .tx
                .matched_var_name
                .as_ref()
                .map(|name| name.bytes(&ordinary.request, ordinary.json_budget.as_mut()).unwrap())
        );
        assert_eq!(
            fused
                .tx
                .entries
                .iter()
                .map(|entry| (
                    entry.key.observed_bytes(&fused.request).unwrap(),
                    entry.value.observed_bytes(&fused.request).unwrap()
                ))
                .collect::<Vec<_>>(),
            ordinary
                .tx
                .entries
                .iter()
                .map(|entry| (
                    entry.key.observed_bytes(&ordinary.request).unwrap(),
                    entry.value.observed_bytes(&ordinary.request).unwrap()
                ))
                .collect::<Vec<_>>()
        );
        assert_eq!(fused.detector_work_bytes, ordinary.detector_work_bytes);
    }
}

#[test]
fn shared_capture_program_rules_replay_independent_actions_and_capture_limits() {
    let source = concat!(
        "SecAction \"id:10,phase:2,pass,setvar:'tx.count=0'\"\n",
        "SecRule ARGS \"@rx ^(?P<word>a)(?P<empty>)$\" \"id:1,phase:2,pass,capture,t:none,setvar:'tx.count=+1',setvar:'tx.first=%{TX.word}'\"\n",
        "SecRule ARGS \"@rx ^(?P<word>a)(?P<empty>)$\" \"id:2,phase:2,pass,capture,t:none,setvar:'tx.count=+1',setvar:'tx.second=%{TX.word}'\"\n",
        "SecRule ARGS \"!@rx ^(?P<word>a)(?P<empty>)$\" \"id:3,phase:2,pass,capture,t:none,setvar:'tx.count=+1'\"\n",
    );
    for (entries, bytes) in [(0, 0), (4, 15), (5, 14), (5, 15), (16, 1024)] {
        compare(
            source,
            br#"{"x":"a"}"#,
            b"json.x=a",
            b"/",
            RuntimeLimits {
                max_capture_entries: entries,
                max_capture_bytes: bytes,
                ..RuntimeLimits::default()
            },
        );
    }
    let generation = compile(source);
    assert_eq!(generation.capture_regexes.len(), 1);
    let mut tx = no_hit_transaction(
        &generation,
        br#"{"x":"a"}"#,
        b"/",
        RuntimeLimits::default(),
        ResultLimits::default(),
        false,
    );
    tx.execute_request_body().unwrap();
    assert_eq!(tx.tx_value(b"count"), Some(b"2".as_slice()));
    assert_eq!(tx.tx_value(b"first"), Some(b"a".as_slice()));
    assert_eq!(tx.tx_value(b"second"), Some(b"a".as_slice()));
}
#[test]
fn static_mixed_targets_captures_transforms_and_chains_match_form_runtime() {
    compare(concat!(
        "SecRule ARGS|ARGS_NAMES \"@rx (hit)\" \"id:1,phase:2,pass,capture,t:lowercase,multiMatch,setvar:'tx.captured=%{TX.1}'\"\n",
        "SecRule ARGS:json.a \"@streq hit\" \"id:2,phase:2,pass,t:lowercase,chain,setvar:'tx.root=%{MATCHED_VAR}'\"\n",
        "SecRule TX:root \"@streq hit\" \"setvar:'tx.chain=done'\"\n",
        "SecRule ARGS:json.a|ARGS:json.a|!ARGS:json.b \"!@contains absent\" \"id:3,phase:2,pass\"\n",
    ), br#"{"b":"miss","a":"HIT"}"#, b"json.a=HIT&json.b=miss", b"/?q=hit", RuntimeLimits::default());
}

#[test]
fn sealed_replay_and_chain_clones_reference_one_payload() {
    let generation = compile(concat!(
        "SecRule ARGS \"@pm hit first\" \"id:1,phase:2,pass,t:lowercase\"\n",
        "SecRule ARGS \"@pm hit second\" \"id:2,phase:2,pass,t:lowercase\"\n",
    ));
    for padding in [0, 262_144] {
        let mut input = b"{\"a\":\"".to_vec();
        input.extend(std::iter::repeat_n(b'x', padding));
        input.extend_from_slice(b"HIT\"}");
        let mut tx = Transaction::new(&generation, head(b"/"), RuntimeLimits::default()).unwrap();
        tx.execute_request_headers().unwrap();
        tx.publish_request_body(RequestBody::new(Bytes::from(input), BodyProcessorSelection::Json))
            .unwrap();
        let publication = tx.request.body.as_ref().unwrap().json().unwrap();
        assert!(publication.source.is_none());
        assert_eq!(tx.json_budget.as_ref().unwrap().usage().matched_bytes, padding + 3);

        let mut first = tx.evaluate_member_matches(0, 0, &TxState::default(), true).unwrap();
        let second = tx.evaluate_member_matches(1, 1, &TxState::default(), true).unwrap();
        let original = first[0].matched_var.as_ref().unwrap();
        assert!(matches!(original, MatchValue::Json(_)));
        assert_eq!(original, second[0].matched_var.as_ref().unwrap());
        let bytes = original.bytes(&tx.request, tx.json_budget.as_mut()).unwrap();
        assert_eq!(&bytes[padding..], b"hit");
        let pointer = bytes.as_ptr();

        let mut staged = TxState::default();
        apply_member_match(
            &mut staged,
            &mut Vec::new(),
            first.pop().unwrap(),
            tx.limits,
            &tx.request,
            tx.json_budget.as_mut(),
        )
        .unwrap();
        let branch = staged.try_clone().unwrap();
        assert_eq!(staged.retained_bytes, branch.retained_bytes);
        assert!(staged.retained_bytes >= 2 * (padding + 3));
        for state in [&staged, &branch] {
            for value in [state.matched_var.as_ref().unwrap(), &state.matched_vars[0].value] {
                assert_eq!(
                    value.bytes(&tx.request, tx.json_budget.as_mut()).unwrap().as_ptr(),
                    pointer
                );
            }
            for selected in [SelectedBytes::MatchedVar, SelectedBytes::MatchedVars(0)] {
                assert_eq!(
                    SelectedValue {
                        collection: CollectionKind::MatchedVar,
                        name: None,
                        value: selected
                    }
                    .bytes(&tx.request, state, None, tx.json_budget.as_mut())
                    .unwrap()
                    .as_ptr(),
                    pointer
                );
            }
        }
        tx.json_budget.as_mut().unwrap().work(usize::MAX).unwrap_err();
        assert!(staged
            .matched_var
            .as_ref()
            .unwrap()
            .bytes(&tx.request, tx.json_budget.as_mut())
            .is_err());
    }
}

#[test]
fn shared_json_observers_preserve_captures_chain_macros_and_rollback() {
    compare(concat!(
        "SecRule ARGS:json.a \"@rx (hit)\" \"id:1,phase:2,pass,capture,t:lowercase,chain,setvar:'tx.root=%{MATCHED_VAR}'\"\n",
        "SecRule ARGS:json.b \"@streq miss\" \"chain\"\n",
        "SecRule MATCHED_VARS:ARGS:json.a \"@streq hit\" \"chain\"\n",
        "SecRule MATCHED_VAR \"@streq hit\" \"setvar:'tx.shared=%{MATCHED_VAR}',setvar:'tx.capture=%{TX.1}'\"\n",
        "SecRule ARGS:json.a \"@streq hit\" \"id:2,phase:2,pass,t:lowercase,chain,setvar:tx.leaked=bad\"\n",
        "SecRule ARGS:json.b \"@streq absent\"\n",
        "SecRule TX:shared \"@streq hit\" \"id:3,phase:2,pass\"\n",
        "SecRule &TX:leaked \"@eq 0\" \"id:4,phase:2,pass\"\n",
    ), br#"{"a":"old","a":"HIT","b":"miss"}"#, b"json.a=HIT&json.b=miss", b"/", RuntimeLimits::default());
}

#[test]
fn shared_large_match_still_enforces_transaction_byte_limit() {
    let generation = compile("SecRule ARGS \"@contains hit\" \"id:1,phase:2,pass\"\n");
    let mut input = b"{\"a\":\"".to_vec();
    input.extend(std::iter::repeat_n(b'x', 8192));
    input.extend_from_slice(b"hit\"}");
    let value_len = 8192 + 3;
    let mut tx = Transaction::new(
        &generation,
        head(b"/"),
        RuntimeLimits {
            max_tx_bytes: 2 * value_len - 1,
            ..RuntimeLimits::default()
        },
    )
    .unwrap();
    tx.execute_request_headers().unwrap();
    tx.publish_request_body(RequestBody::new(Bytes::from(input), BodyProcessorSelection::Json))
        .unwrap();
    assert!(matches!(
        tx.execute_request_body(),
        Err(RuntimeError::TxByteLimit { .. })
    ));
    assert!(tx.tx.matched_var.is_none());
    assert!(tx.tx.matched_vars.is_empty());
}
#[test]
fn dynamic_named_name_and_length_modes_match_form_runtime() {
    compare(
        concat!(
            "SecAction \"id:1,phase:1,pass,setvar:'tx.expected=hit',setvar:'tx.length=3'\"\n",
            "SecRule ARGS:json.a \"@streq %{TX.expected}\" \"id:2,phase:2,pass\"\n",
            "SecRule ARGS_NAMES \"@contains %{TX.expected}\" \"id:3,phase:2,pass\"\n",
            "SecRule ARGS \"@eq %{TX.length}\" \"id:4,phase:2,pass,t:length\"\n",
            "SecAction \"id:5,phase:2,pass,setvar:'tx.named=%{ARGS.json.a}'\"\n",
        ),
        br#"{"hit":"longer","a":"hit"}"#,
        b"json.a=hit&json.hit=longer",
        b"/",
        RuntimeLimits::default(),
    );
}
#[test]
fn summary_count_pushes_one_value_and_exclusion_order_matches_form() {
    for targets in ["&ARGS", "&ARGS|!ARGS:json.a", "&ARGS|!ARGS"] {
        compare(
            &format!("SecRule {targets} \"@eq 3\" \"id:1,phase:2,pass\"\n"),
            br#"{"a":1,"b":2,"c":3}"#,
            b"json.a=1&json.b=2&json.c=3",
            b"/",
            RuntimeLimits {
                max_selected_values: 1,
                ..RuntimeLimits::default()
            },
        );
    }
}
#[test]
fn ordered_rule_limits_remain_identical_after_static_fact_inspection() {
    for selected in [0, 1, 4] {
        for work in [0, 8, 80, 4096] {
            for captures in [0, 8] {
                compare(
                    "SecRule ARGS \"@rx (hit)\" \"id:1,phase:2,pass,capture,t:lowercase,multiMatch\"\n",
                    br#"{"a":"HIT","b":"hit"}"#,
                    b"json.a=HIT&json.b=hit",
                    b"/",
                    RuntimeLimits {
                        max_selected_values: selected,
                        max_operator_work_bytes: work,
                        max_capture_bytes: captures,
                        ..RuntimeLimits::default()
                    },
                );
            }
        }
    }
}
#[test]
fn raw_visibility_reuses_shared_source_and_physical_failure_is_sticky() {
    let generation = compile("SecRule REQUEST_BODY \"@contains a\" \"id:1,phase:2,pass\"\n");
    let mut tx = Transaction::new(&generation, head(b"/"), RuntimeLimits::default()).unwrap();
    tx.execute_request_headers().unwrap();
    tx.tx.force_request_body_variable = Some(super::super::super::OnOff::On);
    let source = Bytes::from_static(br#"{"a":1}"#);
    let pointer = source.as_ptr();
    tx.publish_request_body_with_json_limits(
        RequestBody::new(source.clone(), BodyProcessorSelection::Json),
        JsonInspectionLimits::default(),
    )
    .unwrap();
    assert_eq!(
        tx.request
            .body
            .as_ref()
            .unwrap()
            .json()
            .unwrap()
            .source
            .as_ref()
            .unwrap()
            .as_ptr(),
        pointer
    );
    assert_eq!(tx.execute_request_body().unwrap().findings().len(), 1);
    let mut failed = Transaction::new(&generation, head(b"/"), RuntimeLimits::default()).unwrap();
    failed.execute_request_headers().unwrap();
    let limits = JsonInspectionLimits {
        lexical: JsonStreamLimits {
            max_source_bytes: 0,
            ..JsonInspectionLimits::default().lexical
        },
        ..JsonInspectionLimits::default()
    };
    let error = failed
        .publish_request_body_with_json_limits(RequestBody::new(source, BodyProcessorSelection::Json), limits)
        .unwrap_err();
    assert_eq!(failed.execute_request_body().err(), Some(error));
    assert_eq!(failed.execute_logging().err(), Some(error));
    assert_eq!(
        failed.publish_request_body(RequestBody::new(
            bytes::Bytes::copy_from_slice(b""),
            BodyProcessorSelection::Raw
        )),
        Err(error)
    );
}
#[test]
fn named_macro_first_canonical_collision_and_query_priority_are_preserved() {
    let source = "SecAction \"id:1,phase:2,pass,setvar:'tx.named=%{ARGS.json.a.b}',setvar:'tx.missing=%{ARGS.json.missing}',setvar:'tx.empty=%{ARGS.json.empty}'\"\n";
    for uri in [b"/".as_slice(), b"/?json.a.b=query"] {
        compare(
            source,
            br#"{"a.b":"literal","a":{"b":"overwritten"},"a":{"b":"nested"},"empty":""}"#,
            b"json.a.b=nested&json.a.b=literal&json.empty=",
            uri,
            RuntimeLimits::default(),
        );
    }
}

#[test]
fn empty_and_malformed_publications_charge_persistent_owner_and_metadata() {
    let generation = compile("SecAction \"id:1,phase:2,pass\"\n");
    assert!(size_of::<RequestBodyPublication<'_>>() <= 64);
    for source in [b"".as_slice(), b"!"] {
        let make = || {
            let mut tx = Transaction::new(&generation, head(b"/"), RuntimeLimits::default()).unwrap();
            tx.rule_engine_mode = RuleEngineMode::DetectionOnly;
            tx.execute_request_headers().unwrap();
            tx
        };
        let mut tx = make();
        tx.publish_request_body_with_json_limits(
            RequestBody::new(Bytes::copy_from_slice(source), BodyProcessorSelection::Json),
            JsonInspectionLimits::default(),
        )
        .unwrap();
        let RequestBodyPublication::Json(owner) = tx.request.body.as_ref().unwrap() else {
            panic!("JSON publication")
        };
        let physical = owner.capacity() * size_of::<JsonPublication<'_>>();
        assert_eq!(tx.json_budget.as_ref().unwrap().usage().retained_bytes, physical);
        assert_eq!(
            owner[0].source.as_ref().map(Bytes::len),
            if source.is_empty() { None } else { Some(1) }
        );
        for limit in [0, physical - 1, physical, physical + 1] {
            let mut tx = make();
            let result = tx.publish_request_body_with_json_limits(
                RequestBody::new(Bytes::copy_from_slice(source), BodyProcessorSelection::Json),
                JsonInspectionLimits {
                    results: ResultLimits {
                        retained_bytes: limit,
                        ..ResultLimits::default()
                    },
                    ..JsonInspectionLimits::default()
                },
            );
            assert_eq!(result.is_ok(), limit >= physical);
            if let Err(error) = result {
                assert_eq!(
                    error,
                    RuntimeError::JsonResultLimit {
                        kind: JsonResultLimit::RetainedBytes
                    }
                );
                assert!(tx.request.body.is_none());
                assert_eq!(tx.execute_logging().err(), Some(error));
            }
        }
    }
}

#[test]
fn malformed_fallback_keeps_cumulative_parse_work_and_peak() {
    let generation = compile("SecRule ARGS \"@contains hit\" \"id:1,phase:2,pass\"\n");
    let input = br#"{"a":"hit"} trailing"#;
    let make = || {
        let mut tx = Transaction::new(&generation, head(b"/"), RuntimeLimits::default()).unwrap();
        tx.rule_engine_mode = RuleEngineMode::DetectionOnly;
        tx.execute_request_headers().unwrap();
        tx
    };
    let mut tx = make();
    tx.publish_request_body_with_json_limits(
        RequestBody::new(Bytes::copy_from_slice(input), BodyProcessorSelection::Json),
        JsonInspectionLimits::default(),
    )
    .unwrap();
    let usage = tx.json_budget.as_ref().unwrap().usage();
    assert!(usage.abandoned > 0);
    assert!(usage.work > usage.retained_bytes);
    for limit in [usage.work - 1, usage.work, usage.work + 1] {
        let mut tx = make();
        let result = tx.publish_request_body_with_json_limits(
            RequestBody::new(Bytes::copy_from_slice(input), BodyProcessorSelection::Json),
            JsonInspectionLimits {
                results: ResultLimits {
                    work: limit,
                    ..ResultLimits::default()
                },
                ..JsonInspectionLimits::default()
            },
        );
        assert_eq!(result.is_ok(), limit >= usage.work);
        if result.is_err() {
            assert_eq!(
                result,
                Err(RuntimeError::JsonResultLimit {
                    kind: JsonResultLimit::Work
                })
            );
            assert!(tx.request.body.is_none());
        }
    }
}

#[test]
fn replay_work_failure_blocks_later_body_independent_entry_points() {
    let generation = compile(concat!(
        "SecRule ARGS \"@contains hit\" \"id:1,phase:2,pass\"\n",
        "SecAction \"id:2,phase:5,pass\"\n"
    ));
    let input = Bytes::from_static(br#"{"a":"hit"}"#);
    let mut baseline = Transaction::new(&generation, head(b"/"), RuntimeLimits::default()).unwrap();
    baseline.execute_request_headers().unwrap();
    baseline
        .publish_request_body_with_json_limits(
            RequestBody::new(input.clone(), BodyProcessorSelection::Json),
            JsonInspectionLimits::default(),
        )
        .unwrap();
    let publication_work = baseline.json_budget.as_ref().unwrap().usage().work;
    let mut tx = Transaction::new(&generation, head(b"/"), RuntimeLimits::default()).unwrap();
    tx.execute_request_headers().unwrap();
    tx.publish_request_body_with_json_limits(
        RequestBody::new(input, BodyProcessorSelection::Json),
        JsonInspectionLimits {
            results: ResultLimits {
                work: publication_work,
                ..ResultLimits::default()
            },
            ..JsonInspectionLimits::default()
        },
    )
    .unwrap();
    let error = RuntimeError::JsonResultLimit {
        kind: JsonResultLimit::Work,
    };
    assert_eq!(tx.execute_request_body().err(), Some(error));
    assert_eq!(tx.execute_logging().err(), Some(error));
    assert_eq!(tx.preflight_request_body(BodyProcessorSelection::Raw), Err(error));
}

#[test]
fn scoped_chunk_physical_boundaries_are_cumulative_and_terminal() {
    let generation = compile("SecRule ARGS \"@rx (?P<word>hit)\" \"id:1,phase:2,pass,capture,t:lowercase\"\n");
    let long_value = format!("{}\\u0048IT", "x".repeat(256));
    let input = format!("{{\"a\":\"{long_value}\",\"b\":\"hit\"}}");
    let run = |limits: JsonInspectionLimits| {
        let mut tx = Transaction::new(&generation, head(b"/"), RuntimeLimits::default()).unwrap();
        tx.execute_request_headers().unwrap();
        let mut inspection = tx.start_json_inspection(limits, None).unwrap();
        let mut previous = JsonInspectionUsage::default();
        let mut failure = None;
        for chunk in input.as_bytes().chunks(7) {
            if let Err(error) = inspection.feed(chunk) {
                failure = Some(error);
                break;
            }
            let usage = inspection.usage();
            assert!(usage.inspected_source_bytes > previous.inspected_source_bytes);
            assert!(usage.lexical_work >= previous.lexical_work);
            assert!(usage.result_work >= previous.result_work);
            previous = usage;
        }
        if let Some(error) = failure {
            let stopped = inspection.usage();
            assert_eq!(inspection.feed(b"later"), Err(error));
            assert_eq!(inspection.usage(), stopped);
        }
        let completion = inspection.complete(input.len());
        let usage = inspection.usage();
        if let Err(error) = completion {
            assert_eq!(inspection.feed(b"later"), Err(error));
            assert_eq!(inspection.complete(0), Err(error));
            assert_eq!(inspection.finish(None), Err(error));
            assert!(tx.request.body.is_none());
            assert_eq!(tx.execute_request_body(), Err(error));
            return (Err(error), usage, 0, 0);
        }
        let result = inspection.finish(None);
        if let Err(error) = result {
            assert!(tx.request.body.is_none());
            assert_eq!(tx.execute_request_body(), Err(error));
            assert_eq!(tx.preflight_request_body(BodyProcessorSelection::Json), Err(error));
            return (Err(error), usage, 0, 0);
        }
        let budget = tx.json_budget.as_ref().unwrap().usage();
        (Ok(()), usage, budget.peak_retained_bytes, budget.work)
    };
    let (_, baseline, result_peak, result_work) = run(JsonInspectionLimits::default());
    assert!(baseline.lexical_peak_heap_bytes > 0 && result_peak > 0 && result_work > baseline.result_work);
    for (kind, exact) in [
        (JsonStreamLimit::SourceBytes, input.len()),
        (JsonStreamLimit::Work, baseline.lexical_work),
        (JsonStreamLimit::RawTokenBytes, long_value.len() + 2),
    ] {
        for (limit, succeeds) in [(exact, true), (exact - 1, false)] {
            let mut limits = JsonInspectionLimits::default();
            match kind {
                JsonStreamLimit::SourceBytes => limits.lexical.max_source_bytes = limit,
                JsonStreamLimit::Work => limits.lexical.max_work = limit,
                JsonStreamLimit::RawTokenBytes => limits.lexical.max_raw_token_bytes = limit,
                _ => unreachable!(),
            }
            let result = run(limits).0;
            if succeeds {
                assert_eq!(result, Ok(()));
            } else {
                assert_eq!(
                    result,
                    Err(RuntimeError::JsonChecker {
                        kind: JsonCheckerError::Lexical(JsonStreamError::Limit { kind, limit })
                    })
                );
            }
        }
    }
    for (limit, succeeds) in [(result_work, true), (result_work - 1, false)] {
        let mut limits = JsonInspectionLimits::default();
        limits.results.work = limit;
        let result = run(limits).0;
        if succeeds {
            assert_eq!(result, Ok(()));
        } else {
            assert_eq!(
                result,
                Err(RuntimeError::JsonResultLimit {
                    kind: JsonResultLimit::Work
                })
            );
        }
    }
    // Heap growth can clip geometric capacity to the configured limit. Find an
    // adjacent failed/successful admission pair instead of treating the default
    // allocator peak as a minimum required capacity.
    for lexical in [true, false] {
        let mut failed = 0;
        let mut passed = if lexical {
            baseline.lexical_peak_heap_bytes
        } else {
            result_peak
        };
        let limits_for = |limit| {
            let mut limits = JsonInspectionLimits::default();
            if lexical {
                limits.lexical.max_retained_bytes = limit;
            } else {
                limits.results.retained_bytes = limit;
            }
            limits
        };
        assert!(run(limits_for(failed)).0.is_err());
        assert_eq!(run(limits_for(passed)).0, Ok(()));
        while passed - failed > 1 {
            let middle = failed + (passed - failed) / 2;
            if run(limits_for(middle)).0.is_ok() {
                passed = middle;
            } else {
                failed = middle;
            }
        }
        let (result, usage, peak, _) = run(limits_for(passed));
        assert_eq!(result, Ok(()));
        assert!(if lexical {
            usage.lexical_peak_heap_bytes <= passed
        } else {
            peak <= passed
        });
        let expected = if lexical {
            RuntimeError::JsonChecker {
                kind: JsonCheckerError::Lexical(JsonStreamError::Limit {
                    kind: JsonStreamLimit::RetainedBytes,
                    limit: failed,
                }),
            }
        } else {
            RuntimeError::JsonResultLimit {
                kind: JsonResultLimit::RetainedBytes,
            }
        };
        assert_eq!(run(limits_for(failed)).0, Err(expected));
    }
}

#[test]
fn scoped_malformed_forced_raw_prelude_matches_owned_priority() {
    let generation =
        compile("SecAction \"id:1,phase:1,pass,ctl:forceRequestBodyVariable=On\"\nSecAction \"id:2,phase:2,pass\"\n");
    let input = Bytes::from_static(b"{] trailing");
    for (body, expected) in [
        (
            BodyLimits {
                max_entries: 2,
                max_decoded_bytes: input.len() + input.len().to_string().len(),
                ..BodyLimits::default()
            },
            BodyError::EntryLimit { limit: 2 },
        ),
        (
            BodyLimits {
                max_decoded_bytes: input.len() - 1,
                ..BodyLimits::default()
            },
            BodyError::DecodedByteLimit { limit: input.len() - 1 },
        ),
        (
            BodyLimits {
                max_work_bytes: input.len() - 1,
                max_entries: 0,
                max_decoded_bytes: 0,
                ..BodyLimits::default()
            },
            BodyError::WorkLimit { limit: input.len() - 1 },
        ),
    ] {
        for chunked in [false, true] {
            let mut tx = Transaction::new(
                &generation,
                head(b"/"),
                RuntimeLimits {
                    body,
                    ..RuntimeLimits::default()
                },
            )
            .unwrap();
            tx.execute_request_headers().unwrap();
            let result = if chunked {
                let mut inspection = tx.start_json_inspection(JsonInspectionLimits::default(), None).unwrap();
                for chunk in input.chunks(2) {
                    let _ = inspection.feed(chunk);
                }
                let error = inspection.complete(input.len()).unwrap_err();
                assert_eq!(inspection.complete(0), Err(error));
                assert_eq!(inspection.feed(b"later"), Err(error));
                inspection.finish(Some(input.clone()))
            } else {
                tx.publish_request_body_with_json_limits(
                    RequestBody::new(input.clone(), BodyProcessorSelection::Json),
                    JsonInspectionLimits::default(),
                )
            };
            assert_eq!(result, Err(RuntimeError::Body { kind: expected }));
            assert!(tx.request.body.is_none());
        }
    }
}

#[test]
fn fixed_summary_limit_priority_preserves_prelude_and_syntax_boundaries() {
    let generation = compile("SecAction \"id:1,phase:2,pass\"\n");
    let input = br#"{"a":[1]}"#;
    let cases = [
        (
            BodyLimits {
                max_name_bytes: 5,
                max_depth: 0,
                max_entries: 2,
                ..BodyLimits::default()
            },
            BodyError::NameByteLimit { limit: 5 },
        ),
        (
            BodyLimits {
                max_depth: 0,
                max_entries: 2,
                ..BodyLimits::default()
            },
            BodyError::DepthLimit { limit: 0 },
        ),
        (
            BodyLimits {
                max_entries: 2,
                max_decoded_bytes: 5,
                ..BodyLimits::default()
            },
            BodyError::EntryLimit { limit: 2 },
        ),
        (
            BodyLimits {
                max_decoded_bytes: 5,
                max_work_bytes: input.len(),
                ..BodyLimits::default()
            },
            BodyError::DecodedByteLimit { limit: 5 },
        ),
        (
            BodyLimits {
                max_work_bytes: input.len(),
                ..BodyLimits::default()
            },
            BodyError::WorkLimit { limit: input.len() },
        ),
    ];
    for (body, expected) in cases {
        for chunked in [false, true] {
            let mut tx = Transaction::new(
                &generation,
                head(b"/"),
                RuntimeLimits {
                    body,
                    ..RuntimeLimits::default()
                },
            )
            .unwrap();
            tx.execute_request_headers().unwrap();
            let result = if chunked {
                let mut inspection = tx.start_json_inspection(JsonInspectionLimits::default(), None).unwrap();
                for chunk in input.chunks(2) {
                    let _ = inspection.feed(chunk);
                }
                match inspection.complete(input.len()) {
                    Ok(()) => inspection.finish(None),
                    Err(error) => {
                        drop(inspection);
                        Err(error)
                    }
                }
            } else {
                tx.publish_request_body_with_json_limits(
                    RequestBody::new(Bytes::copy_from_slice(input), BodyProcessorSelection::Json),
                    JsonInspectionLimits::default(),
                )
            };
            assert_eq!(result, Err(RuntimeError::Body { kind: expected }));
        }
    }
    for (entries, detection, expected) in [
        (0, false, BodyError::EntryLimit { limit: 0 }),
        (
            2,
            false,
            BodyError::Malformed {
                processor: BodyProcessorSelection::Json,
            },
        ),
        (2, true, BodyError::EntryLimit { limit: 2 }),
    ] {
        for chunked in [false, true] {
            let mut tx = Transaction::new(
                &generation,
                head(b"/"),
                RuntimeLimits {
                    body: BodyLimits {
                        max_entries: entries,
                        max_name_bytes: 0,
                        max_depth: 0,
                        ..BodyLimits::default()
                    },
                    ..RuntimeLimits::default()
                },
            )
            .unwrap();
            if detection {
                tx.rule_engine_mode = RuleEngineMode::DetectionOnly;
            }
            tx.execute_request_headers().unwrap();
            let input = Bytes::from_static(br#"{"long":0} trailing"#);
            let result = if chunked {
                let mut inspection = tx.start_json_inspection(JsonInspectionLimits::default(), None).unwrap();
                for chunk in input.chunks(2) {
                    let _ = inspection.feed(chunk);
                }
                let result = inspection.complete(input.len());
                let source = inspection.requires_raw_source().then(|| input.clone());
                match result {
                    Ok(()) => inspection.finish(source),
                    Err(error) => {
                        drop(inspection);
                        Err(error)
                    }
                }
            } else {
                tx.publish_request_body_with_json_limits(
                    RequestBody::new(input, BodyProcessorSelection::Json),
                    JsonInspectionLimits::default(),
                )
            };
            assert_eq!(result, Err(RuntimeError::Body { kind: expected }));
        }
    }
}

#[test]
fn named_demands_are_phase_scoped_and_missing_differs_from_empty() {
    for later in [false, true] {
        let mut source = "SecAction \"id:1,phase:1,pass,setvar:'tx.early=%{ARGS.json.empty}'\"\n".to_string();
        if later {
            source.push_str("SecAction \"id:2,phase:2,pass,setvar:'tx.later=%{ARGS.json.empty}',setvar:'tx.absent=%{ARGS.json.absent}'\"\n");
        }
        let generation = compile(&source);
        let mut tx = Transaction::new(&generation, head(b"/"), RuntimeLimits::default()).unwrap();
        tx.execute_request_headers().unwrap();
        tx.publish_request_body_with_json_limits(
            RequestBody::new(Bytes::from_static(br#"{"empty":""}"#), BodyProcessorSelection::Json),
            JsonInspectionLimits::default(),
        )
        .unwrap();
        let results = tx
            .request
            .body
            .as_ref()
            .unwrap()
            .json()
            .unwrap()
            .results
            .as_ref()
            .unwrap();
        if later {
            let budget = tx.json_budget.as_mut().unwrap();
            assert_eq!(results.named_value(budget, 0).unwrap(), None);
            assert_eq!(results.named_value(budget, 1).unwrap(), Some(b"".as_slice()));
        } else {
            assert_eq!(tx.json_budget.as_ref().unwrap().usage().live_candidates, 0);
        }
        tx.execute_request_body().unwrap();
    }
}

#[test]
fn overwritten_semantic_errors_disappear_but_syntax_and_physical_errors_do_not() {
    compare(
        "SecRule ARGS \"@contains hit\" \"id:1,phase:2,pass\"\n",
        br#"{"a":"too-long","a":"hit"}"#,
        b"json.a=hit",
        b"/",
        RuntimeLimits {
            max_transformation_output_bytes: 3,
            ..RuntimeLimits::default()
        },
    );
    let generation = compile("SecRule ARGS \"@contains hit\" \"id:1,phase:2,pass\"\n");
    for (input, limits, syntax) in [
        (
            br#"{"a":"\q","a":"hit"}"#.as_slice(),
            JsonInspectionLimits::default(),
            true,
        ),
        (
            br#"{"a":"too-long","a":"hit"}"#.as_slice(),
            JsonInspectionLimits {
                lexical: JsonStreamLimits {
                    max_raw_token_bytes: 4,
                    ..JsonInspectionLimits::default().lexical
                },
                ..JsonInspectionLimits::default()
            },
            false,
        ),
    ] {
        let mut tx = Transaction::new(&generation, head(b"/"), RuntimeLimits::default()).unwrap();
        tx.execute_request_headers().unwrap();
        let error = tx
            .publish_request_body_with_json_limits(
                RequestBody::new(Bytes::copy_from_slice(input), BodyProcessorSelection::Json),
                limits,
            )
            .unwrap_err();
        assert_eq!(
            matches!(
                error,
                RuntimeError::Body {
                    kind: BodyError::Malformed { .. }
                }
            ),
            syntax
        );
        assert!(tx.request.body.is_none());
    }
}

#[test]
fn injection_capture_and_large_then_small_match_charges_match_form_runtime() {
    compare(
        "SecRule ARGS \"@detectSQLi\" \"id:1,phase:2,pass,capture,t:urlDecodeUni,setvar:'tx.fp=%{TX.0}'\"\n",
        br#"{"a":"%27%20or%201%3D1--"}"#,
        b"json.a=%2527%2520or%25201%253D1--",
        b"/",
        RuntimeLimits::default(),
    );
    for bytes in [0, 32, 64, 128, 256, 512] {
        compare(
            concat!(
                "SecRule ARGS \"!@pm absent\" \"id:1,phase:2,pass\"\n",
                "SecRule ARGS \"@pm absent\" \"id:2,phase:2,pass\"\n"
            ),
            br#"{"a":"a-long-matched-value","b":"x"}"#,
            b"json.a=a-long-matched-value&json.b=x",
            b"/",
            RuntimeLimits {
                max_tx_bytes: bytes,
                ..RuntimeLimits::default()
            },
        );
    }
}

#[test]
fn matching_and_tx_limits_precede_action_metadata_expansion() {
    let mut generation = compile("SecRule ARGS \"@pm hit\" \"id:1,phase:2,pass,msg:'long-message'\"\n");
    generation.members[0].runtime_expansion_byte_limit = 3;
    for max_tx_variables in [0, 8] {
        let limits = RuntimeLimits {
            max_tx_variables,
            ..RuntimeLimits::default()
        };
        for matching in [false, true] {
            let mut fused = Transaction::new(&generation, head(b"/"), limits).unwrap();
            let mut ordinary = Transaction::new(&generation, head(b"/"), limits).unwrap();
            fused.execute_request_headers().unwrap();
            ordinary.execute_request_headers().unwrap();
            let (json, form) = if matching {
                (br#"{"a":"hit"}"#.as_slice(), b"json.a=hit".as_slice())
            } else {
                (br#"{"a":"miss"}"#.as_slice(), b"json.a=miss".as_slice())
            };
            fused
                .publish_request_body_with_json_limits(
                    RequestBody::new(Bytes::copy_from_slice(json), BodyProcessorSelection::Json),
                    JsonInspectionLimits::default(),
                )
                .unwrap();
            ordinary
                .publish_request_body(RequestBody::new(
                    bytes::Bytes::copy_from_slice(form),
                    BodyProcessorSelection::UrlEncoded,
                ))
                .unwrap();
            let actual = fused.execute_request_body().err();
            assert_eq!(actual, ordinary.execute_request_body().err());
            if !matching {
                assert_eq!(actual, None);
            } else if max_tx_variables == 0 {
                assert_eq!(actual, Some(RuntimeError::TxVariableLimit { limit: 0 }));
            } else {
                assert_eq!(actual, Some(RuntimeError::ValueByteLimit { actual: 12, limit: 3 }));
            }
        }
    }
}

#[test]
fn configured_depth_above_127_is_supported_and_each_depth_cap_is_independent() {
    let generation = compile("SecAction \"id:1,phase:2,pass\"\n");
    let mut input = vec![b'['; 130];
    input.push(b'0');
    input.extend(std::iter::repeat_n(b']', 130));
    for lowered in 0..4 {
        let runtime = RuntimeLimits {
            body: BodyLimits {
                max_depth: if lowered == 1 { 129 } else { 256 },
                ..BodyLimits::default()
            },
            ..RuntimeLimits::default()
        };
        let limits = JsonInspectionLimits {
            lexical: JsonStreamLimits {
                max_depth: if lowered == 2 { 129 } else { 256 },
                ..JsonInspectionLimits::default().lexical
            },
            structure: JsonStructureLimits {
                max_depth: if lowered == 3 { 129 } else { 256 },
                ..JsonStructureLimits::default()
            },
            ..JsonInspectionLimits::default()
        };
        let mut tx = Transaction::new(&generation, head(b"/"), runtime).unwrap();
        tx.execute_request_headers().unwrap();
        assert_eq!(
            tx.publish_request_body_with_json_limits(
                RequestBody::new(Bytes::copy_from_slice(&input), BodyProcessorSelection::Json),
                limits
            )
            .is_ok(),
            lowered == 0
        );
    }
}

#[test]
fn ctl_target_exclusions_apply_to_json_facts_and_counts() {
    compare(
        concat!(
            "SecAction \"id:1,phase:1,pass,ctl:ruleRemoveTargetByTag=check;ARGS:/^json[.]a$/\"\n",
            "SecRule ARGS \"@pm hit\" \"id:2,phase:2,pass,tag:check\"\n",
            "SecRule &ARGS \"@eq 1\" \"id:3,phase:2,pass,tag:check\"\n",
        ),
        br#"{"a":"hit","b":"miss"}"#,
        b"json.a=hit&json.b=miss",
        b"/",
        RuntimeLimits::default(),
    );
}

#[test]
fn preflight_honors_phase_one_override_without_publishing() {
    let generation = compile(concat!(
        "SecAction \"id:1,phase:1,pass,ctl:requestBodyProcessor=JSON,ctl:forceRequestBodyVariable=On\"\n",
        "SecRule ARGS \"@contains hit\" \"id:2,phase:2,pass\"\n",
    ));
    let mut tx = Transaction::new(&generation, head(b"/"), RuntimeLimits::default()).unwrap();
    tx.execute_request_headers().unwrap();
    for requested in [BodyProcessorSelection::Raw, BodyProcessorSelection::Auto] {
        assert_eq!(tx.preflight_request_body(requested), Ok(BodyProcessorSelection::Json));
        assert!(tx.request.body.is_none());
    }
    tx.publish_request_body_with_json_limits(
        RequestBody::new(Bytes::from_static(br#"{"a":"hit"}"#), BodyProcessorSelection::Raw),
        JsonInspectionLimits::default(),
    )
    .unwrap();
    assert!(tx.request.body.as_ref().unwrap().json().unwrap().source.is_some());
    assert_eq!(tx.execute_request_body().unwrap().findings()[0].rule_id(), 2);
}

#[test]
fn literal_json_accounting_preserves_raw_and_head_overflow_boundaries() {
    let generation = compile("");
    for raw in [false, true] {
        let make = |body_limits| {
            let mut tx = Transaction::new(
                &generation,
                head(b"/?x=1234567890123456"),
                RuntimeLimits {
                    body: body_limits,
                    ..RuntimeLimits::default()
                },
            )
            .unwrap();
            tx.execute_request_headers().unwrap();
            tx.tx.force_request_body_variable = Some(if raw {
                crate::seclang::OnOff::On
            } else {
                crate::seclang::OnOff::Off
            });
            tx.publish_request_body(RequestBody::new(
                Bytes::from_static(br#"{"a":1}"#),
                BodyProcessorSelection::Json,
            ))
            .map(|()| tx)
        };
        let entries = if raw { 6 } else { 5 };
        let decoded = if raw { 33 } else { 26 };
        let caps = crate::seclang::BodyLimits {
            max_entries: entries,
            max_decoded_bytes: decoded,
            max_work_bytes: 17,
            ..crate::seclang::BodyLimits::default()
        };
        let tx = make(caps).unwrap();
        let summary = tx.request.body.as_ref().unwrap().json().unwrap().summary;
        assert_eq!(
            (
                summary.entries,
                summary.decoded_bytes,
                summary.work_bytes,
                summary.args_combined_size
            ),
            (entries, decoded, 17, 24)
        );
        for cap in [
            crate::seclang::BodyLimits {
                max_entries: entries - 1,
                ..caps
            },
            crate::seclang::BodyLimits {
                max_decoded_bytes: decoded - 1,
                ..caps
            },
            crate::seclang::BodyLimits {
                max_work_bytes: 16,
                ..caps
            },
        ] {
            assert!(make(cap).is_err());
        }
        let mut accounting = BodyAccounting::json_prelude(7, crate::seclang::BodyLimits::default(), raw).unwrap();
        accounting
            .apply_json_summary(JsonSemanticSummary {
                argument_count: 1,
                argument_name_bytes: 6,
                argument_value_bytes: 1,
                body_entry_count: 2,
                decoded_bytes: 19,
                combined_size: 7,
                traversal_work: 10,
                max_checked_name_bytes: 6,
                max_value_depth: 1,
            })
            .unwrap();
        assert_eq!(accounting.finish_json(usize::MAX), Err(BodyError::Arithmetic));
    }
}

#[test]
fn empty_matching_scalar_has_actions_and_descriptors_but_absent_candidate_does_not() {
    let generation = compile("SecRule ARGS \"@rx ^$\" \"id:1,phase:2,pass,setvar:tx.empty=+1\"\n");
    for (input, expected) in [(br#"{"a":null}"#.as_slice(), true), (b"{}".as_slice(), false)] {
        let mut tx = Transaction::new(&generation, head(b"/"), RuntimeLimits::default()).unwrap();
        tx.execute_request_headers().unwrap();
        tx.publish_request_body(RequestBody::new(
            Bytes::copy_from_slice(input),
            BodyProcessorSelection::Json,
        ))
        .unwrap();
        assert_eq!(!tx.execute_request_body().unwrap().findings().is_empty(), expected);
        assert_eq!(tx.tx_value(b"empty"), expected.then_some(b"1".as_slice()));
        assert_eq!(tx.tx.matched_vars.len(), usize::from(expected));
        if expected {
            assert!(tx.tx.retained_bytes >= size_of::<TxEntry>());
        }
    }
}

#[test]
fn scheduler_modes_findings_skips_and_deferred_failures_match_full_form_outcomes() {
    for action in ["pass", "deny,status:418", "pass,skip:1", "pass,skipAfter:AFTER"] {
        let source = format!("SecRule ARGS:json.a \"@streq hit\" \"id:1,phase:2,{action}\"\nSecRule ARGS:json.z \"@streq miss\" \"id:2,phase:2,pass\"\nSecMarker AFTER\n");
        let generation = compile(&source);
        for mode in [RuleEngineMode::On, RuleEngineMode::DetectionOnly] {
            for max_findings in [0, 1, 8] {
                let limits = RuntimeLimits {
                    max_findings,
                    max_transformation_output_bytes: 8,
                    ..RuntimeLimits::default()
                };
                let run = |processor, data| {
                    let mut tx =
                        Transaction::new_with_modes(&generation, head(b"/"), limits, EvaluationMode::Accelerated, mode)
                            .unwrap();
                    tx.execute_request_headers().unwrap();
                    tx.publish_request_body(RequestBody::new(Bytes::from_static(data), processor))
                        .unwrap();
                    let result = tx.execute_request_body();
                    (result, tx.next_phase())
                };
                assert_eq!(
                    run(BodyProcessorSelection::Json, br#"{"z":"abcdefghijklmnop","a":"hit"}"#),
                    run(
                        BodyProcessorSelection::UrlEncoded,
                        b"json.a=hit&json.z=abcdefghijklmnop"
                    )
                );
            }
        }
    }
}

#[test]
fn semantic_retry_repeats_prior_committed_actions() {
    let generation = compile(
        "SecAction \"id:1,phase:2,pass,setvar:tx.attempt=+1\"\nSecRule ARGS \"@streq hit\" \"id:2,phase:2,pass\"\n",
    );
    let mut tx = Transaction::new(
        &generation,
        head(b"/"),
        RuntimeLimits {
            max_selected_values: 0,
            ..RuntimeLimits::default()
        },
    )
    .unwrap();
    tx.execute_request_headers().unwrap();
    tx.publish_request_body(RequestBody::new(
        Bytes::from_static(br#"{"a":"hit"}"#),
        BodyProcessorSelection::Json,
    ))
    .unwrap();
    for count in [b"1", b"2", b"3"] {
        assert!(matches!(
            tx.execute_request_body(),
            Err(RuntimeError::SelectedValueLimit { .. })
        ));
        assert_eq!(tx.tx_value(b"attempt"), Some(count.as_slice()));
        assert_eq!(tx.next_phase(), Some(Phase::RequestBody));
    }
}

#[test]
fn shared_generations_are_pinned_isolated_and_released_after_drop() {
    let generation = Arc::new(compile(
        "SecRule ARGS \"@streq hit\" \"id:1,phase:2,pass,setvar:tx.hit=1\"",
    ));
    let baseline = Arc::strong_count(&generation);
    let mut hit = Transaction::new_shared(generation.clone(), head(b"/"), RuntimeLimits::default()).unwrap();
    let mut miss = Transaction::new_shared(generation.clone(), head(b"/"), RuntimeLimits::default()).unwrap();
    for (tx, input) in [
        (&mut hit, br#"{"a":"hit"}"#.as_slice()),
        (&mut miss, br#"{"a":"miss"}"#.as_slice()),
    ] {
        tx.execute_request_headers().unwrap();
        tx.publish_request_body(RequestBody::new(
            Bytes::copy_from_slice(input),
            BodyProcessorSelection::Json,
        ))
        .unwrap();
    }
    let newer = Arc::new(compile("SecRule ARGS \"@streq miss\" \"id:2,phase:2,pass\""));
    assert_eq!(hit.execute_request_body().unwrap().findings()[0].rule_id(), 1);
    assert!(miss.execute_request_body().unwrap().findings().is_empty());
    assert_eq!(hit.tx_value(b"hit"), Some(b"1".as_slice()));
    assert_eq!(miss.tx_value(b"hit"), None);
    assert!(!Arc::ptr_eq(&generation, &newer));
    drop(hit);
    drop(miss);
    assert_eq!(Arc::strong_count(&generation), baseline);
}

#[test]
fn later_phases_keep_named_json_and_captures_but_clear_match_observers() {
    let generation = compile(concat!(
        "SecRule ARGS:json.a \"@rx ^(hit)$\" \"id:1,phase:2,pass,capture\"\n",
        "SecAction \"id:2,phase:3,pass,setvar:'tx.name=%{ARGS.json.a}',setvar:'tx.capture=%{TX.1}',setvar:'tx.old=%{MATCHED_VAR}'\"\n",
        "SecRule REQUEST_METHOD \"@streq POST\" \"id:3,phase:4,pass,setvar:'tx.new=%{MATCHED_VAR}'\"\n",
        "SecAction \"id:4,phase:5,pass,setvar:'tx.final=%{ARGS.json.a}',setvar:'tx.cleared=%{MATCHED_VAR}'\"\n",
    ));
    let mut tx = Transaction::new(&generation, head(b"/"), RuntimeLimits::default()).unwrap();
    tx.execute_request_headers().unwrap();
    tx.publish_request_body(RequestBody::new(
        Bytes::from_static(br#"{"a":"hit"}"#),
        BodyProcessorSelection::Json,
    ))
    .unwrap();
    tx.execute_request_body().unwrap();
    let retained = tx.retained_bytes();
    let response = ResponseHead::new(200, &[]);
    tx.limits.max_retained_bytes = retained;
    assert!(matches!(
        tx.publish_response_head(response),
        Err(RuntimeError::RetainedByteLimit { .. })
    ));
    tx.limits.max_retained_bytes = RuntimeLimits::default().max_retained_bytes;
    tx.publish_response_head(ResponseHead::new(200, &[])).unwrap();
    tx.execute_response_headers().unwrap();
    assert_eq!(tx.tx_value(b"name"), Some(b"hit".as_slice()));
    assert_eq!(tx.tx_value(b"capture"), Some(b"hit".as_slice()));
    assert_eq!(tx.tx_value(b"old"), Some(b"".as_slice()));
    tx.execute_response_window(ResponseWindow::new(b"response", true))
        .unwrap();
    assert_eq!(tx.tx_value(b"new"), Some(b"POST".as_slice()));
    tx.execute_logging().unwrap();
    assert_eq!(tx.tx_value(b"final"), Some(b"hit".as_slice()));
    assert_eq!(tx.tx_value(b"cleared"), Some(b"".as_slice()));
    assert_eq!(tx.tx_value(b"1"), Some(b"hit".as_slice()));
}

#[test]
fn seeded_tx_peak_accepts_exact_threshold_and_rejects_one_less() {
    let source =
        "SecAction \"id:1,phase:1,pass,setvar:tx.seed=seeded\"\nSecRule ARGS \"@contains hit\" \"id:2,phase:2,pass\"\n";
    let generation = compile(source);
    let run = |processor, input, cap| {
        let mut tx = Transaction::new(
            &generation,
            head(b"/"),
            RuntimeLimits {
                max_tx_bytes: cap,
                ..RuntimeLimits::default()
            },
        )
        .unwrap();
        tx.execute_request_headers()?;
        tx.publish_request_body(RequestBody::new(Bytes::from_static(input), processor))?;
        tx.execute_request_body()
    };
    let json = br#"{"a":"hit-long-long-long","b":"hit","c":"hit-medium"}"#;
    let form = b"json.a=hit-long-long-long&json.b=hit&json.c=hit-medium";
    let threshold = (0..4096)
        .find(|cap| run(BodyProcessorSelection::UrlEncoded, form, *cap).is_ok())
        .unwrap();
    assert!(threshold > 0);
    for cap in [threshold - 1, threshold, threshold + 1] {
        assert_eq!(
            run(BodyProcessorSelection::Json, json, cap),
            run(BodyProcessorSelection::UrlEncoded, form, cap)
        );
    }
}

#[test]
fn json_inspection_measurements() {
    use crate::seclang::measurements::{self, Event as E};
    struct Fixture {
        name: &'static str,
        input: Bytes,
        rules: String,
        fields: usize,
        no_payload: bool,
        depth: usize,
        deferred_bytes: usize,
    }
    let no_hits = format!(
        "{{{}}}",
        (0..10_000)
            .rev()
            .map(|i| format!("\"k{i:05}\":\"{}\"", "x".repeat(16)))
            .collect::<Vec<_>>()
            .join(",")
    );
    let static_rules = |count| {
        (0..count)
            .map(|id| {
                format!(
                    "SecRule ARGS \"@pm missing{id}\" \"id:{},phase:2,pass,t:lowercase\"\n",
                    id + 1
                )
            })
            .collect::<String>()
    };
    let no_hit_rule = static_rules(1);
    let mut fixtures = vec![
        Fixture {
            name: "wide_no_hits_1",
            input: Bytes::copy_from_slice(no_hits.as_bytes()),
            rules: static_rules(1),
            fields: 10_000,
            no_payload: true,
            depth: 64,
            deferred_bytes: 0,
        },
        Fixture {
            name: "wide_no_hits_32",
            input: Bytes::from(no_hits),
            rules: static_rules(32),
            fields: 10_000,
            no_payload: true,
            depth: 64,
            deferred_bytes: 0,
        },
        Fixture {
            name: "many_capture_hits",
            input: Bytes::from(format!(
                "{{{}}}",
                (0..128)
                    .map(|i| format!("\"k{i:04}\":\"hit\""))
                    .collect::<Vec<_>>()
                    .join(",")
            )),
            rules: "SecRule ARGS \"@rx (hit)\" \"id:1,phase:2,pass,capture\"\n".into(),
            fields: 128,
            no_payload: false,
            depth: 64,
            deferred_bytes: 0,
        },
        Fixture {
            name: "plain_long",
            input: Bytes::from(format!("{{\"a\":\"{}\"}}", "x".repeat(64 << 10))),
            rules: no_hit_rule.clone(),
            fields: 1,
            no_payload: true,
            depth: 64,
            deferred_bytes: 0,
        },
        Fixture {
            name: "escaped_long",
            input: Bytes::from(format!("{{\"a\":\"{}\"}}", "\\u0078".repeat(64 << 10))),
            rules: no_hit_rule.clone(),
            fields: 1,
            no_payload: true,
            depth: 64,
            deferred_bytes: 0,
        },
        Fixture {
            name: "duplicate_last_miss",
            input: Bytes::from(format!(
                "{{{},\"same\":\"miss\"}}",
                (0..256).map(|_| "\"same\":\"hit\"").collect::<Vec<_>>().join(",")
            )),
            rules: "SecRule ARGS \"@pm hit\" \"id:1,phase:2,pass\"".into(),
            fields: 1,
            no_payload: true,
            depth: 64,
            deferred_bytes: 0,
        },
        Fixture {
            name: "deep_arrays",
            input: Bytes::from(format!("{}null{}", "[".repeat(130), "]".repeat(130))),
            rules: "SecRule &ARGS \"@eq 131\" \"id:1,phase:2,pass\"".into(),
            fields: 131,
            no_payload: true,
            depth: 160,
            deferred_bytes: 0,
        },
        Fixture {
            name: "one_named_dependency",
            input: Bytes::from(format!(
                "{{{},\"wanted\":\"hit\"}}",
                (0..1024)
                    .map(|i| format!("\"k{i:04}\":\"{}\"", "x".repeat(128)))
                    .collect::<Vec<_>>()
                    .join(",")
            )),
            rules: "SecAction \"id:1,phase:2,pass,setvar:'tx.wanted=%{ARGS.json.wanted}'\"".into(),
            fields: 1025,
            no_payload: false,
            depth: 64,
            deferred_bytes: 3,
        },
    ];
    let columns = [
        ("grammar_instances", E::GrammarInstances),
        ("source_bytes_consumed", E::SourceBytes),
        ("completions", E::Completions),
        ("token_explicit_copy_bytes", E::TokenCopyBytes),
        ("key_explicit_copy_bytes", E::KeyCopyBytes),
        ("path_explicit_write_bytes", E::PathCopyBytes),
        ("name_explicit_copy_bytes", E::NameCopyBytes),
        ("deferred_explicit_copy_bytes", E::DeferredCopyBytes),
        ("matched_explicit_copy_bytes", E::MatchedCopyBytes),
        ("capture_explicit_copy_bytes", E::CaptureCopyBytes),
        ("temporary_capture_explicit_copy_bytes", E::TemporaryCaptureCopyBytes),
        ("candidates_created", E::Candidates),
        ("groups_dispatched", E::DispatchGroups),
        ("profile_walks", E::ProfileWalks),
        ("transform_steps_completed", E::TransformSteps),
        ("operator_stages", E::OperatorStages),
        ("full_matcher_scans", E::FullScans),
        ("range_queries", E::RangeQueries),
        ("failed_range_queries", E::RangeQueryFailures),
        ("dispatch_work_units", E::DispatchWork),
        ("transform_work_units", E::TransformWork),
        ("operator_work_units", E::OperatorWork),
        ("detector_work_units", E::DetectorWork),
        ("lexical_work_units", E::LexicalWork),
        ("structure_work_units", E::StructureWork),
        ("lexical_peak_capacity_bytes", E::LexicalPeakBytes),
        ("structure_peak_capacity_bytes", E::StructurePeakBytes),
    ];
    println!("JSON_MECHANISMS,schema_version,case,source_bytes,status,{},result_retained_capacity_bytes,result_peak_capacity_bytes,result_work_units,live_candidates,live_name_bytes,live_deferred_bytes,live_matched_bytes,live_capture_bytes", columns.iter().map(|(name, _)| *name).collect::<Vec<_>>().join(","));
    let mut no_hit_retained = None;
    for fixture in fixtures.drain(..) {
        let generation = compile(&fixture.rules);
        let mut limits = RuntimeLimits::default();
        limits.body.max_depth = fixture.depth;
        let mut tx = Transaction::new(&generation, head(b"/"), limits).unwrap();
        tx.execute_request_headers().unwrap();
        let mut inspection = JsonInspectionLimits::default();
        inspection.lexical.max_depth = fixture.depth;
        inspection.structure.max_depth = fixture.depth;
        let source_bytes = fixture.input.len();
        let scope = measurements::start();
        tx.publish_request_body_with_json_limits(
            RequestBody::new(fixture.input, BodyProcessorSelection::Json),
            inspection,
        )
        .unwrap();
        assert_eq!(
            tx.request.body.as_ref().unwrap().json().unwrap().argument_count(),
            fixture.fields
        );
        tx.execute_request_body().unwrap();
        let counts = scope.counts();
        drop(scope);
        let usage = tx.json_budget.as_ref().unwrap().usage();
        assert_eq!(counts.get(E::GrammarInstances), 1, "{}", fixture.name);
        assert_eq!(counts.get(E::SourceBytes), source_bytes);
        assert_eq!(counts.get(E::Completions), 1);
        assert_eq!(counts.get(E::RangeQueryFailures), 0);
        assert_eq!(usage.deferred_bytes, fixture.deferred_bytes);
        assert_eq!(counts.get(E::DeferredCopyBytes), fixture.deferred_bytes);
        if fixture.no_payload {
            assert_eq!(usage.matched_bytes + usage.capture_bytes + usage.deferred_bytes, 0);
        }
        match fixture.name {
            "wide_no_hits_1" | "wide_no_hits_32" => {
                let subscribers = if fixture.name.ends_with("_32") { 32 } else { 1 };
                assert_eq!(counts.get(E::Candidates), 10_000);
                assert_eq!(counts.get(E::ProfileWalks), 10_000);
                assert_eq!(counts.get(E::TransformSteps), 10_000);
                assert_eq!(counts.get(E::FullScans), 10_000);
                assert_eq!(counts.get(E::DispatchGroups), 10_000 * subscribers);
                assert_eq!(counts.get(E::RangeQueries), 10_000 * subscribers);
                if subscribers == 1 {
                    no_hit_retained = Some(usage.retained_bytes);
                } else {
                    assert_eq!(Some(usage.retained_bytes), no_hit_retained);
                }
                assert_eq!(
                    counts.get(E::MatchedCopyBytes) + counts.get(E::CaptureCopyBytes) + counts.get(E::TokenCopyBytes),
                    0
                );
            }
            "plain_long" => assert_eq!(counts.get(E::TokenCopyBytes), 0),
            "escaped_long" => assert_eq!(counts.get(E::TokenCopyBytes), 64 << 10),
            "duplicate_last_miss" => {
                assert_eq!(usage.live_candidates, 1);
                assert_eq!(counts.get(E::MatchedCopyBytes), 256 * 3);
            }
            "many_capture_hits" => {
                assert_eq!(usage.matched_bytes, 128 * 3);
                assert_eq!(usage.capture_bytes, 128 * 8);
                assert_eq!(counts.get(E::TemporaryCaptureCopyBytes), 0);
                assert_eq!(counts.get(E::CaptureCopyBytes), 128 * 2);
            }
            "deep_arrays" => assert_eq!(usage.live_candidates, 0),
            "one_named_dependency" => {
                assert_eq!(usage.live_candidates, 1);
                assert_eq!(tx.tx_value(b"wanted"), Some(b"hit".as_slice()));
            }
            _ => unreachable!(),
        }
        assert!(usage.peak_retained_bytes <= inspection.results.retained_bytes);
        println!(
            "JSON_MECHANISMS,1,{},{},complete,{},{},{},{},{},{},{},{},{}",
            fixture.name,
            source_bytes,
            columns
                .iter()
                .map(|(_, event)| counts.get(*event).to_string())
                .collect::<Vec<_>>()
                .join(","),
            usage.retained_bytes,
            usage.peak_retained_bytes,
            usage.work,
            usage.live_candidates,
            usage.name_bytes,
            usage.deferred_bytes,
            usage.matched_bytes,
            usage.capture_bytes
        );
    }
    let unsupported = compile("SecRule ARGS \"@streq %{TX.expected}\" \"id:77,phase:2,pass\"");
    assert_eq!(unsupported.json_inspection_unsupported()[0].rule_id, 77);
    let mut tx = Transaction::new(&unsupported, head(b"/"), RuntimeLimits::default()).unwrap();
    tx.execute_request_headers().unwrap();
    let scope = measurements::start();
    assert_eq!(
        tx.publish_request_body(RequestBody::new(
            Bytes::from_static(b"{}"),
            BodyProcessorSelection::Json
        )),
        Err(RuntimeError::JsonUnsupported { rule_id: 77 })
    );
    assert_eq!(scope.counts().get(E::GrammarInstances), 0);
    assert_eq!(scope.counts().get(E::SourceBytes), 0);
    println!(
        "JSON_MECHANISMS,1,broad_dynamic_rejected,2,unsupported_rule_77,{},0,0,0,0,0,0,0,0",
        columns
            .iter()
            .map(|(_, event)| scope.counts().get(*event).to_string())
            .collect::<Vec<_>>()
            .join(",")
    );
}

#[test]
fn sealed_names_and_capture_values_share_through_match_tx_and_branch_clones() {
    let generation = compile(concat!(
        "SecRule ARGS \"@rx ^(.*)$\" \"id:1,phase:2,pass,capture\"\n",
        "SecRule ARGS \"@rx ^(.*)$\" \"id:2,phase:2,pass,capture\"\n",
    ));
    for length in [0, 16_385] {
        let mut input = b"{\"long_name\":\"".to_vec();
        input.extend(std::iter::repeat_n(b'x', length));
        input.extend_from_slice(b"\"}");
        let mut tx = Transaction::new(&generation, head(b"/"), RuntimeLimits::default()).unwrap();
        tx.execute_request_headers().unwrap();
        tx.publish_request_body(RequestBody::new(Bytes::from(input), BodyProcessorSelection::Json))
            .unwrap();
        let mut first = tx.evaluate_member_matches(0, 0, &TxState::default(), true).unwrap();
        let second = tx.evaluate_member_matches(1, 1, &TxState::default(), true).unwrap();
        assert_eq!(first[0].matched_var_name, second[0].matched_var_name);
        assert!(matches!(first[0].matched_var_name, Some(MatchValue::Json(_))));
        let name_pointer = first[0]
            .matched_var_name
            .as_ref()
            .unwrap()
            .bytes(&tx.request, tx.json_budget.as_mut())
            .unwrap()
            .as_ptr();
        let captured = &first[0].captures[1].value;
        assert!(matches!(captured, MatchValue::Json(_)));
        let capture_pointer = captured.bytes(&tx.request, tx.json_budget.as_mut()).unwrap().as_ptr();
        assert_eq!(
            capture_pointer,
            first[0]
                .matched_var
                .as_ref()
                .unwrap()
                .bytes(&tx.request, tx.json_budget.as_mut())
                .unwrap()
                .as_ptr()
        );
        assert_eq!(
            capture_pointer,
            first[0].captures[0]
                .value
                .bytes(&tx.request, tx.json_budget.as_mut())
                .unwrap()
                .as_ptr()
        );
        assert_eq!(
            capture_pointer,
            second[0].captures[1]
                .value
                .bytes(&tx.request, tx.json_budget.as_mut())
                .unwrap()
                .as_ptr()
        );
        let mut staged = TxState::default();
        let mut keys = Vec::new();
        apply_member_match(
            &mut staged,
            &mut keys,
            first.pop().unwrap(),
            tx.limits,
            &tx.request,
            tx.json_budget.as_mut(),
        )
        .unwrap();
        let branch = staged.try_clone().unwrap();
        let unit = staged.try_clone_for_unit().unwrap();
        for state in [&staged, &branch, &unit] {
            let value = state.get(b"1", &tx.request, tx.json_budget.as_mut()).unwrap().unwrap();
            assert_eq!(value.len(), length);
            assert_eq!(
                value.bytes(&tx.request, tx.json_budget.as_mut()).unwrap().as_ptr(),
                capture_pointer
            );
            assert!(state.retained_bytes >= 2 * length);
            assert_eq!(
                SelectedValue {
                    collection: CollectionKind::Tx,
                    name: Some(SelectedName::Tx(1)),
                    value: SelectedBytes::Tx(1)
                }
                .bytes(&tx.request, state, None, tx.json_budget.as_mut())
                .unwrap()
                .as_ptr(),
                capture_pointer
            );
        }
        for state in [&staged, &branch] {
            let name = state.matched_var_name.as_ref().unwrap();
            assert_eq!(
                name.bytes(&tx.request, tx.json_budget.as_mut()).unwrap(),
                b"ARGS:json.long_name"
            );
            assert_eq!(
                name.bytes(&tx.request, tx.json_budget.as_mut()).unwrap().as_ptr(),
                name_pointer
            );
            assert_eq!(
                state.matched_vars[0]
                    .key
                    .bytes(&tx.request, tx.json_budget.as_mut())
                    .unwrap()
                    .as_ptr(),
                name_pointer
            );
        }
        tx.tx = staged;
        let work = tx.json_budget.as_ref().unwrap().usage().work;
        assert_eq!(tx.tx_value(b"1").unwrap().as_ptr(), capture_pointer);
        assert_eq!(tx.json_budget.as_ref().unwrap().usage().work, work);
        tx.json_budget.as_mut().unwrap().work(usize::MAX).unwrap_err();
        // Public inspection remains a checked immutable observation even after
        // rule execution is terminal; execution cannot bypass the sticky error.
        assert_eq!(tx.tx_value(b"1").unwrap().as_ptr(), capture_pointer);
        assert!(tx.tx.get(b"1", &tx.request, tx.json_budget.as_mut()).is_err());
    }
}

#[test]
fn shared_capture_numeric_updates_overwrites_delete_and_recapture_match_owned_runtime() {
    compare(
        concat!(
            "SecRule ARGS:json.a \"@rx ^(12)$\" \"id:1,phase:2,pass,capture,chain\"\n",
            "SecRule TX:1 \"@streq 12\" \"chain,setvar:'tx.1=+3',setvar:'tx.old=%{MATCHED_VAR_NAME}'\"\n",
            "SecRule TX:1 \"@eq 15\" \"setvar:'tx.numeric=%{TX.1}',setvar:'tx.1=owned',setvar:!tx.0\"\n",
            "SecRule &TX:0 \"@eq 0\" \"id:2,phase:2,pass\"\n",
            "SecRule TX:1 \"@streq owned\" \"id:3,phase:2,pass\"\n",
            "SecRule ARGS:json.b \"@rx ^(34)$\" \"id:4,phase:2,pass,capture,setvar:'tx.new=%{TX.1}'\"\n",
            "SecRule TX:1 \"@streq 34\" \"id:5,phase:3,pass\"\n",
        ),
        br#"{"a":"12","b":"34"}"#,
        b"json.a=12&json.b=34",
        b"/",
        RuntimeLimits::default(),
    );
}

#[test]
fn sealed_capture_keys_share_through_tx_cleanup_and_clones_with_checked_reads() {
    let generation = compile("SecRule ARGS \"@rx ^(?P<Name>a)$\" \"id:1,phase:2,pass,capture,t:none\"\n");
    let mut tx = no_hit_transaction(
        &generation,
        br#"{"x":"a"}"#,
        b"/",
        RuntimeLimits::default(),
        ResultLimits::default(),
        false,
    );
    let mut matches = tx.evaluate_member_matches(0, 0, &TxState::default(), true).unwrap();
    let pointers = matches[0]
        .captures
        .iter()
        .map(|capture| {
            assert!(matches!(capture.key, MatchValue::Json(_)));
            capture.key.observed_bytes(&tx.request).unwrap().as_ptr()
        })
        .collect::<Vec<_>>();
    let mut staged = TxState::default();
    let mut keys = Vec::new();
    apply_member_match(
        &mut staged,
        &mut keys,
        matches.pop().unwrap(),
        tx.limits,
        &tx.request,
        tx.json_budget.as_mut(),
    )
    .unwrap();
    let full = staged.try_clone().unwrap();
    let unit = staged.try_clone_for_unit().unwrap();
    let cloned_keys = try_clone_keys(&keys).unwrap();
    for state in [&staged, &full, &unit] {
        for (index, entry) in state.entries.iter().enumerate() {
            assert!(matches!(entry.key, MatchValue::Json(_)));
            assert_eq!(
                entry.key.bytes(&tx.request, tx.json_budget.as_mut()).unwrap().as_ptr(),
                pointers[index]
            );
        }
    }
    for list in [&keys, &cloned_keys] {
        for (index, key) in list.iter().enumerate() {
            assert_eq!(
                key.bytes(&tx.request, tx.json_budget.as_mut()).unwrap().as_ptr(),
                pointers[index]
            );
        }
    }
    assert!(staged.get(b"name", &tx.request, None).is_err());
    let other = Transaction::new(&generation, head(b"/"), tx.limits).unwrap();
    assert!(staged.observed_get(b"Name", &other.request).is_err());
    assert!(staged.delete(b"Name", &other.request, None).is_err());
    assert_eq!(staged.entries.len(), 3);
    staged
        .set(
            b"NAME".to_vec(),
            b"owned".to_vec(),
            tx.limits,
            &tx.request,
            tx.json_budget.as_mut(),
        )
        .unwrap();
    assert_eq!(staged.entries[2].key.observed_bytes(&tx.request).unwrap(), b"Name");
    assert_eq!(
        staged.entries[2].key.observed_bytes(&tx.request).unwrap().as_ptr(),
        pointers[2]
    );
    staged
        .set(
            b"01".to_vec(),
            b"distinct".to_vec(),
            tx.limits,
            &tx.request,
            tx.json_budget.as_mut(),
        )
        .unwrap();
    assert_eq!(
        staged
            .get(b"1", &tx.request, tx.json_budget.as_mut())
            .unwrap()
            .unwrap()
            .observed_bytes(&tx.request)
            .unwrap(),
        b"a"
    );
    assert_eq!(
        staged
            .get(b"01", &tx.request, tx.json_budget.as_mut())
            .unwrap()
            .unwrap()
            .observed_bytes(&tx.request)
            .unwrap(),
        b"distinct"
    );
    staged.delete(b"nAmE", &tx.request, tx.json_budget.as_mut()).unwrap();
    assert!(staged.observed_get(b"Name", &tx.request).unwrap().is_none());
    tx.tx = full;
    tx.capture_keys = keys;
    let work = tx.json_budget.as_ref().unwrap().usage().work;
    assert_eq!(tx.tx_value(b"name"), Some(b"a".as_slice()));
    assert_eq!(tx.json_budget.as_ref().unwrap().usage().work, work);
    tx.json_budget.as_mut().unwrap().work(usize::MAX).unwrap_err();
    assert_eq!(tx.tx_value(b"NAME"), Some(b"a".as_slice()));
    let retained = tx.tx.retained_bytes;
    assert!(tx
        .tx
        .set(
            b"Name".to_vec(),
            b"lost".to_vec(),
            tx.limits,
            &tx.request,
            tx.json_budget.as_mut()
        )
        .is_err());
    assert!(tx.tx.delete(b"Name", &tx.request, tx.json_budget.as_mut()).is_err());
    assert_eq!(tx.tx.retained_bytes, retained);
    assert_eq!(tx.tx_value(b"Name"), Some(b"a".as_slice()));
}

#[test]
fn shared_capture_key_reads_enforce_exact_and_one_less_physical_work() {
    let generation = compile("SecRule ARGS \"@rx ^(?P<Name>a)$\" \"id:1,phase:2,pass,capture,t:none\"\n");
    for operation in ["get", "delete", "set"] {
        for remaining in [3, 2] {
            let result_limits = ResultLimits::default();
            let mut tx = no_hit_transaction(
                &generation,
                br#"{"x":"a"}"#,
                b"/",
                RuntimeLimits::default(),
                result_limits,
                false,
            );
            tx.execute_request_body().unwrap();
            assert_eq!(tx.tx.entries.len(), 3);
            let before = tx.json_budget.as_ref().unwrap().usage().work;
            tx.json_budget
                .as_mut()
                .unwrap()
                .work(result_limits.work - before - remaining)
                .unwrap();
            let result = match operation {
                "get" => tx
                    .tx
                    .get(b"name", &tx.request, tx.json_budget.as_mut())
                    .map(|value| assert!(value.is_some())),
                "delete" => tx.tx.delete(b"NAME", &tx.request, tx.json_budget.as_mut()),
                "set" => tx.tx.set(
                    b"nAmE".to_vec(),
                    b"new".to_vec(),
                    tx.limits,
                    &tx.request,
                    tx.json_budget.as_mut(),
                ),
                _ => unreachable!(),
            };
            assert_eq!(result.is_ok(), remaining == 3, "{operation}/{remaining}");
            if remaining == 3 {
                assert_eq!(tx.json_budget.as_ref().unwrap().usage().work, result_limits.work);
                assert!(tx.json_budget.as_ref().unwrap().failure().is_none());
            } else {
                assert!(tx.json_budget.as_ref().unwrap().failure().is_some());
                assert_eq!(tx.tx_value(b"Name"), Some(b"a".as_slice()));
                assert_eq!(tx.tx.entries.len(), 3);
                assert!(tx.tx.get(b"Name", &tx.request, tx.json_budget.as_mut()).is_err());
            }
            for key in &tx.capture_keys {
                assert!(key.observed_bytes(&tx.request).is_ok());
            }
        }
    }
}

#[test]
fn shared_named_key_selectors_actions_recapture_and_rollback_match_owned_runtime() {
    compare(
        concat!(
        "SecRule ARGS:json.a \"@rx ^(?P<Name>12)$\" \"id:1,phase:2,pass,capture,t:none,chain\"\n",
        "SecRule &TX:name \"@eq 1\" \"chain,setvar:tx.NAME=+3,setvar:tx.01=distinct\"\n",
        "SecRule TX:/^NAME$/ \"@eq 15\" \"setvar:tx.saved=%{TX.name}\"\n",
        "SecRule &TX:01 \"@eq 1\" \"id:2,phase:2,pass\"\n",
        "SecRule ARGS:json.b \"@rx ^(?P<Other>34)$\" \"id:3,phase:2,pass,capture,t:none,chain,setvar:!tx.NaMe\"\n",
        "SecRule TX:other \"@streq absent\" \"setvar:tx.lost=yes\"\n",
        "SecRule TX:name \"@eq 15\" \"id:4,phase:2,pass\"\n",
        "SecRule ARGS:json.b \"@rx ^(?P<Other>34)$\" \"id:5,phase:2,pass,capture,t:none,setvar:tx.last=%{TX.other}\"\n",
        "SecRule &TX:Name \"@eq 0\" \"id:6,phase:3,pass,setvar:tx.later=%{TX.Other}\"\n",
    ),
        br#"{"a":"12","b":"34"}"#,
        b"json.a=12&json.b=34",
        b"/",
        RuntimeLimits::default(),
    );
}

#[test]
fn shared_matched_vars_name_selection_and_macros_use_metered_reads() {
    compare(concat!(
        "SecRule ARGS_NAMES \"@contains long\" \"id:1,phase:2,pass,chain\"\n",
        "SecRule MATCHED_VARS:ARGS_NAMES:json.long_name \"@contains long\" \"chain,setvar:'tx.first=%{MATCHED_VAR_NAME}'\"\n",
        "SecRule MATCHED_VAR \"@contains long\" \"setvar:'tx.second=%{MATCHED_VAR_NAME}'\"\n",
    ), br#"{"long_name":"value"}"#, b"json.long_name=value", b"/", RuntimeLimits::default());
}

#[test]
fn shared_capture_payload_lengths_still_count_toward_tx_admission() {
    let generation = compile("SecRule ARGS \"@rx ^(.*)$\" \"id:1,phase:2,pass,capture\"\n");
    let length = 8192;
    let mut input = b"{\"a\":\"".to_vec();
    input.extend(std::iter::repeat_n(b'x', length));
    input.extend_from_slice(b"\"}");
    let mut tx = Transaction::new(
        &generation,
        head(b"/"),
        RuntimeLimits {
            max_tx_bytes: 4 * length - 1,
            ..RuntimeLimits::default()
        },
    )
    .unwrap();
    tx.execute_request_headers().unwrap();
    tx.publish_request_body(RequestBody::new(Bytes::from(input), BodyProcessorSelection::Json))
        .unwrap();
    assert!(matches!(
        tx.execute_request_body(),
        Err(RuntimeError::TxByteLimit { .. })
    ));
    assert_eq!(tx.tx_value(b"1"), None);
    assert!(tx.tx.matched_var.is_none());
}
