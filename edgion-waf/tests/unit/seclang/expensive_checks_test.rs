use super::*;
use crate::seclang::{
    analyze_one_semantic_draft, compile_validated, parse_syntax, scan_source, validate_semantic_draft,
    BodyProcessorSelection, EvaluationMode, PhaseDefaults, RequestBody, RequestHead, RequestHeader, RuleCheckPolicy,
    RuleEngineMode, RuntimeLimits, SemanticLimits, SemanticValidationContext, SourceLimits, SyntaxLimits, Transaction,
    ValidationLimits,
};

const ORIGINAL: &str = include_str!("../../fixtures/expensive-checks-crs-4.25.1.conf");
fn compile(text: &str) -> CompiledGeneration {
    let source = scan_source("optional-check-test", text.as_bytes(), SourceLimits::default()).unwrap();
    let syntax = parse_syntax(&source, SyntaxLimits::default()).unwrap();
    let draft = analyze_one_semantic_draft(syntax, SemanticLimits::default()).unwrap();
    let defaults = [
        Phase::RequestHeaders,
        Phase::RequestBody,
        Phase::ResponseHeaders,
        Phase::ResponseBody,
        Phase::Logging,
    ]
    .map(|p| PhaseDefaults::pass(p, true, true));
    let context = SemanticValidationContext::try_new(Some(Phase::RequestBody), &defaults).unwrap();
    let validated = validate_semantic_draft(draft, context, ValidationLimits::default()).unwrap();
    compile_validated(validated, None, CompilerLimits::default()).unwrap()
}
fn transaction<'a>(
    g: &'a CompiledGeneration,
    uri: &[u8],
    headers: &[RequestHeader<'_>],
    enabled: bool,
) -> Transaction<'a> {
    Transaction::new_with_policy(
        g,
        RequestHead::new(
            b"POST / HTTP/1.1",
            b"POST",
            uri,
            uri,
            b"HTTP/1.1",
            b"192.0.2.1",
            b"test",
            headers,
        ),
        RuntimeLimits::default(),
        EvaluationMode::Accelerated,
        RuleEngineMode::On,
        RuleCheckPolicy {
            expensive_checks: enabled,
        },
    )
    .unwrap()
}

#[test]
fn canonical_shapes_classify_without_changing_original_schedule() {
    let g = compile(ORIGINAL);
    assert!(g.has_expensive_checks);
    assert_eq!(
        g.units
            .iter()
            .filter(|u| u.expensive_check)
            .map(|u| u.id)
            .collect::<Vec<_>>(),
        vec![921170, 921180]
    );
    assert_eq!(g.units.len(), 4);
}

#[test]
fn off_avoids_histogram_tx_and_on_preserves_original_hpp() {
    let text = format!("SecAction \"id:1,phase:1,pass,setvar:tx.critical_anomaly_score=5\"\n{ORIGINAL}");
    let g = compile(&text);
    let mut retained = [0; 2];
    for enabled in [false, true] {
        let mut tx = transaction(&g, b"/?a=one&a=two", &[], enabled);
        tx.execute_request_headers().unwrap();
        tx.publish_request_body(RequestBody::new(bytes::Bytes::new(), BodyProcessorSelection::Auto))
            .unwrap();
        let outcome = tx.execute_request_body().unwrap();
        assert_eq!(
            tx.tx_value(b"paramcounter_ARGS_NAMES:a"),
            enabled.then_some(b"2".as_slice())
        );
        assert_eq!(outcome.findings().iter().any(|f| f.rule_id() == 921180), enabled);
        assert_eq!(outcome.evaluated_units(), if enabled { 4 } else { 2 });
        retained[usize::from(enabled)] = tx.retained_bytes();
    }
    assert!(retained[0] < retained[1]);
}

