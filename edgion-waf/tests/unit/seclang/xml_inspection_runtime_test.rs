use super::super::json_inspection::StoreError;
use super::*;
use crate::seclang::compiler::{CanonicalWork, MatcherCompileWork};
use crate::seclang::{
    compile_with_includes, FrontendLimits, IncludeFailure, IncludeRequest, IncludeSink, IncludeSource, PhaseDefaults,
    SemanticValidationContext,
};
struct NoIncludes;
impl IncludeSource for NoIncludes {
    fn load(&mut self, _: IncludeRequest<'_>, _: &mut IncludeSink<'_>) -> Result<(), IncludeFailure> {
        Err(IncludeFailure::NotFound)
    }
}
fn compile(rules: &str) -> CompiledGeneration {
    let phases = [
        PhaseDefaults::pass(Phase::RequestHeaders, true, true),
        PhaseDefaults::pass(Phase::RequestBody, true, true),
    ];
    let context = SemanticValidationContext::try_new(Some(Phase::RequestBody), &phases).unwrap();
    compile_with_includes(
        "xml-runtime",
        rules.as_bytes(),
        &mut NoIncludes,
        None,
        context,
        FrontendLimits::default(),
    )
    .unwrap()
    .into_parts()
    .0
}
fn transaction(g: &CompiledGeneration, mode: RuleEngineMode, limits: RuntimeLimits) -> Transaction<'_> {
    let headers = [RequestHeader::new(b"content-type", b"application/xml")];
    let head = RequestHead::new(
        b"POST /?q=one HTTP/1.1",
        b"POST",
        b"/?q=one",
        b"/?q=one",
        b"HTTP/1.1",
        b"192.0.2.1",
        b"xml-runtime",
        &headers,
    );
    let mut tx = Transaction::new_with_modes(g, head, limits, EvaluationMode::Accelerated, mode).unwrap();
    tx.execute_request_headers().unwrap();
    tx
}
fn publish(tx: &mut Transaction<'_>, bytes: &[u8]) -> Result<(), RuntimeError> {
    tx.publish_request_body(RequestBody::new(
        Bytes::copy_from_slice(bytes),
        BodyProcessorSelection::Auto,
    ))
}
#[test]
fn source_order_chain_capture_and_qualified_names_replay_after_seal() {
    let g = compile(concat!(
        "SecRule XML://@*|XML:/* \"@rx (attack)\" \"id:1,phase:2,pass,capture,setvar:tx.first=%{MATCHED_VAR_NAME},setvar:tx.captured=%{TX.1},chain\"\n",
        "SecRule TX:captured \"@streq attack\" \"setvar:tx.chain=done\"\n",
        "SecRule XML:/* \"@contains attack\" \"id:2,phase:2,pass,setvar:tx.text=%{MATCHED_VAR_NAME}\"\n"
    ));
    let xml = b"<r a='attack'>attack</r>";
    for chunk in [1, 3, xml.len()] {
        let mut tx = transaction(&g, RuleEngineMode::On, RuntimeLimits::default());
        let mut session = tx
            .start_xml_inspection(XmlInspectionLimits::default(), Some(xml.len()))
            .unwrap();
        for part in xml.chunks(chunk) {
            session.feed(part).unwrap();
        }
        session.complete(xml.len()).unwrap();
        assert!(!session.requires_raw_source());
        session.finish(None).unwrap();
        assert_eq!(tx.tx_value(b"first"), None);
        let outcome = tx.execute_request_body().unwrap();
        assert_eq!(outcome.findings().len(), 2);
        assert_eq!(tx.tx_value(b"first"), Some(b"XML:a".as_slice()));
        assert_eq!(tx.tx_value(b"captured"), Some(b"attack".as_slice()));
        assert_eq!(tx.tx_value(b"chain"), Some(b"done".as_slice()));
        assert_eq!(tx.tx_value(b"text"), Some(b"XML".as_slice()));
    }
}
#[test]
fn count_dynamic_exclusion_and_controls_apply_at_their_source_position() {
    let g = compile(concat!(
        "SecAction \"id:1,phase:2,pass,setvar:tx.count=2\"\n",
        "SecRule &XML:/* \"@eq %{TX.count}\" \"id:2,phase:2,pass,setvar:tx.counted=yes\"\n",
        "SecRule XML:/*|XML://@*|!XML://@* \"@streq attack\" \"id:3,phase:2,pass,setvar:tx.before=yes,tag:check\"\n",
        "SecRule TX:counted \"@streq yes\" \"id:901181,phase:2,pass,ctl:ruleRemoveTargetByTag=check;XML://@*\"\n",
        "SecRule XML://@* \"@streq attack\" \"id:4,phase:2,deny,tag:check\"\n",
        "SecRule XML:/* \"@streq attack\" \"id:5,phase:2,pass,setvar:tx.literal=yes\"\n"
    ));
    let mut tx = transaction(&g, RuleEngineMode::On, RuntimeLimits::default());
    publish(&mut tx, b"<r a='attack' b='attack'>attack<x/>safe</r>").unwrap();
    let outcome = tx.execute_request_body().unwrap();
    assert!(outcome.intervention.is_none());
    assert_eq!(tx.tx_value(b"counted"), Some(b"yes".as_slice()));
    assert_eq!(tx.tx_value(b"before"), Some(b"yes".as_slice()));
    assert_eq!(tx.tx_value(b"literal"), Some(b"yes".as_slice()));
    assert!(!outcome.findings().iter().any(|finding| finding.rule_id == 4));
}
#[test]
fn malformed_provisional_hits_abandon_without_tx_and_only_detection_falls_back() {
    let g = compile("SecRule XML \"@contains attack\" \"id:1,phase:2,deny,setvar:tx.hit=yes\"\nSecRule REQUEST_BODY \"@contains attack\" \"id:2,phase:2,pass,setvar:tx.raw=yes\"");
    let xml = b"<r a='attack'>attack</wrong>";
    let mut on = transaction(&g, RuleEngineMode::On, RuntimeLimits::default());
    assert!(matches!(
        publish(&mut on, xml),
        Err(RuntimeError::Body {
            kind: BodyError::Malformed { .. }
        })
    ));
    assert_eq!(on.tx_value(b"hit"), None);
    assert!(on.execute_request_body().is_err());
    let mut detection = transaction(&g, RuleEngineMode::DetectionOnly, RuntimeLimits::default());
    publish(&mut detection, xml).unwrap();
    let outcome = detection.execute_request_body().unwrap();
    assert!(outcome.intervention.is_none());
    assert_eq!(detection.tx_value(b"hit"), None);
    assert_eq!(detection.tx_value(b"raw"), Some(b"yes".as_slice()));
    assert!(detection.request_body_error().is_some());
}
#[test]
fn unbound_attribute_prefix_keeps_detection_fallback_at_exact_syntax_work() {
    use crate::seclang::{XmlStreamSink, XmlStreamSinkError};
    struct Values;
    impl XmlStreamSink for Values {
        fn value(&mut self, _: Option<&[u8]>, _: &[u8]) -> Result<(), XmlStreamSinkError> {
            Ok(())
        }
    }
    let xml = b"<r p:x='v'/>";
    let mut parser = XmlStream::new(XmlStreamLimits::default());
    assert_eq!(
        parser.feed(XmlStreamInput::Data(xml), &mut Values),
        Err(XmlStreamError::Syntax)
    );
    let exact = parser.usage().work;
    let g = compile(concat!(
        "SecRule XML://@* \"@contains v\" \"id:1,phase:2,pass,setvar:tx.xml=yes\"\n",
        "SecRule REQUEST_BODY \"@contains p:x\" \"id:2,phase:2,pass,setvar:tx.raw=yes\""
    ));
    for scoped in [false, true] {
        for physical_failure in [false, true] {
            let limits = XmlInspectionLimits {
                parser: XmlStreamLimits {
                    max_work: exact - usize::from(physical_failure),
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut tx = transaction(&g, RuleEngineMode::DetectionOnly, RuntimeLimits::default());
            let expected = RuntimeError::XmlStream {
                kind: XmlStreamError::Limit {
                    kind: XmlStreamLimit::Work,
                    limit: limits.parser.max_work,
                },
            };
            if scoped {
                let mut session = tx.start_xml_inspection(limits, Some(xml.len())).unwrap();
                if physical_failure {
                    assert_eq!(session.feed(xml), Err(expected));
                    assert_eq!(session.feed(b""), Err(expected));
                    assert_eq!(session.complete(xml.len()), Err(expected));
                } else {
                    session.feed(xml).unwrap();
                    session.complete(xml.len()).unwrap();
                    session.finish(Some(Bytes::from_static(xml))).unwrap();
                }
            } else {
                let result = tx.publish_request_body_with_xml_limits(
                    RequestBody::new(Bytes::from_static(xml), BodyProcessorSelection::Xml),
                    limits,
                );
                assert_eq!(result, if physical_failure { Err(expected) } else { Ok(()) });
            }
            if physical_failure {
                assert_eq!(tx.execute_request_body(), Err(expected));
                assert_eq!(tx.execute_request_body(), Err(expected));
            } else {
                let outcome = tx.execute_request_body().unwrap();
                assert_eq!(outcome.findings().len(), 1);
                assert_eq!(tx.tx_value(b"xml"), None);
                assert_eq!(tx.tx_value(b"raw"), Some(b"yes".as_slice()));
                assert!(matches!(tx.request_body_error(), Some(BodyError::Malformed { .. })));
            }
        }
    }
}
#[test]
fn source_physical_cap_is_sticky_even_after_detection_syntax_failure() {
    let g = compile("SecRule XML \"@contains attack\" \"id:1,phase:2,pass\"");
    let limits = XmlInspectionLimits {
        parser: XmlStreamLimits {
            max_source_bytes: 8,
            ..XmlStreamLimits::default()
        },
        ..XmlInspectionLimits::default()
    };
    let expected = RuntimeError::XmlStream {
        kind: XmlStreamError::Limit {
            kind: XmlStreamLimit::SourceBytes,
            limit: 8,
        },
    };
    for scoped in [false, true] {
        let mut tx = transaction(&g, RuleEngineMode::DetectionOnly, RuntimeLimits::default());
        if scoped {
            let mut s = tx.start_xml_inspection(limits, None).unwrap();
            s.feed(b"!").unwrap();
            assert_eq!(s.feed(b"12345678"), Err(expected));
            assert_eq!(s.feed(b""), Err(expected));
            assert_eq!(s.complete(9), Err(expected));
        } else {
            assert_eq!(
                tx.publish_request_body_with_xml_limits(
                    RequestBody::new(Bytes::from_static(b"!12345678"), BodyProcessorSelection::Xml),
                    limits
                ),
                Err(expected)
            );
        }
        assert_eq!(tx.execute_request_body(), Err(expected));
    }
    let mut exact = transaction(&g, RuleEngineMode::On, RuntimeLimits::default());
    exact
        .publish_request_body_with_xml_limits(
            RequestBody::new(Bytes::from_static(b"<r></r> "), BodyProcessorSelection::Xml),
            limits,
        )
        .unwrap();
}
#[test]
fn no_demand_still_checks_all_logical_values_and_empty_transport_exception() {
    let g = compile("SecAction \"id:1,phase:2,pass\"");
    let mut limits = RuntimeLimits::default();
    limits.body.max_entries = 4;
    let mut tx = transaction(&g, RuleEngineMode::On, limits);
    publish(&mut tx, b"<r>x</r>").unwrap();
    assert_eq!(tx.request.body.as_ref().unwrap().xml().unwrap().summary.entries, 4);
    let store = tx.request.body.as_ref().unwrap().structured_results().unwrap();
    assert!(store.receipt.is_none());
    let mut over = transaction(&g, RuleEngineMode::On, limits);
    assert!(matches!(
        publish(&mut over, b"<r a='x'>x</r>"),
        Err(RuntimeError::Body {
            kind: BodyError::EntryLimit { .. }
        })
    ));
    let mut empty = transaction(&g, RuleEngineMode::On, RuntimeLimits::default());
    publish(&mut empty, b"").unwrap();
    empty.execute_request_body().unwrap();
    for nonempty in [b" ".as_slice(), b"<!--empty-->", b"<?p?>"] {
        let mut tx = transaction(&g, RuleEngineMode::On, RuntimeLimits::default());
        assert!(publish(&mut tx, nonempty).is_err());
    }
}
#[test]
fn raw_visibility_is_explicit_and_metadata_is_query_only() {
    let g = compile("SecRule REQUEST_BODY \"@contains safe\" \"id:1,phase:2,pass,setvar:tx.raw=yes\"\nSecRule ARGS_COMBINED_SIZE \"@eq 4\" \"id:2,phase:2,pass,setvar:tx.combined=yes\"");
    let mut tx = transaction(&g, RuleEngineMode::On, RuntimeLimits::default());
    publish(&mut tx, b"<r>safe</r>").unwrap();
    tx.execute_request_body().unwrap();
    assert_eq!(tx.tx_value(b"raw"), None);
    assert_eq!(tx.tx_value(b"combined"), Some(b"yes".as_slice()));
    let forced = compile("SecAction \"id:1,phase:1,pass,ctl:forceRequestBodyVariable=On\"\nSecRule REQUEST_BODY \"@contains safe\" \"id:2,phase:2,pass,setvar:tx.raw=yes\"");
    let mut tx = transaction(&forced, RuleEngineMode::On, RuntimeLimits::default());
    let mut s = tx.start_xml_inspection(XmlInspectionLimits::default(), None).unwrap();
    s.feed(b"<r>safe</r>").unwrap();
    s.complete(11).unwrap();
    assert!(s.requires_raw_source());
    s.finish(Some(Bytes::from_static(b"<r>safe</r>"))).unwrap();
    tx.execute_request_body().unwrap();
    assert_eq!(tx.tx_value(b"raw"), Some(b"yes".as_slice()));
}
#[test]
fn unsupported_value_dependencies_and_unfinished_drop_fail_closed() {
    let g = compile("SecRule XML \"@contains %{TX.dynamic}\" \"id:1,phase:2,pass\"");
    assert_eq!(
        g.xml_inspection_unsupported()[0].reason,
        crate::seclang::XmlInspectionUnsupportedReason::DynamicValueOperand
    );
    let mut tx = transaction(&g, RuleEngineMode::On, RuntimeLimits::default());
    assert!(matches!(
        publish(&mut tx, b"<r/>"),
        Err(RuntimeError::XmlUnsupported { rule_id: 1 })
    ));
    assert!(matches!(
        tx.execute_request_body(),
        Err(RuntimeError::XmlUnsupported { .. })
    ));
    let mut scoped = transaction(&g, RuleEngineMode::DetectionOnly, RuntimeLimits::default());
    assert!(matches!(
        scoped.start_xml_inspection(XmlInspectionLimits::default(), None),
        Err(RuntimeError::XmlUnsupported { rule_id: 1 })
    ));
    assert_eq!(
        scoped.execute_request_body(),
        Err(RuntimeError::XmlUnsupported { rule_id: 1 })
    );
    let g = compile("SecRule XML \"@contains attack\" \"id:1,phase:2,pass\"");
    let mut tx = transaction(&g, RuleEngineMode::On, RuntimeLimits::default());
    {
        let mut s = tx.start_xml_inspection(XmlInspectionLimits::default(), None).unwrap();
        s.feed(b"<r>").unwrap();
    }
    assert_eq!(
        tx.execute_request_body(),
        Err(RuntimeError::XmlStream {
            kind: XmlStreamError::Abandoned
        })
    );
}

#[test]
fn xml_selected_names_captures_macro_reads_and_sticky_replay_budgets_keep_xml_domain() {
    let g = compile("SecRule XML://@* \"@rx (attack)\" \"id:1,phase:2,pass,capture,setvar:tx.saved=%{TX.1},setvar:tx.name=%{MATCHED_VAR_NAME}\"");
    let mut tx = transaction(&g, RuleEngineMode::On, RuntimeLimits::default());
    publish(&mut tx, b"<r a='attack'/>").unwrap();
    tx.execute_request_body().unwrap();
    assert_eq!(tx.tx_value(b"saved"), Some(b"attack".as_slice()));
    let id = tx
        .request
        .body
        .as_ref()
        .unwrap()
        .structured_results()
        .unwrap()
        .receipt
        .unwrap()
        .head;
    assert_eq!(
        tx.json_budget.as_mut().unwrap().work(JsonResultLimits::default().work),
        Err(StoreError::Work)
    );
    let expected = RuntimeError::XmlResultLimit {
        kind: JsonResultLimit::Work,
    };
    assert_eq!(
        selected_name_bytes(
            Some(SelectedName::Xml(id)),
            &tx.request,
            &tx.tx,
            None,
            tx.json_budget.as_mut()
        ),
        Err(expected)
    );
    let captured = tx
        .tx
        .entries
        .iter()
        .find(|entry| entry.key.observed_bytes(&tx.request).unwrap() == b"saved")
        .unwrap();
    assert_eq!(
        captured.value.bytes(&tx.request, tx.json_budget.as_mut()),
        Ok(b"attack".as_slice())
    );
    assert_eq!(
        tx.tx
            .matched_var
            .as_ref()
            .unwrap()
            .bytes(&tx.request, tx.json_budget.as_mut()),
        Err(expected)
    );
    let macro_captured = tx
        .tx
        .entries
        .iter()
        .find(|entry| entry.key.observed_bytes(&tx.request).unwrap() == b"1")
        .unwrap();
    assert_eq!(
        macro_captured.value.bytes(&tx.request, tx.json_budget.as_mut()),
        Err(expected)
    );
    assert_eq!(tx.execute_request_body(), Err(expected));
    assert_eq!(tx.execute_request_body(), Err(expected));
    assert_eq!(tx.preflight_request_body(BodyProcessorSelection::Xml), Err(expected));
}
#[test]
fn xml_attribute_replay_exhaustion_is_attributable_after_successful_publication() {
    let g = compile("SecRule XML://@* \"@contains attack\" \"id:1,phase:2,pass\"");
    let body = RequestBody::new(Bytes::from_static(b"<r a='attack'/>"), BodyProcessorSelection::Xml);
    let mut baseline = transaction(&g, RuleEngineMode::On, RuntimeLimits::default());
    baseline.publish_request_body(body.clone()).unwrap();
    baseline.execute_request_body().unwrap();
    let exact = baseline.json_budget.as_ref().unwrap().usage().work;
    let expected = RuntimeError::XmlResultLimit {
        kind: JsonResultLimit::Work,
    };
    for succeeds in [true, false] {
        let mut tx = transaction(&g, RuleEngineMode::On, RuntimeLimits::default());
        let limits = XmlInspectionLimits {
            results: JsonResultLimits {
                work: exact - usize::from(!succeeds),
                ..Default::default()
            },
            ..Default::default()
        };
        tx.publish_request_body_with_xml_limits(body.clone(), limits).unwrap();
        if succeeds {
            assert_eq!(tx.execute_request_body().unwrap().findings().len(), 1);
            assert_eq!(tx.json_budget.as_ref().unwrap().usage().work, exact);
        } else {
            assert_eq!(tx.execute_request_body(), Err(expected));
            assert_eq!(tx.execute_request_body(), Err(expected));
            assert_eq!(tx.preflight_request_body(BodyProcessorSelection::Xml), Err(expected));
        }
    }
}

#[test]
fn xml_static_miss_selection_and_replay_charge_only_remaining_checked_reads() {
    for (targets, reads) in [("XML", 8), ("XML:/*", 4), ("XML://@*", 4), ("XML:/*|XML://@*", 10)] {
        let g = compile(&format!("SecRule {targets} \"@contains absent\" \"id:1,phase:2,pass\""));
        let mut tx = transaction(&g, RuleEngineMode::On, RuntimeLimits::default());
        publish(&mut tx, b"<r a='safe'>safe</r>").unwrap();
        let before = tx.json_budget.as_ref().unwrap().usage().work;
        assert!(tx.execute_request_body().unwrap().findings().is_empty());
        assert_eq!(
            tx.json_budget.as_ref().unwrap().usage().work - before,
            reads,
            "{targets}"
        );
    }
}

#[test]
fn canonical_literal_regex_and_count_xml_attribute_ir_never_selects_unnamed_text() {
    // The source grammar reserves XML suffixes for Path. Preserve the existing
    // canonical Literal/Regex IR contracts without inventing new source syntax.
    let mut g = compile(concat!(
        "SecRule ARGS:a|ARGS:/^a$/ \"@contains attack\" \"id:1,phase:2,pass,setvar:tx.selected=%{MATCHED_VAR_NAME}\"\n",
        "SecRule &ARGS:a \"@eq 1\" \"id:2,phase:2,pass,setvar:tx.counted=yes\"\n"
    ));
    for target in &mut g.target_records {
        if target.collection == CollectionKind::Args {
            target.collection = CollectionKind::Xml;
        }
    }
    let mut memory = MatcherCompileWork {
        limits: crate::seclang::CompilerLimits::default(),
        validated_bytes: 0,
        working_bytes: 0,
        compiler_owned_bytes: 0,
        matcher_charged_bytes: 0,
        peak_bytes: 0,
        requirement_cursors: [0; 8],
    };
    let mut work = CanonicalWork::default();
    g.xml_inspection = crate::seclang::compiler::json_inspection_plan::CompiledJsonPlan::compile_domain(
        &g,
        &mut memory,
        &mut work,
        InspectionDomain::Xml,
    )
    .unwrap();
    assert!(g.xml_inspection_unsupported().is_empty());
    assert_eq!(g.xml_inspection.groups.len(), 1);
    let mut tx = transaction(&g, RuleEngineMode::On, RuntimeLimits::default());
    publish(&mut tx, b"<r a='attack' b='attack'>attack</r>").unwrap();
    let outcome = tx.execute_request_body().unwrap();
    assert_eq!(outcome.findings().len(), 2);
    assert_eq!(tx.tx_value(b"selected"), Some(b"XML:a".as_slice()));
    assert_eq!(tx.tx_value(b"counted"), Some(b"yes".as_slice()));
}

#[test]
fn xml_negated_captures_failed_chains_and_late_multimatch_errors_replay_concretely() {
    let negative = "SecRule XML:/* \"!@rx (absent)\" \"id:1,phase:2,pass,capture,setvar:tx.negative=yes\"";
    let failed_chain = concat!(
        "SecRule XML:/* \"@rx (attack)\" \"id:1,phase:2,pass,capture,setvar:tx.leaked=bad,chain\"\n",
        "SecRule XML://@* \"@streq absent\" \"setvar:tx.leaked=also_bad\"\n",
        "SecAction \"id:2,phase:2,pass,setvar:tx.done=yes\"\n"
    );
    let late_error =
        "SecRule XML:/* \"@contains A\" \"id:1,phase:2,pass,multiMatch,t:lowercase,t:hexEncode,setvar:tx.leaked=bad\"";
    for (rules, xml, output_limit, expected_ids, negative_value, done_value, error) in [
        (
            negative,
            b"<r>attack</r>".as_slice(),
            1024,
            vec![1],
            Some(b"yes".as_slice()),
            None,
            None,
        ),
        (negative, b"<r>absent</r>".as_slice(), 1024, vec![], None, None, None),
        (
            failed_chain,
            b"<r a='safe'>attack</r>".as_slice(),
            1024,
            vec![2],
            None,
            Some(b"yes".as_slice()),
            None,
        ),
        (
            late_error,
            b"<r>ABC</r>".as_slice(),
            3,
            vec![],
            None,
            None,
            Some(RuntimeError::Transformation {
                kind: super::super::RuntimeTransformError::OutputLimit,
            }),
        ),
    ] {
        let g = compile(rules);
        for chunk in [0, 1, 3, xml.len()] {
            let mut tx = transaction(
                &g,
                RuleEngineMode::On,
                RuntimeLimits {
                    max_transformation_output_bytes: output_limit,
                    ..RuntimeLimits::default()
                },
            );
            if chunk == 0 {
                publish(&mut tx, xml).unwrap();
            } else {
                let mut session = tx
                    .start_xml_inspection(XmlInspectionLimits::default(), Some(xml.len()))
                    .unwrap();
                for part in xml.chunks(chunk) {
                    session.feed(part).unwrap();
                }
                session.complete(xml.len()).unwrap();
                session.finish(None).unwrap();
            }
            assert_eq!(tx.tx_value(b"negative"), None);
            assert_eq!(tx.tx_value(b"leaked"), None);
            let outcome = tx.execute_request_body();
            if let Some(expected) = error {
                assert_eq!(outcome, Err(expected), "chunk {chunk}");
            } else {
                let outcome = outcome.unwrap();
                assert!(outcome.intervention.is_none());
                assert_eq!(
                    outcome
                        .findings()
                        .iter()
                        .map(|finding| finding.rule_id)
                        .collect::<Vec<_>>(),
                    expected_ids,
                    "chunk {chunk}"
                );
            }
            assert_eq!(tx.tx_value(b"negative"), negative_value);
            assert_eq!(tx.tx_value(b"done"), done_value);
            assert_eq!(tx.tx_value(b"leaked"), None);
            assert_eq!(tx.tx_value(b"0"), None);
            assert_eq!(tx.tx_value(b"1"), None);
        }
    }
}

#[test]
fn preordered_shared_and_distinct_xml_profiles_preserve_target_effect_order() {
    let mut rules = String::from("SecAction \"id:100,phase:2,pass,setvar:tx.count=0\"\n");
    for id in 1..=32 {
        rules.push_str(&format!("SecRule XML:/*|XML://@* \"@pm safe\" \"id:{id},phase:2,pass,t:lowercase,setvar:tx.count=+1,setvar:tx.last=%{{MATCHED_VAR_NAME}}\"\n"));
    }
    rules.push_str(concat!(
        "SecRule XML \"@contains SAFE\" \"id:101,phase:2,pass,t:none,setvar:tx.raw=yes\"\n",
        "SecRule XML \"@rx (safe)\" \"id:102,phase:2,pass,t:lowercase,multiMatch,capture,chain\"\n",
        "SecRule TX:1 \"@streq safe\" \"setvar:tx.capture=%{TX.1}\"\n"
    ));
    let g = compile(&rules);
    assert_eq!(g.xml_inspection.groups.len(), 34);
    assert_eq!(g.xml_inspection.profiles.len(), 3);
    let xml = b"<r a='SAFE'>safe</r>";
    for chunk in [0, 1, 3, xml.len()] {
        let mut tx = transaction(&g, RuleEngineMode::On, RuntimeLimits::default());
        if chunk == 0 {
            publish(&mut tx, xml).unwrap();
        } else {
            let mut session = tx
                .start_xml_inspection(XmlInspectionLimits::default(), Some(xml.len()))
                .unwrap();
            for part in xml.chunks(chunk) {
                session.feed(part).unwrap();
            }
            session.complete(xml.len()).unwrap();
            session.finish(None).unwrap();
        }
        let outcome = tx.execute_request_body().unwrap();
        assert_eq!(outcome.findings().len(), 35);
        assert_eq!(tx.tx_value(b"count"), Some(b"64".as_slice()));
        assert_eq!(tx.tx_value(b"last"), Some(b"XML:a".as_slice()));
        assert_eq!(tx.tx_value(b"raw"), Some(b"yes".as_slice()));
        assert_eq!(tx.tx_value(b"capture"), Some(b"safe".as_slice()));
    }
}