#[test]
fn changed_shapes_and_custom_dependencies_conservatively_keep_checks() {
    for extra in [
        "SecRule TX:paramcounter_ARGS_NAMES:a \"@eq 2\" \"id:20,phase:2,pass\"\n",
        "SecRule TX:/.*counter.*/ \"@eq 2\" \"id:20,phase:2,pass\"\n",
        "SecRule TX \"@eq 2\" \"id:20,phase:2,pass\"\n",
        "SecAction \"id:20,phase:2,pass,setvar:tx.saved=%{tx.paramcounter_custom}\"\n",
        "SecAction \"id:20,phase:2,pass,setvar:tx.%{MATCHED_VAR_NAME}=1\"\n",
        "SecAction \"id:20,phase:2,pass,setvar:tx.paramcounter_custom=1\"\n",
        "SecRule ARGS \"@rx (?P<paramcounter_custom>a)\" \"id:20,phase:2,pass,capture\"\n",
        "SecRule TX:/^header_name_920450_|paramcounter_/ \"@eq 2\" \"id:20,phase:2,pass\"\n",
    ] {
        assert!(!compile(&format!("{ORIGINAL}{extra}")).has_expensive_checks, "{extra}");
    }
    for (from, to) in [
        ("@rx .", "@rx a"),
        ("pass,\\\n    nolog", "pass,capture,\\\n    nolog"),
        ("@gt 1", "@gt 2"),
        ("t:none,t:lowercase", "t:sha1,t:none,t:lowercase"),
        ("@endsWith .%{request_headers.host}", "@endsWith .safe"),
        ("id:922110,", "id:922111,"),
    ] {
        assert!(ORIGINAL.contains(from));
        assert!(!compile(&ORIGINAL.replace(from, to)).has_expensive_checks, "{to}");
    }
    assert!(!compile(&ORIGINAL[..ORIGINAL.find("SecRule TX:/paramcounter_").unwrap()]).has_expensive_checks);
}

#[test]
fn disjoint_literal_dependencies_allow_policy_but_shared_attack_matches_survive() {
    let extra = "SecRule ARGS_NAMES \"@rx .\" \"id:20,phase:2,pass,setvar:tx.saved=yes\"\nSecRule TX:other \"@eq 2\" \"id:21,phase:2,pass\"\nSecRule TX:/^header_name_920450_/ \"@eq 2\" \"id:22,phase:2,pass\"\n";
    let g = compile(&format!("{ORIGINAL}{extra}"));
    assert!(g.has_expensive_checks);
    let headers = [RequestHeader::new(b"content-type", b"application/json")];
    let mut tx = transaction(&g, b"/", &headers, false);
    tx.execute_request_headers().unwrap();
    tx.publish_request_body(RequestBody::new(
        bytes::Bytes::from_static(br#"{"a":"one","a":"two"}"#),
        BodyProcessorSelection::Auto,
    ))
    .unwrap();
    let outcome = tx.execute_request_body().unwrap();
    assert!(outcome.findings().iter().any(|f| f.rule_id() == 20));
    assert_eq!(tx.tx_value(b"saved"), Some(b"yes".as_slice()));
    assert_eq!(tx.tx_value(b"paramcounter_ARGS_NAMES:json.a"), None);
}

#[test]
fn partial_multipart_and_rfi_attack_units_remain_active_with_off() {
    let setup = "SecAction \"id:1,phase:1,pass,setvar:tx.critical_anomaly_score=5,setvar:tx.multipart_headers_content_types_0=invalid\"\n";
    let g = compile(&format!("{setup}{ORIGINAL}"));
    assert!(g.has_expensive_checks);
    for enabled in [false, true] {
        let headers = [RequestHeader::new(b"host", b"local.example")];
        let mut tx = transaction(
            &g,
            b"/?paramcounter=one&rfi_parameter_x=http://evil.example/",
            &headers,
            enabled,
        );
        tx.execute_request_headers().unwrap();
        tx.publish_request_body(RequestBody::new(bytes::Bytes::new(), BodyProcessorSelection::Auto))
            .unwrap();
        let outcome = tx.execute_request_body().unwrap();
        assert!(outcome.findings().iter().any(|f| f.rule_id() == 922110));
        assert!(outcome.findings().iter().any(|f| f.rule_id() == 931130));
    }
}

#[test]
fn cheap_counts_and_mandatory_xml_grammar_remain_enabled_with_off() {
    let g = compile(&format!(
        "{ORIGINAL}SecRule &ARGS \"@gt 1\" \"id:20,phase:2,pass,setvar:tx.count_checked=yes\"\n"
    ));
    assert!(g.has_expensive_checks);
    let mut tx = transaction(&g, b"/?a=one&b=two", &[], false);
    tx.execute_request_headers().unwrap();
    tx.publish_request_body(RequestBody::new(bytes::Bytes::new(), BodyProcessorSelection::Auto))
        .unwrap();
    tx.execute_request_body().unwrap();
    assert_eq!(tx.tx_value(b"count_checked"), Some(b"yes".as_slice()));
    let headers = [RequestHeader::new(b"content-type", b"application/xml")];
    let mut tx = transaction(&g, b"/", &headers, false);
    tx.execute_request_headers().unwrap();
    assert!(tx
        .publish_request_body(RequestBody::new(
            bytes::Bytes::from_static(b"<r a='one' a='two'/>"),
            BodyProcessorSelection::Xml
        ))
        .is_err());
}

#[test]
fn schedule_skip_counts_include_optional_units_when_off() {
    let g = compile(&format!("SecRule ARGS:json.flag \"@eq 1\" \"id:20,phase:2,pass,skip:2\"\n{ORIGINAL}SecAction \"id:21,phase:2,pass,setvar:tx.end=yes\"\n"));
    assert!(g.has_expensive_checks);
    let headers = [RequestHeader::new(b"content-type", b"application/json")];
    for enabled in [false, true] {
        let mut tx = transaction(&g, b"/", &headers, enabled);
        tx.execute_request_headers().unwrap();
        tx.publish_request_body(RequestBody::new(
            bytes::Bytes::from_static(br#"{"flag":1}"#),
            BodyProcessorSelection::Auto,
        ))
        .unwrap();
        tx.execute_request_body().unwrap();
        assert_eq!(tx.tx_value(b"paramcounter_ARGS_NAMES:json.flag"), None);
        assert_eq!(tx.tx_value(b"end"), Some(b"yes".as_slice()));
    }
}

#[test]
fn optional_classifier_still_obeys_existing_compiler_work_limits() {
    let mut work = CanonicalWork::default();
    let limits = CompilerLimits {
        max_group_key_record_comparisons: 0,
        ..CompilerLimits::default()
    };
    let mut proof = Proof {
        work: &mut work,
        limits,
    };
    assert!(matches!(
        proof.histogram_key("paramcounter_a"),
        Err(CompilerError::GroupKeyRecordComparisonLimit { limit: 0 })
    ));
    let mut work = CanonicalWork::default();
    let limits = CompilerLimits {
        max_group_key_comparison_bytes: 0,
        ..CompilerLimits::default()
    };
    let mut proof = Proof {
        work: &mut work,
        limits,
    };
    assert!(matches!(
        proof.histogram_key("paramcounter_a"),
        Err(CompilerError::GroupKeyComparisonByteLimit { limit: 0 })
    ));
}

#[test]
fn target_updates_action_and_chain_changes_keep_original_computation() {
    for extra in [
        "SecRuleUpdateTargetById 921170 ARGS\n",
        "SecRuleUpdateTargetById 921180 TX:extra\n",
        "SecAction \"id:20,phase:2,pass,setvar:tx.saved=%{MATCHED_VAR}\"\n",
    ] {
        assert!(!compile(&format!("{ORIGINAL}{extra}")).has_expensive_checks);
    }
    let needle = "setvar:'TX.paramcounter_%{MATCHED_VAR_NAME}=+1'\"";
    for replacement in [
        "setvar:'TX.paramcounter_%{MATCHED_VAR_NAME}=+1',setvar:tx.extra=1\"",
        "setvar:'TX.paramcounter_%{MATCHED_VAR_NAME}=+1',chain\"\n SecRule ARGS \"@contains x\" \"\"",
    ] {
        assert!(ORIGINAL.contains(needle));
        assert!(!compile(&ORIGINAL.replace(needle, replacement)).has_expensive_checks);
    }
}

#[test]
fn json_and_form_hpp_computation_follow_explicit_policy() {
    let g = compile(&format!(
        "SecAction \"id:1,phase:1,pass,setvar:tx.critical_anomaly_score=5\"\n{ORIGINAL}"
    ));
    for (content_type, body, key) in [
        (
            b"application/json".as_slice(),
            br#"{"a":{"b":"one"},"a.b":"two"}"#.as_slice(),
            b"paramcounter_ARGS_NAMES:json.a.b".as_slice(),
        ),
        (
            b"application/x-www-form-urlencoded".as_slice(),
            b"a=one&a=two".as_slice(),
            b"paramcounter_ARGS_NAMES:a".as_slice(),
        ),
    ] {
        let headers = [RequestHeader::new(b"content-type", content_type)];
        for enabled in [false, true] {
            let mut tx = transaction(&g, b"/", &headers, enabled);
            tx.execute_request_headers().unwrap();
            tx.publish_request_body(RequestBody::new(
                bytes::Bytes::copy_from_slice(body),
                BodyProcessorSelection::Auto,
            ))
            .unwrap();
            let outcome = tx.execute_request_body().unwrap();
            assert_eq!(tx.tx_value(key), enabled.then_some(b"2".as_slice()));
            assert_eq!(outcome.findings().iter().any(|f| f.rule_id() == 921180), enabled);
        }
    }
}

#[test]
fn canonical_unanchored_readers_lose_only_the_declared_histogram_inputs() {
    let g = compile(&format!(
        "SecAction \"id:1,phase:1,pass,setvar:tx.critical_anomaly_score=5\"\n{ORIGINAL}"
    ));
    assert!(g.has_expensive_checks);
    let headers = [RequestHeader::new(b"host", b"local.example")];
    for (uri, incidental_rule) in [
        (b"/?MULTIPART_HEADERS_CONTENT_TYPES_x=safe".as_slice(), 922110),
        (b"/?rfi_parameter_x=http://local.example/".as_slice(), 931130),
    ] {
        for enabled in [false, true] {
            let mut tx = transaction(&g, uri, &headers, enabled);
            tx.execute_request_headers().unwrap();
            tx.publish_request_body(RequestBody::new(bytes::Bytes::new(), BodyProcessorSelection::Auto))
                .unwrap();
            let outcome = tx.execute_request_body().unwrap();
            assert_eq!(
                outcome.findings().iter().any(|f| f.rule_id() == incidental_rule),
                enabled
            );
        }
    }
}

#[test]
fn skip_after_keeps_shared_names_for_a_later_phase_consumer() {
    let g = compile(&format!(
        "SecRule REQUEST_METHOD \"@streq POST\" \"id:10,phase:2,pass,skipAfter:END\"\n{ORIGINAL}SecMarker END\nSecRule ARGS_NAMES \"@rx .\" \"id:20,phase:3,pass,setvar:tx.later=yes\"\n"
    ));
    assert!(g.has_expensive_checks);
    let headers = [RequestHeader::new(b"content-type", b"application/json")];
    for enabled in [false, true] {
        let mut tx = transaction(&g, b"/", &headers, enabled);
        tx.execute_request_headers().unwrap();
        tx.publish_request_body(RequestBody::new(
            bytes::Bytes::from_static(br#"{"flag":"safe"}"#),
            BodyProcessorSelection::Auto,
        ))
        .unwrap();
        assert_eq!(tx.execute_request_body().unwrap().evaluated_units(), 1);
        assert_eq!(tx.tx_value(b"paramcounter_ARGS_NAMES:json.flag"), None);
        tx.publish_response_head(crate::seclang::ResponseHead::new(200, &[]))
            .unwrap();
        tx.execute_response_headers().unwrap();
        assert_eq!(tx.tx_value(b"later"), Some(b"yes".as_slice()));
    }
}

const ALL_ORIGINAL: &str = include_str!("../../fixtures/expensive-aggregates-crs-4.25.1.conf");
const MULTIPART_BODY: &[u8] = b"--boundary\r\nContent-Disposition: form-data; name=\"part\"\r\nContent-Type: invalid\r\n\r\nsafe\r\n--boundary--\r\n";

#[test]
fn unanchored_custom_literal_dynamic_and_capture_writers_keep_hpp() {
    for extra in [
        "SecAction \"id:20,phase:1,pass,setvar:tx.other_paramcounter_custom=2\"\n",
        "SecAction \"id:20,phase:1,pass,setvar:tx.alias=paramcounter_custom,setvar:'tx.other_%{tx.alias}=2'\"\n",
        "SecRule ARGS_GET:a \"@rx (?P<other_paramcounter_custom>2)\" \"id:20,phase:1,pass,capture\"\n",
    ] {
        let g = compile(&format!(
            "SecAction \"id:1,phase:1,pass,setvar:tx.critical_anomaly_score=5\"\n{ORIGINAL}{extra}"
        ));
        assert!(!g.has_expensive_checks, "{extra}");
        for enabled in [false, true] {
            let mut tx = transaction(&g, b"/?a=2", &[], enabled);
            tx.execute_request_headers().unwrap();
            tx.publish_request_body(RequestBody::new(bytes::Bytes::new(), BodyProcessorSelection::Auto))
                .unwrap();
            let outcome = tx.execute_request_body().unwrap();
            assert!(outcome.findings().iter().any(|f| f.rule_id() == 921180));
            assert_eq!(tx.tx_value(b"inbound_anomaly_score_pl3"), Some(b"5".as_slice()));
        }
    }
}

#[test]
fn complete_families_omit_aggregate_state_and_preserve_collector_captures() {
    let g = compile(&format!(
        "SecAction \"id:1,phase:1,pass,setvar:tx.critical_anomaly_score=5\"\n{ALL_ORIGINAL}"
    ));
    let optional_ids = g
        .units
        .iter()
        .filter(|u| u.expensive_check)
        .map(|u| u.id)
        .collect::<Vec<_>>();
    assert_eq!(optional_ids, vec![921170, 921180, 922140, 922110]);
    let collector = g.units.iter().find(|u| u.id == 922150).unwrap();
    assert!(g.members[collector.member_start].expensive_aggregate_actions);
    let headers = [RequestHeader::new(
        b"content-type",
        b"multipart/form-data; boundary=boundary",
    )];
    for enabled in [false, true] {
        let mut tx = transaction(&g, b"/", &headers, enabled);
        tx.execute_request_headers().unwrap();
        tx.publish_request_body(RequestBody::new(
            bytes::Bytes::from_static(MULTIPART_BODY),
            BodyProcessorSelection::Auto,
        ))
        .unwrap();
        let outcome = tx.execute_request_body().unwrap();
        assert_eq!(
            tx.tx_value(b"multipart_headers_content_counter"),
            enabled.then_some(b"1".as_slice())
        );
        assert_eq!(
            tx.tx_value(b"multipart_headers_content_types_0"),
            enabled.then_some(b"invalid".as_slice())
        );
        assert_eq!(tx.tx_value(b"1"), Some(b"invalid".as_slice()));
        assert_eq!(outcome.findings().iter().any(|f| f.rule_id() == 922110), enabled);
    }
}

#[test]
fn multipart_namespace_dependencies_and_canonical_mutations_retain_family() {
    for extra in [
        "SecRule TX:multipart_headers_content_types_0 \"@contains invalid\" \"id:20,phase:2,pass\"\n",
        "SecAction \"id:20,phase:2,pass,setvar:tx.saved=%{tx.multipart_headers_content_counter}\"\n",
        "SecAction \"id:20,phase:1,pass,setvar:tx.other_multipart_headers_content_types_custom=invalid\"\n",
        "SecAction \"id:20,phase:1,pass,setvar:'tx.other_%{tx.alias}=invalid'\"\n",
        "SecRule ARGS_GET \"@rx (?P<other_multipart_headers_content_types_custom>x)\" \"id:20,phase:1,pass,capture\"\n",
    ] {
        let g = compile(&format!("{ALL_ORIGINAL}{extra}"));
        assert!(
            !g.units.iter().find(|u| u.id == 922110).unwrap().expensive_check,
            "{extra}"
        );
        let collector = g.units.iter().find(|u| u.id == 922150).unwrap();
        assert!(!g.members[collector.member_start].expensive_aggregate_actions);
    }
    for (from, to) in [
        ("@gt 0", "@gt 1"),
        ("header_name_920450_%{tx.0}", "custom_%{tx.0}"),
        (
            "multipart_headers_content_counter=+1",
            "multipart_headers_content_counter=+2",
        ),
    ] {
        assert!(ALL_ORIGINAL.contains(from));
        let g = compile(&ALL_ORIGINAL.replace(from, to));
        assert!(!g.units.iter().find(|u| u.id == 922110).unwrap().expensive_check);
    }
}

#[test]
fn multipart_policy_preserves_numeric_slots_named_cleanup_and_sentinel_case() {
    for (before, after, expected) in [
        (
            "SecRule ARGS_GET:a \"@rx (?P<foo>x)\" \"id:20,phase:1,pass,capture\"\n",
            "SecAction \"id:21,phase:2,pass,setvar:tx.saved=%{tx.foo},setvar:tx.numeric=%{tx.1}\"\n",
            b"".as_slice(),
        ),
        (
            "SecRule ARGS_GET:a \"@rx ^x$\" \"id:20,phase:1,pass,capture,setvar:tx.1=sentinel\"\n",
            "SecRule ARGS_GET:a \"@rx ^x$\" \"id:21,phase:2,pass,capture,setvar:tx.saved=%{tx.1}\"\n",
            b"".as_slice(),
        ),
        (
            "",
            "SecRule TX:1 \"@streq invalid\" \"id:21,phase:2,pass,setvar:tx.saved=invalid\"\n",
            b"invalid".as_slice(),
        ),
        (
            "SecRule ARGS_GET:a \"@rx (?P<foo>x)\" \"id:20,phase:1,pass,capture\"\n",
            "SecRule TX:foo \"@streq x\" \"id:21,phase:2,pass,setvar:tx.saved=x\"\n",
            b"absent".as_slice(),
        ),
    ] {
        let g = compile(&format!(
            "SecAction \"id:1,phase:1,pass,setvar:tx.critical_anomaly_score=5\"\n{before}{ALL_ORIGINAL}{after}"
        ));
        assert!(g.units.iter().find(|u| u.id == 922110).unwrap().expensive_check);
        let headers = [RequestHeader::new(
            b"content-type",
            b"multipart/form-data; boundary=boundary",
        )];
        for enabled in [false, true] {
            let mut tx = transaction(&g, b"/?a=x", &headers, enabled);
            tx.execute_request_headers().unwrap();
            tx.publish_request_body(RequestBody::new(
                bytes::Bytes::from_static(MULTIPART_BODY),
                BodyProcessorSelection::Auto,
            ))
            .unwrap();
            tx.execute_request_body().unwrap();
            assert_eq!(tx.tx_value(b"saved"), (expected != b"absent").then_some(expected));
            assert_eq!(tx.tx_value(b"foo"), None);
            assert_eq!(
                tx.tx_value(b"multipart_headers_content_types_0"),
                enabled.then_some(b"invalid".as_slice())
            );
        }
    }
}

#[test]
fn canonical_header_check_survives_and_incidental_hpp_error_is_optional() {
    let g = compile(&format!(
        "SecAction \"id:1,phase:1,pass,setvar:tx.critical_anomaly_score=5,setvar:'tx.restricted_headers_basic=/paramcounter_custom/'\"\n{ALL_ORIGINAL}"
    ));
    assert!(g.has_expensive_checks);
    let headers = [RequestHeader::new(b"paramcounter_custom", b"safe")];
    for enabled in [false, true] {
        let mut tx = transaction(&g, b"/", &headers, enabled);
        let head = tx.execute_request_headers().unwrap();
        assert!(head.findings().iter().any(|f| f.rule_id() == 920450));
        assert_eq!(
            tx.tx_value(b"header_name_920450_paramcounter_custom"),
            Some(b"/paramcounter_custom/".as_slice())
        );
        tx.publish_request_body(RequestBody::new(bytes::Bytes::new(), BodyProcessorSelection::Auto))
            .unwrap();
        let outcome = tx.execute_request_body();
        assert_eq!(outcome.is_err(), enabled);
        if enabled {
            assert!(matches!(outcome, Err(crate::seclang::RuntimeError::IntegerConversion)));
        }
    }
}

#[test]
fn off_keeps_other_part_header_predicates_and_independent_hpp() {
    let g = compile(&format!("{ALL_ORIGINAL}SecRule MULTIPART_PART_HEADERS \"@contains Content-Transfer-Encoding\" \"id:20,phase:2,pass,setvar:tx.direct=matched\"\nSecRule TX:multipart_headers_content_counter \"@ge 0\" \"id:21,phase:2,pass\"\n"));
    assert!(g.units.iter().find(|u| u.id == 921170).unwrap().expensive_check);
    assert!(!g.units.iter().find(|u| u.id == 922110).unwrap().expensive_check);
    let g = compile(&format!("{ALL_ORIGINAL}SecRule MULTIPART_PART_HEADERS \"@contains Content-Transfer-Encoding\" \"id:20,phase:2,pass,setvar:tx.direct=matched\"\n"));
    let body = String::from_utf8(MULTIPART_BODY.to_vec()).unwrap().replace(
        "Content-Type: invalid",
        "Content-Type: text/plain\r\nContent-Transfer-Encoding: base64",
    );
    let headers = [RequestHeader::new(
        b"content-type",
        b"multipart/form-data; boundary=boundary",
    )];
    let mut tx = transaction(&g, b"/", &headers, false);
    tx.execute_request_headers().unwrap();
    tx.publish_request_body(RequestBody::new(bytes::Bytes::from(body), BodyProcessorSelection::Auto))
        .unwrap();
    tx.execute_request_body().unwrap();
    assert_eq!(tx.tx_value(b"direct"), Some(b"matched".as_slice()));
    assert_eq!(tx.tx_value(b"multipart_headers_content_types_0"), None);
}

#[test]
fn local_chain_observers_and_macros_allow_optional_aggregates() {
    let g = compile(&format!("{ALL_ORIGINAL}\nSecRule ARGS:a \"@contains safe\" \"id:20,phase:2,pass,chain\"\n SecRule MATCHED_VARS \"@contains safe\" \"chain\"\n SecRule MATCHED_VARS \"@streq %{{MATCHED_VAR}}\" \"setvar:tx.local_result=%{{MATCHED_VAR}}\"\n"));
    assert_eq!(g.units.iter().filter(|u| u.expensive_check).count(), 4);
    for enabled in [false, true] {
        let mut tx = transaction(&g, b"/?a=safe", &[], enabled);
        tx.execute_request_headers().unwrap();
        tx.publish_request_body(RequestBody::new(bytes::Bytes::new(), BodyProcessorSelection::Auto))
            .unwrap();
        let outcome = tx.execute_request_body().unwrap();
        assert!(outcome.findings().iter().any(|f| f.rule_id() == 20));
        assert_eq!(tx.tx_value(b"local_result"), Some(b"safe".as_slice()));
        assert_eq!(
            tx.tx_value(b"paramcounter_ARGS_NAMES:a"),
            enabled.then_some(b"1".as_slice())
        );
    }
}

#[test]
fn chain_locality_never_hides_direct_aggregate_aliases() {
    for extra in [
        "SecRule TX:paramcounter_custom \"@eq 2\" \"id:20,phase:2,pass,chain\"\n SecRule MATCHED_VARS \"@contains 2\" \"\"\n",
        "SecRule ARGS:a \"@contains safe\" \"id:20,phase:2,pass,chain\"\n SecRule TX:paramcounter_custom \"@eq 2\" \"\"\n",
        "SecRule ARGS:a \"@contains safe\" \"id:20,phase:2,pass,chain\"\n SecRule MATCHED_VARS \"@contains safe\" \"setvar:tx.other_%{MATCHED_VAR}=2\"\n",
        "SecRule MATCHED_VARS \"@contains safe\" \"id:20,phase:2,pass\"\n",
    ] {
        assert!(!compile(&format!("{ORIGINAL}{extra}")).has_expensive_checks, "{extra}");
    }
    let g = compile(&format!("{ALL_ORIGINAL}SecRule TX:multipart_headers_content_types_0 \"@contains plain\" \"id:20,phase:2,pass,chain\"\n SecRule MATCHED_VARS \"@contains plain\" \"\"\n"));
    assert_eq!(
        g.units
            .iter()
            .filter(|u| u.expensive_check)
            .map(|u| u.id)
            .collect::<Vec<_>>(),
        vec![921170, 921180]
    );
}
