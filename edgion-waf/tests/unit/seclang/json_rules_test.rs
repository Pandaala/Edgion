use super::*;
use crate::seclang::{
    compile_with_includes, BodyLimits, BodyProcessorSelection, FrontendLimits, IncludeFailure, IncludeRequest,
    IncludeSink, IncludeSource, JsonBodyConfig, JsonDocumentLimits, JsonStreamInput, JsonStreamLimits, PhaseDefaults,
    RequestBody, SemanticValidationContext,
};

struct NoIncludes;
impl IncludeSource for NoIncludes {
    fn load(&mut self, _: IncludeRequest<'_>, _: &mut IncludeSink<'_>) -> Result<(), IncludeFailure> {
        Err(IncludeFailure::NotFound)
    }
}

fn compile(source: &str) -> CompiledGeneration {
    let defaults = [
        Phase::RequestHeaders,
        Phase::RequestBody,
        Phase::ResponseHeaders,
        Phase::ResponseBody,
        Phase::Logging,
    ]
    .map(|phase| PhaseDefaults::pass(phase, true, true));
    compile_with_includes(
        "json-rule-fixture",
        source.as_bytes(),
        &mut NoIncludes,
        None,
        SemanticValidationContext::try_new(Some(Phase::RequestBody), &defaults).unwrap(),
        FrontendLimits::default(),
    )
    .unwrap()
    .into_parts()
    .0
}

fn request(uri: &[u8]) -> RequestHead<'_> {
    RequestHead::new(
        b"GET / HTTP/1.1",
        b"GET",
        uri,
        uri,
        b"HTTP/1.1",
        b"192.0.2.1",
        b"json-rules",
        &[],
    )
}

fn plan_limits() -> JsonPlanLimits {
    JsonPlanLimits {
        max_subscribers: 128,
        max_retained_bytes: 1024 * 1024,
    }
}
fn fact_limits() -> JsonFactLimits {
    JsonFactLimits {
        max_facts: 131_072,
        max_retained_bytes: 32 * 1024 * 1024,
        max_work: 128 * 1024 * 1024,
    }
}
fn body_config(head: usize) -> JsonBodyConfig {
    JsonBodyConfig {
        lexical: JsonStreamLimits::default(),
        document: JsonDocumentLimits {
            max_retained_bytes: 16 * 1024 * 1024,
            max_key_bytes: 1024 * 1024,
            max_members: 65_536,
            max_events: 131_072,
            max_path_bytes: 16 * 1024,
            max_depth: 64,
            max_work: 128 * 1024 * 1024,
        },
        body: BodyLimits::default(),
        head_args_combined_size: head,
        retain_raw_body: None,
        detection_only: false,
    }
}

fn compare(source: &str, body: &[u8], split: usize, query: &[(&[u8], &[u8])], uri: &[u8], limits: RuntimeLimits) {
    let generation = compile(source);
    let plan = JsonSubscriberPlan::for_generation(&generation, plan_limits()).unwrap();
    let mut reference = Transaction::new(&generation, request(uri), limits).unwrap();
    reference.execute_request_headers().unwrap();
    let head = parse_usize(reference.request.first_value(CollectionKind::ArgsCombinedSize).unwrap()).unwrap();
    reference
        .publish_request_body(RequestBody::new(body, BodyProcessorSelection::Json))
        .unwrap();
    let mut scratch_owner = Transaction::new(&generation, request(b"/"), RuntimeLimits::default()).unwrap();
    let mut session = JsonSubscriberSession::new(
        &plan,
        &mut scratch_owner.group_scratch,
        limits,
        EvaluationMode::Accelerated,
        body_config(head),
        fact_limits(),
    );
    for &(name, value) in query {
        session.add_query(name, value).unwrap();
    }
    session.feed(JsonStreamInput::Data(&body[..split])).unwrap();
    session.feed(JsonStreamInput::Data(&body[split..])).unwrap();
    session.feed(JsonStreamInput::Complete).unwrap();
    for index in 0..plan.len() {
        let member = plan.member_id(index).unwrap();
        let unit_id = generation
            .units
            .iter()
            .position(|unit| unit.member_start == member)
            .unwrap();
        let actual = session.replay_rule(index, 0);
        // Each standalone comparison starts from the same nonmatch TX base.
        reference.tx.clear_matches();
        match reference.evaluate_unit(unit_id, None) {
            Ok(matched) => {
                let actual = actual.unwrap();
                assert_eq!(actual.match_count != 0, matched);
                assert_eq!(actual.match_count, reference.tx.matched_vars.len());
                assert_eq!(actual.final_tx_bytes, reference.tx.retained_bytes);
            }
            Err(error) => assert_eq!(actual, Err(JsonExecutionError::Runtime(error))),
        }
    }
}

#[test]
fn subscriber_replay_matches_reference_with_queries_duplicates_and_transforms() {
    let source = r#"
SecRule ARGS "@pm hit" "id:1,phase:2,pass"
SecRule ARGS "!@pm hit" "id:2,phase:2,pass"
SecRule ARGS_NAMES "@pm json q" "id:3,phase:2,pass"
SecRule ARGS:json.a "@pm hit" "id:4,phase:2,pass,t:lowercase,t:lowercase"
SecRule ARGS:json.A "@pm hit" "id:5,phase:2,pass,t:urlDecodeUni,t:none,t:lowercase"
"#;
    for body in [
        b"{}".as_slice(),
        b"[]",
        b"null",
        br#"{"a":"HIT","A":"hit","z":"miss"}"#,
        br#"{"a":"hit","a":{},"b":["hit","miss"]}"#,
        br#"{"a.b":"hit","a":{"b":"miss"},"a.b":"miss"}"#,
        br#"["hit",null,true,1.5,[],{}]"#,
    ] {
        for split in 0..=body.len() {
            compare(
                source,
                body,
                split,
                &[(b"q", b"hit"), (b"q", b"miss")],
                b"/?q=hit&q=miss",
                RuntimeLimits::default(),
            );
        }
    }
}

#[test]
fn ordered_replay_preserves_competing_runtime_limits() {
    let source = "SecRule ARGS \"!@pm miss\" \"id:1,phase:2,pass,t:lowercase\"\n";
    let body = br#"{"z":"long-value","a":"HIT","b":"miss"}"#;
    for max_selected_values in [0, 1, 3, 4, 8] {
        for max_transformation_output_bytes in [0, 3, 16] {
            for max_operator_work_bytes in [0, 8, 160, 4096] {
                for max_operator_evaluations in [0, 1, 4] {
                    for max_tx_variables in [0, 1, 4] {
                        compare(
                            source,
                            body,
                            11,
                            &[(b"q", b"hit")],
                            b"/?q=hit",
                            RuntimeLimits {
                                max_selected_values,
                                max_transformation_output_bytes,
                                max_operator_work_bytes,
                                max_operator_evaluations,
                                max_tx_variables,
                                ..RuntimeLimits::default()
                            },
                        );
                    }
                }
            }
        }
    }
    for max_tx_bytes in 0..256 {
        compare(
            source,
            body,
            0,
            &[],
            b"/",
            RuntimeLimits {
                max_tx_bytes,
                ..RuntimeLimits::default()
            },
        );
    }
}

#[test]
fn overwritten_deferred_errors_disappear_and_selection_precedes_predicates() {
    let source = "SecRule ARGS \"@pm hit\" \"id:1,phase:2,pass\"\n";
    let limits = RuntimeLimits {
        max_transformation_output_bytes: 3,
        ..RuntimeLimits::default()
    };
    compare(source, br#"{"a":"too-long","a":"hit"}"#, 15, &[], b"/", limits);
    compare(
        source,
        br#"{"a":"too-long","b":"hit"}"#,
        15,
        &[],
        b"/",
        RuntimeLimits {
            max_selected_values: 1,
            ..limits
        },
    );
    compare(
        source,
        br#"{"a":"hit"}"#,
        0,
        &[(b"q", b"too-long")],
        b"/?q=too-long",
        limits,
    );
}

#[test]
fn candidate_plan_uses_effective_transforms_and_rejects_unsafe_dependencies() {
    for source in [
        "SecRule ARGS \"@pm hit\" \"id:1,phase:2,pass,t:urlDecodeUni,t:none,t:lowercase\"\n",
        "SecRule ARGS_NAMES:json.a \"!@pm hit\" \"id:1,phase:2,deny,t:lowercase,t:lowercase\"\n",
        "SecRule ARGS \"@pm hit\" \"id:1,phase:2,pass,msg:'static',logdata:'static'\"\n",
    ] {
        let generation = compile(source);
        assert_eq!(
            JsonSubscriberPlan::for_generation(&generation, plan_limits())
                .unwrap()
                .len(),
            1
        );
    }
    for source in [
        "SecRule ARGS \"@pm hit\" \"id:1,phase:2,pass,t:urlDecodeUni\"\n",
        "SecRule ARGS \"@pm hit\" \"id:1,phase:2,pass,capture\"\n",
        "SecRule ARGS \"@pm hit\" \"id:1,phase:2,pass,multiMatch\"\n",
        "SecRule ARGS \"@pm hit\" \"id:1,phase:2,pass,setvar:tx.x=1\"\n",
        "SecRule ARGS \"@pm hit\" \"id:1,phase:2,pass,msg:'%{MATCHED_VAR}'\"\n",
        "SecRule ARGS|ARGS_NAMES \"@pm hit\" \"id:1,phase:2,pass\"\n",
        "SecRule &ARGS \"@pm hit\" \"id:1,phase:2,pass\"\n",
        "SecRule ARGS \"@contains hit\" \"id:1,phase:2,pass\"\n",
        "SecRule ARGS \"@pm hit\" \"id:1,phase:2,pass,chain\"\nSecRule ARGS \"@pm hit\" \"t:none\"\n",
        "SecAction \"id:2,phase:1,pass,ctl:ruleRemoveTargetByTag=tag;ARGS:q\"\nSecRule ARGS \"@pm hit\" \"id:1,phase:2,pass\"\n",
    ] {
        let generation = compile(source);
        assert!(matches!(JsonSubscriberPlan::for_generation(&generation, plan_limits()), Err(JsonExecutionError::Unsupported)), "unexpected candidate: {source}");
    }
}

#[test]
fn multi_rule_facts_are_bounded_and_failure_releases_owned_state() {
    let generation =
        compile("SecRule ARGS \"@pm hit\" \"id:1,phase:2,pass\"\nSecRule ARGS \"@pm other\" \"id:2,phase:2,pass\"\n");
    assert!(JsonSubscriberPlan::for_generation(
        &generation,
        JsonPlanLimits {
            max_subscribers: 1,
            ..plan_limits()
        }
    )
    .is_err());
    assert!(JsonSubscriberPlan::for_generation(
        &generation,
        JsonPlanLimits {
            max_retained_bytes: 0,
            ..plan_limits()
        }
    )
    .is_err());
    let plan = JsonSubscriberPlan::for_generation(&generation, plan_limits()).unwrap();
    for limits in [
        JsonFactLimits {
            max_facts: 0,
            ..fact_limits()
        },
        JsonFactLimits {
            max_retained_bytes: 0,
            ..fact_limits()
        },
        JsonFactLimits {
            max_work: 0,
            ..fact_limits()
        },
    ] {
        let mut owner = Transaction::new(&generation, request(b"/"), RuntimeLimits::default()).unwrap();
        let mut session = JsonSubscriberSession::new(
            &plan,
            &mut owner.group_scratch,
            RuntimeLimits::default(),
            EvaluationMode::Accelerated,
            body_config(0),
            limits,
        );
        let error = session.feed(JsonStreamInput::Data(br#"{"a":"hit"}"#)).unwrap_err();
        assert!(matches!(error, JsonExecutionError::Limit { .. }));
        assert_eq!(session.usage().retained_bytes, 0);
        assert_eq!(session.usage().facts, 0);
        assert!(session.usage().work > 0);
        assert_eq!(session.feed(JsonStreamInput::Complete), Err(error));
        assert!(session.summary().is_none());
    }
}

#[test]
fn replay_pins_literal_match_retention_and_member_pattern_isolation() {
    let generation = compile("SecRule ARGS \"@pm hit\" \"id:1,phase:2,pass\"\nSecRule ARGS \"@pm other\" \"id:2,phase:2,pass\"\nSecRule ARGS_NAMES \"@pm q json\" \"id:3,phase:2,pass\"\n");
    let plan = JsonSubscriberPlan::for_generation(&generation, plan_limits()).unwrap();
    let mut owner = Transaction::new(&generation, request(b"/"), RuntimeLimits::default()).unwrap();
    let mut session = JsonSubscriberSession::new(
        &plan,
        &mut owner.group_scratch,
        RuntimeLimits::default(),
        EvaluationMode::Reference,
        body_config(4),
        fact_limits(),
    );
    session.add_query(b"q", b"hit").unwrap();
    session.feed(JsonStreamInput::Data(br#"{"a":"hit"}"#)).unwrap();
    session.feed(JsonStreamInput::Complete).unwrap();
    let base = 71;
    let args = session.replay_rule(0, base).unwrap();
    assert_eq!(args.selected_count, 2);
    assert_eq!(args.match_count, 2);
    assert_eq!(args.final_tx_bytes, base + 2 * size_of::<TxEntry>() + 9 + 14 + 14);
    let unrelated = session.replay_rule(1, base).unwrap();
    assert_eq!(unrelated.match_count, 0);
    assert_eq!(unrelated.final_tx_bytes, base);
    let names = session.replay_rule(2, base).unwrap();
    assert_eq!(names.final_tx_bytes, base + 2 * size_of::<TxEntry>() + 13 + 23 + 23);
    assert_eq!(
        session.feed(JsonStreamInput::Complete),
        Err(JsonExecutionError::AlreadyComplete)
    );
    assert_eq!(
        session.add_query(b"late", b"value"),
        Err(JsonExecutionError::AlreadyComplete)
    );
    assert_eq!(session.replay_rule(0, base).unwrap(), args);
}

#[test]
fn static_metadata_validation_follows_matching_and_tx_limits() {
    let mut generation = compile("SecRule ARGS \"@pm hit\" \"id:1,phase:2,pass,msg:'long-message'\"\nSecRule ARGS \"@pm other\" \"id:2,phase:2,pass,msg:'long-message'\"\n");
    // Lower the existing compiled runtime cap to exercise the same immutable
    // template and member limits used by validate_runtime_actions.
    for member in &mut generation.members {
        member.runtime_expansion_byte_limit = 3;
    }
    let plan = JsonSubscriberPlan::for_generation(&generation, plan_limits()).unwrap();
    for max_tx_variables in [0, 8] {
        let limits = RuntimeLimits {
            max_tx_variables,
            ..RuntimeLimits::default()
        };
        let mut reference = Transaction::new(&generation, request(b"/"), limits).unwrap();
        reference.execute_request_headers().unwrap();
        reference
            .publish_request_body(RequestBody::new(br#"{"a":"hit"}"#, BodyProcessorSelection::Json))
            .unwrap();
        let mut owner = Transaction::new(&generation, request(b"/"), RuntimeLimits::default()).unwrap();
        let mut session = JsonSubscriberSession::new(
            &plan,
            &mut owner.group_scratch,
            limits,
            EvaluationMode::Accelerated,
            body_config(0),
            fact_limits(),
        );
        session.feed(JsonStreamInput::Data(br#"{"a":"hit"}"#)).unwrap();
        session.feed(JsonStreamInput::Complete).unwrap();
        let error = reference.evaluate_unit(0, None).unwrap_err();
        assert_eq!(session.replay_rule(0, 0), Err(JsonExecutionError::Runtime(error)));
        assert_eq!(reference.evaluate_unit(1, None), Ok(false));
        assert_eq!(session.replay_rule(1, 0).unwrap().match_count, 0);
    }
}

#[test]
fn provisional_duplicate_facts_stay_charged_and_abandonment_releases_them() {
    let generation = compile("SecRule ARGS \"@pm hit\" \"id:1,phase:2,pass\"\n");
    let plan = JsonSubscriberPlan::for_generation(&generation, plan_limits()).unwrap();
    let mut owner = Transaction::new(&generation, request(b"/"), RuntimeLimits::default()).unwrap();
    let mut session = JsonSubscriberSession::new(
        &plan,
        &mut owner.group_scratch,
        RuntimeLimits::default(),
        EvaluationMode::Accelerated,
        body_config(0),
        fact_limits(),
    );
    session
        .feed(JsonStreamInput::Data(br#"{"a":"hit","a":"miss"}"#))
        .unwrap();
    assert_eq!(session.usage().facts, 2);
    assert!(session.usage().retained_bytes >= 2 * size_of::<Fact>());
    assert_eq!(session.replay_rule(0, 0), Err(JsonExecutionError::State));
    let error = session.feed(JsonStreamInput::Abandoned).unwrap_err();
    assert_eq!(
        error,
        JsonExecutionError::Body(crate::seclang::JsonBodyError::Abandoned)
    );
    assert_eq!(session.usage().facts, 0);
    assert_eq!(session.usage().retained_bytes, 0);
    assert_eq!(session.feed(JsonStreamInput::Complete), Err(error));

    let limits = JsonFactLimits {
        max_facts: 1,
        ..fact_limits()
    };
    let mut session = JsonSubscriberSession::new(
        &plan,
        &mut owner.group_scratch,
        RuntimeLimits::default(),
        EvaluationMode::Accelerated,
        body_config(0),
        limits,
    );
    assert!(matches!(
        session.feed(JsonStreamInput::Data(br#"{"a":"hit","a":"miss"}"#)),
        Err(JsonExecutionError::Limit {
            kind: JsonExecutionLimit::Facts,
            limit: 1
        })
    ));
}

#[test]
fn ordinary_tx_base_and_nonmatching_units_preserve_committed_match_charge() {
    let generation =
        compile("SecRule ARGS \"@pm hit\" \"id:1,phase:2,pass\"\nSecRule ARGS \"@pm miss\" \"id:2,phase:2,pass\"\n");
    let plan = JsonSubscriberPlan::for_generation(&generation, plan_limits()).unwrap();
    let limits = RuntimeLimits::default();
    let mut reference = Transaction::new(&generation, request(b"/"), limits).unwrap();
    reference
        .tx
        .set(b"ordinary".to_vec(), b"retained".to_vec(), limits)
        .unwrap();
    let base = reference.tx.retained_bytes;
    reference.execute_request_headers().unwrap();
    reference
        .publish_request_body(RequestBody::new(br#"{"a":"hit"}"#, BodyProcessorSelection::Json))
        .unwrap();
    let mut owner = Transaction::new(&generation, request(b"/"), limits).unwrap();
    let mut session = JsonSubscriberSession::new(
        &plan,
        &mut owner.group_scratch,
        limits,
        EvaluationMode::Accelerated,
        body_config(0),
        fact_limits(),
    );
    session.feed(JsonStreamInput::Data(br#"{"a":"hit"}"#)).unwrap();
    session.feed(JsonStreamInput::Complete).unwrap();
    assert!(reference.evaluate_unit(0, None).unwrap());
    let committed = reference.tx.retained_bytes;
    assert_eq!(session.replay_rule(0, base).unwrap().final_tx_bytes, committed);
    assert!(!reference.evaluate_unit(1, None).unwrap());
    let nonmatch = session.replay_rule(1, base).unwrap();
    assert_eq!(nonmatch.match_count, 0);
    assert_eq!(nonmatch.final_tx_bytes, base);
    assert_eq!(reference.tx.retained_bytes, committed);
    assert!(committed > base);
}

#[test]
fn fact_capacity_limit_has_an_exact_two_record_boundary() {
    let generation = compile("SecRule ARGS \"@pm hit\" \"id:1,phase:2,pass\"\n");
    let plan = JsonSubscriberPlan::for_generation(&generation, plan_limits()).unwrap();
    let bytes = 2 * size_of::<Fact>();
    for cap in [bytes - 1, bytes] {
        let mut owner = Transaction::new(&generation, request(b"/"), RuntimeLimits::default()).unwrap();
        let mut session = JsonSubscriberSession::new(
            &plan,
            &mut owner.group_scratch,
            RuntimeLimits::default(),
            EvaluationMode::Accelerated,
            body_config(0),
            JsonFactLimits {
                max_retained_bytes: cap,
                ..fact_limits()
            },
        );
        let result = session.feed(JsonStreamInput::Data(br#"{"a":"hit","b":"miss"}"#));
        if cap == bytes {
            result.unwrap();
            session.feed(JsonStreamInput::Complete).unwrap();
            assert_eq!(session.usage().facts, 2);
            assert_eq!(session.usage().retained_bytes, bytes);
        } else {
            assert!(matches!(
                result,
                Err(JsonExecutionError::Limit {
                    kind: JsonExecutionLimit::RetainedBytes,
                    ..
                })
            ));
            assert_eq!(session.usage().retained_bytes, 0);
        }
    }
}

#[test]
fn sparse_ranges_preserve_subscriber_and_query_boundaries() {
    let source = "SecRule ARGS:q \"@pm hit\" \"id:1,phase:2,pass\"\nSecRule ARGS:json.a \"@pm hit\" \"id:2,phase:2,pass\"\nSecRule ARGS_NAMES:r \"@pm r\" \"id:3,phase:2,pass\"\n";
    let body = br#"{"unused":1,"a":"hit","a":"miss","b":null}"#;
    for split in 0..=body.len() {
        compare(
            source,
            body,
            split,
            &[(b"unused", b"hit"), (b"q", b"hit"), (b"r", b"value"), (b"q", b"miss")],
            b"/?unused=hit&q=hit&r=value&q=miss",
            RuntimeLimits::default(),
        );
    }
}

#[test]
fn cached_plan_admission_uses_exact_shared_capacity_and_stable_identity() {
    let generation = compile(concat!(
        "SecRule ARGS \"@pm hit\" \"id:1,phase:2,pass\"\n",
        "SecRule ARGS_NAMES \"!@pm miss\" \"id:2,phase:2,pass\"\n",
    ));
    let shared = generation.json_capability.subscribers().unwrap();
    let bytes = generation.json_capability.retained_bytes();
    assert!(bytes >= std::mem::size_of_val(shared));
    let limits = JsonPlanLimits {
        max_subscribers: shared.len(),
        max_retained_bytes: bytes,
    };
    for _ in 0..3 {
        let plan = JsonSubscriberPlan::for_generation(&generation, limits).unwrap();
        assert_eq!(plan.subscribers.as_ptr(), shared.as_ptr());
        assert_eq!(plan.len(), shared.len());
    }
    assert!(matches!(
        JsonSubscriberPlan::for_generation(
            &generation,
            JsonPlanLimits {
                max_subscribers: shared.len() - 1,
                ..limits
            }
        ),
        Err(JsonExecutionError::Limit {
            kind: JsonExecutionLimit::Subscribers,
            ..
        })
    ));
    assert!(matches!(
        JsonSubscriberPlan::for_generation(
            &generation,
            JsonPlanLimits {
                max_retained_bytes: bytes - 1,
                ..limits
            }
        ),
        Err(JsonExecutionError::Limit {
            kind: JsonExecutionLimit::RetainedBytes,
            ..
        })
    ));
}

#[test]
fn cached_plan_distinguishes_empty_and_unsupported_before_request_caps() {
    let zero = JsonPlanLimits {
        max_subscribers: 0,
        max_retained_bytes: 0,
    };
    let empty = compile("SecRule REQUEST_METHOD \"@streq GET\" \"id:1,phase:1,pass\"\n");
    assert_eq!(JsonSubscriberPlan::for_generation(&empty, zero).unwrap().len(), 0);
    let late_unsupported = compile(concat!(
        "SecRule ARGS \"@pm hit\" \"id:1,phase:2,pass\"\n",
        "SecRule ARGS \"@contains hit\" \"id:2,phase:2,pass\"\n",
    ));
    assert!(matches!(
        JsonSubscriberPlan::for_generation(&late_unsupported, zero),
        Err(JsonExecutionError::Unsupported)
    ));
}

#[test]
fn mixed_plan_counts_only_json_subscribers_and_rejects_body_dependent_chains() {
    let generation = compile(concat!(
        "SecAction \"id:1,phase:2,pass,setvar:tx.seed=1\"\n",
        "SecRule REQUEST_METHOD \"@streq GET\" \"id:2,phase:2,pass,chain\"\n",
        "SecRule TX:seed \"@eq 1\" \"setvar:tx.seed=2\"\n",
        "SecRule ARGS \"@pm hit\" \"id:3,phase:2,pass\"\n",
        "SecRule REQUEST_METHOD \"@streq GET\" \"id:4,phase:2,pass\"\n",
        "SecRule ARGS_NAMES \"@pm json\" \"id:5,phase:2,pass\"\n",
    ));
    let plan = JsonSubscriberPlan::for_generation(
        &generation,
        JsonPlanLimits {
            max_subscribers: 2,
            max_retained_bytes: generation.json_capability.retained_bytes(),
        },
    )
    .unwrap();
    assert_eq!(plan.len(), 2);
    assert_eq!(plan.member_id(0), Some(3));
    assert_eq!(plan.member_id(1), Some(5));
    for source in [
        "SecRule REQUEST_METHOD \"@streq GET\" \"id:1,phase:2,pass,chain\"\nSecRule ARGS \"@pm hit\" \"t:none\"\n",
        "SecRule ARGS \"@pm hit\" \"id:1,phase:2,pass,chain\"\nSecRule REQUEST_METHOD \"@streq GET\" \"t:none\"\n",
        "SecRule REQUEST_METHOD \"@streq GET\" \"id:1,phase:2,pass,setvar:tx.value=%{ARGS.a}\"\n",
        "SecAction \"id:1,phase:2,pass,ctl:ruleRemoveByTag=tag\"\n",
    ] {
        let generation = compile(source);
        assert!(matches!(
            JsonSubscriberPlan::for_generation(&generation, plan_limits()),
            Err(JsonExecutionError::Unsupported)
        ));
    }
}

#[test]
fn json_plan_rejects_body_macros_in_every_post_header_template_position() {
    for phase in 2..=5 {
        for (operator, action) in [
            ("@streq %{ARGS.q}", "pass"),
            ("@streq GET", "pass,msg:'%{ARGS.q}'"),
            ("@streq GET", "pass,logdata:'%{ARGS.q}'"),
            ("@streq GET", "pass,setvar:tx.copy=%{ARGS.q}"),
            ("@streq GET", "pass,setvar:tx.key_%{ARGS.q}=yes"),
        ] {
            let source = format!("SecRule REQUEST_METHOD \"{operator}\" \"id:1,phase:{phase},{action}\"\n");
            let generation = compile(&source);
            assert!(
                matches!(
                    JsonSubscriberPlan::for_generation(&generation, plan_limits()),
                    Err(JsonExecutionError::Unsupported)
                ),
                "phase {phase}: {action}"
            );
        }
        let source = format!(
            concat!(
                "SecRule REQUEST_METHOD \"@streq GET\" \"id:1,phase:{},pass,chain\"\n",
                "SecRule REQUEST_METHOD \"@streq GET\" \"setvar:tx.child=%{{ARGS.q}}\"\n",
            ),
            phase
        );
        let generation = compile(&source);
        assert!(matches!(
            JsonSubscriberPlan::for_generation(&generation, plan_limits()),
            Err(JsonExecutionError::Unsupported)
        ));
    }
}

#[test]
fn json_plan_allows_ordinary_macros_but_requires_static_subscriber_metadata() {
    let generation = compile(concat!(
        "SecAction \"id:1,phase:1,pass,setvar:tx.seed=%{ARGS.q}\"\n",
        "SecRule TX:seed \"@streq %{TX.seed}\" \"id:2,phase:2,pass,msg:'%{MATCHED_VAR}'\"\n",
        "SecRule ARGS \"@pm hit\" \"id:3,phase:2,pass,msg:'static'\"\n",
        "SecRule TX:seed \"@streq %{TX.seed}\" \"id:4,phase:5,pass,logdata:'%{MATCHED_VAR_NAME}'\"\n",
    ));
    assert_eq!(
        JsonSubscriberPlan::for_generation(&generation, plan_limits())
            .unwrap()
            .len(),
        1
    );
    for source in [
        "TX.seed",
        "REQUEST_HEADERS.X-Seed",
        "MATCHED_VAR",
        "MATCHED_VAR_NAME",
        "REMOTE_ADDR",
        "REQUEST_LINE",
        "ARGS.q",
    ] {
        for action in ["msg", "logdata"] {
            let source = format!("SecRule ARGS \"@pm hit\" \"id:1,phase:2,pass,{action}:'%{{{source}}}'\"\n");
            let generation = compile(&source);
            assert!(matches!(
                JsonSubscriberPlan::for_generation(&generation, plan_limits()),
                Err(JsonExecutionError::Unsupported)
            ));
        }
    }
}

fn compare_scalar_reuse<const QUERY: bool>(
    generation: &CompiledGeneration,
    runtime: RuntimeLimits,
    mode: EvaluationMode,
    caps: JsonFactLimits,
) -> (usize, usize) {
    let plan = JsonSubscriberPlan::for_generation(generation, plan_limits()).unwrap();
    let mut cached_owner = Transaction::new(generation, request(b"/"), runtime).unwrap();
    let mut ordinary_owner = Transaction::new(generation, request(b"/"), runtime).unwrap();
    let mut cached = Facts {
        values: Vec::new(),
        query_headers: Vec::new(),
        limits: caps,
        work: 0,
    };
    let mut ordinary = Facts {
        values: Vec::new(),
        query_headers: Vec::new(),
        limits: caps,
        work: 0,
    };
    // Repeated names mimic overwritten values. Different successive scalars must
    // never inherit a prior scalar's match bits or deferred transform result.
    for (name, value) in [
        (b"json.a".as_slice(), b"HIT".as_slice()),
        (b"json.a", b"miss"),
        (b"json.b", b"OTHER"),
        (b"json.a", b""),
    ] {
        let scalar = || {
            if QUERY {
                ScalarInput::Query(name, value)
            } else {
                ScalarInput::Body(name, value)
            }
        };
        let actual = cached
            .evaluate_with_reuse::<true>(plan.view(), &mut cached_owner.group_scratch, runtime, mode, scalar())
            .map(|range| (range.start, range.end));
        let expected = ordinary
            .evaluate_with_reuse::<false>(plan.view(), &mut ordinary_owner.group_scratch, runtime, mode, scalar())
            .map(|range| (range.start, range.end));
        assert_eq!(actual, expected);
        assert_eq!(cached.work, ordinary.work);
        assert_eq!(cached.values.capacity(), ordinary.values.capacity());
        assert_eq!(cached.query_headers.capacity(), ordinary.query_headers.capacity());
        assert_eq!(
            cached
                .query_headers
                .iter()
                .map(|h| (h.end, h.path_bytes, h.value_bytes))
                .collect::<Vec<_>>(),
            ordinary
                .query_headers
                .iter()
                .map(|h| (h.end, h.path_bytes, h.value_bytes))
                .collect::<Vec<_>>(),
        );
        let facts = |values: &[Fact]| {
            values
                .iter()
                .map(|fact| (fact.subscriber, fact.transformed(), fact.positive()))
                .collect::<Vec<_>>()
        };
        assert_eq!(facts(&cached.values), facts(&ordinary.values));
        if actual.is_err() {
            break;
        }
    }
    (cached.work, cached.values.len())
}

#[test]
fn scalar_group_reuse_preserves_facts_and_every_work_boundary() {
    let source = r#"
SecRule ARGS "@pm hit alpha beta" "id:1,phase:2,pass,t:lowercase"
SecRule ARGS "!@pm other" "id:2,phase:2,pass,t:lowercase"
SecRule ARGS:json.never "@pm hit" "id:3,phase:2,pass,t:lowercase"
SecRule ARGS "@pm miss" "id:4,phase:2,pass,t:lowercase"
SecRule ARGS_NAMES "@pm json.a" "id:5,phase:2,pass,t:lowercase"
SecRule ARGS "@pm other" "id:6,phase:2,pass,t:lowercase"
SecRule ARGS:json.a "@pm hit" "id:7,phase:2,pass"
SecRule ARGS:json.a "!@pm miss" "id:8,phase:2,pass"
SecRule ARGS "@pm alpha" "id:9,phase:2,pass,t:none,t:lowercase"
SecRule ARGS:json.a "@pm HIT" "id:10,phase:2,pass,t:lowercase,t:none"
"#;
    let generation = compile(source);
    let body = br#"{"a":"HIT","a":"miss","b":"OTHER","nested":["HIT","miss"]}"#;
    for split in 0..=body.len() {
        compare(source, body, split, &[], b"/", RuntimeLimits::default());
    }
    let group = |member: usize| match generation.operator_plans[member].unwrap().operand {
        CompiledOperatorOperand::Matcher {
            group, pattern_start, ..
        } => (group, pattern_start),
        _ => panic!("fixture must use phrase matcher operands"),
    };
    assert_eq!(group(0).0, group(1).0);
    assert_ne!(group(0).1, group(1).1);
    assert_eq!(group(0).0, group(3).0);
    assert_ne!(group(0).0, group(4).0);
    assert_eq!(group(0).0, group(5).0);
    assert_eq!(group(0).0, group(8).0);
    assert_eq!(group(6).0, group(9).0);
    for mode in [EvaluationMode::Accelerated, EvaluationMode::Reference] {
        for runtime in [
            RuntimeLimits::default(),
            RuntimeLimits {
                max_transformation_output_bytes: 2,
                ..RuntimeLimits::default()
            },
            RuntimeLimits {
                max_transformation_work_bytes: 2,
                ..RuntimeLimits::default()
            },
        ] {
            let (work, count) = compare_scalar_reuse::<false>(&generation, runtime, mode, fact_limits());
            assert!(count > 0);
            // Exhaustively compare the first failure at every exact work budget.
            for max_work in 0..=work {
                compare_scalar_reuse::<false>(
                    &generation,
                    runtime,
                    mode,
                    JsonFactLimits {
                        max_work,
                        ..fact_limits()
                    },
                );
            }
            for max_facts in 0..=count {
                compare_scalar_reuse::<false>(
                    &generation,
                    runtime,
                    mode,
                    JsonFactLimits {
                        max_facts,
                        ..fact_limits()
                    },
                );
            }
            for max_retained_bytes in [0, size_of::<Fact>() - 1, size_of::<Fact>(), 8 * size_of::<Fact>()] {
                compare_scalar_reuse::<false>(
                    &generation,
                    runtime,
                    mode,
                    JsonFactLimits {
                        max_retained_bytes,
                        ..fact_limits()
                    },
                );
            }
        }
    }
}

#[test]
fn compact_fact_layout_and_deferred_outcomes_are_explicit() {
    // Exact pre-compaction field types, solely for measuring the old layout.
    struct PreviousFactLayout {
        _range_end: usize,
        _subscriber: usize,
        _input_bytes: usize,
        _transformed: Result<usize, RuntimeError>,
        _positive: bool,
        _name_bytes: usize,
    }
    println!(
        "fact layout: previous={} compact={} header={} word={}",
        size_of::<PreviousFactLayout>(),
        size_of::<Fact>(),
        size_of::<QueryHeader>(),
        size_of::<usize>()
    );
    assert!(size_of::<Fact>() < size_of::<PreviousFactLayout>());
    assert!(size_of::<Fact>() <= 3 * size_of::<usize>());
    assert_eq!(size_of::<QueryHeader>(), 3 * size_of::<usize>());
    for (outcome, expected) in [
        (FactOutcome::Positive, Ok(true)),
        (FactOutcome::Negative, Ok(false)),
        (
            FactOutcome::OutputLimit,
            Err(RuntimeError::Transformation {
                kind: RuntimeTransformError::OutputLimit,
            }),
        ),
        (
            FactOutcome::WorkLimit,
            Err(RuntimeError::Transformation {
                kind: RuntimeTransformError::WorkLimit,
            }),
        ),
    ] {
        for transformed_bytes in [0, 3] {
            let fact = Fact {
                subscriber: 0,
                transformed_bytes,
                outcome,
            };
            assert_eq!(fact.positive(), expected.unwrap_or(false));
            assert_eq!(fact.transformed(), expected.map(|_| transformed_bytes));
            for (collection, expected_input, expected_name) in
                [(CollectionKind::Args, 7, 11), (CollectionKind::ArgsNames, 6, 17)]
            {
                let replay = ReplayFact {
                    fact: &fact,
                    collection,
                    path_bytes: 6,
                    value_bytes: 7,
                };
                assert_eq!(replay.input_bytes(), expected_input);
                assert_eq!(replay.name_bytes().unwrap(), expected_name);
            }
        }
    }
}

#[test]
#[cfg(target_pointer_width = "64")]
fn compact_fact_literal_byte_and_growth_work_boundaries() {
    let generation = compile(concat!(
        "SecRule ARGS \"@pm hit\" \"id:1,phase:2,pass\"\n",
        "SecRule ARGS \"@pm hit\" \"id:2,phase:2,pass\"\n",
    ));
    let plan = JsonSubscriberPlan::for_generation(&generation, plan_limits()).unwrap();
    assert_eq!(size_of::<Fact>(), 24);
    // Per subscriber: name selection 7 + input copying 6 + group scan 12 = 25.
    // The second body record adds 24 bytes of growth-copy work: total 74.
    // S17 required 96 retained bytes and 98 work for the same body facts.
    for (bytes, work, error, records, attempted_work) in [
        (48, 74, None, 2, 74),
        (
            47,
            74,
            Some(JsonExecutionError::Limit {
                kind: JsonExecutionLimit::RetainedBytes,
                limit: 47,
            }),
            1,
            62,
        ),
        (
            48,
            73,
            Some(JsonExecutionError::Limit {
                kind: JsonExecutionLimit::Work,
                limit: 73,
            }),
            1,
            74,
        ),
    ] {
        let mut owner = Transaction::new(&generation, request(b"/"), RuntimeLimits::default()).unwrap();
        let mut facts = Facts {
            values: Vec::new(),
            query_headers: Vec::new(),
            limits: JsonFactLimits {
                max_facts: 2,
                max_retained_bytes: bytes,
                max_work: work,
            },
            work: 0,
        };
        let result = facts.evaluate(
            plan.view(),
            &mut owner.group_scratch,
            RuntimeLimits::default(),
            EvaluationMode::Accelerated,
            b"json.a",
            b"hit",
        );
        assert_eq!(result.err(), error);
        assert_eq!(facts.values.len(), records);
        assert_eq!(facts.work, attempted_work);
        assert!(facts.values.capacity() * size_of::<Fact>() <= bytes);
    }
}

#[test]
fn compact_deferred_errors_keep_selection_work_and_negation_precedence() {
    let source = "SecRule ARGS \"!@pm miss\" \"id:1,phase:2,pass,t:lowercase\"\n";
    let body = br#"{"x":"HIT","y":""}"#;
    for output_limit in [0, 1024] {
        for selected_limit in [0, 1, 2] {
            let runtime = RuntimeLimits {
                max_transformation_output_bytes: output_limit,
                max_transformation_work_bytes: 0,
                max_selected_values: selected_limit,
                max_operator_work_bytes: 0,
                ..RuntimeLimits::default()
            };
            compare(source, body, 7, &[], b"/", runtime);
        }
    }
}

#[test]
#[cfg(target_pointer_width = "64")]
fn shared_query_headers_fit_exact_aggregate_caps_and_cleanup_failures() {
    let generation = compile("SecRule ARGS \"@pm hit\" \"id:1,phase:2,pass\"\n");
    let plan = JsonSubscriberPlan::for_generation(&generation, plan_limits()).unwrap();
    assert_eq!(size_of::<Fact>(), 24);
    assert_eq!(size_of::<QueryHeader>(), 24);
    // Three single-subscriber queries need three facts and three headers.
    // Each query costs 21 work; growth copies both arenas at lengths 1 and 2.
    // This is the same 144 bytes / 207 work as S17's three 48-byte query facts.
    for (bytes, work, expected) in [
        (144, 207, None),
        (
            143,
            207,
            Some(JsonExecutionError::Limit {
                kind: JsonExecutionLimit::RetainedBytes,
                limit: 143,
            }),
        ),
        (
            144,
            206,
            Some(JsonExecutionError::Limit {
                kind: JsonExecutionLimit::Work,
                limit: 206,
            }),
        ),
    ] {
        let mut owner = Transaction::new(&generation, request(b"/"), RuntimeLimits::default()).unwrap();
        let mut session = JsonSubscriberSession::new(
            &plan,
            &mut owner.group_scratch,
            RuntimeLimits::default(),
            EvaluationMode::Accelerated,
            body_config(0),
            JsonFactLimits {
                max_facts: 8,
                max_retained_bytes: bytes,
                max_work: work,
            },
        );
        session.add_query(b"json.a", b"hit").unwrap();
        session.add_query(b"json.a", b"hit").unwrap();
        let result = session.add_query(b"json.a", b"hit");
        assert_eq!(result.err(), expected);
        if let Some(error) = expected {
            assert_eq!(session.usage().facts, 0);
            assert_eq!(session.usage().retained_bytes, 0);
            assert_eq!(session.core.facts.values.capacity(), 0);
            assert_eq!(session.core.facts.query_headers.capacity(), 0);
            assert_eq!(session.feed(JsonStreamInput::Complete), Err(error));
        } else {
            assert_eq!(session.usage().facts, 3);
            assert_eq!(session.usage().retained_bytes, 144);
            assert_eq!(session.usage().work, 207);
            assert_eq!(session.core.facts.query_headers.len(), 3);
            assert_eq!(session.core.facts.query_headers[2].end, 3);
        }
    }
}

#[test]
fn sparse_query_headers_and_body_event_lengths_preserve_both_collections() {
    let source = concat!(
        "SecRule ARGS:q \"@pm hit\" \"id:1,phase:2,pass\"\n",
        "SecRule ARGS_NAMES:q \"@pm q\" \"id:2,phase:2,pass\"\n",
        "SecRule ARGS:json.q \"@pm hit\" \"id:3,phase:2,pass\"\n",
    );
    let query: &[(&[u8], &[u8])] = &[(b"skip", b"unused"), (b"q", b"hit"), (b"q", b"")];
    let body = br#"{"q":"hit","unused":"hit"}"#;
    compare(
        source,
        body,
        11,
        query,
        b"/?skip=unused&q=hit&q=",
        RuntimeLimits::default(),
    );
    let generation = compile(source);
    let plan = JsonSubscriberPlan::for_generation(&generation, plan_limits()).unwrap();
    let mut owner = Transaction::new(&generation, request(b"/"), RuntimeLimits::default()).unwrap();
    let mut session = JsonSubscriberSession::new(
        &plan,
        &mut owner.group_scratch,
        RuntimeLimits::default(),
        EvaluationMode::Accelerated,
        body_config(0),
        fact_limits(),
    );
    session.add_query(query[0].0, query[0].1).unwrap();
    assert_eq!(session.usage().facts, 0);
    assert_eq!(session.usage().retained_bytes, 0);
    for &(name, value) in &query[1..] {
        session.add_query(name, value).unwrap();
    }
    assert_eq!(session.core.facts.query_headers.len(), 2);
    assert_eq!(session.core.facts.query_headers[0].end, 2);
    assert_eq!(session.core.facts.query_headers[1].end, 4);
    session.feed(JsonStreamInput::Data(body)).unwrap();
    session.feed(JsonStreamInput::Complete).unwrap();
    assert_eq!(session.usage().facts, 5);
    assert_eq!(session.replay_rule(0, 0).unwrap().match_count, 1);
    assert_eq!(session.replay_rule(1, 0).unwrap().match_count, 2);
    assert_eq!(session.replay_rule(2, 0).unwrap().match_count, 1);
}

#[test]
fn query_header_reservation_failure_does_not_leak_partial_arena_capacity() {
    let generation = compile("SecRule ARGS \"@pm hit\" \"id:1,phase:2,pass\"\n");
    let plan = JsonSubscriberPlan::for_generation(&generation, plan_limits()).unwrap();
    let mut owner = Transaction::new(&generation, request(b"/"), RuntimeLimits::default()).unwrap();
    let cap = size_of::<Fact>() + size_of::<QueryHeader>() - 1;
    let mut session = JsonSubscriberSession::new(
        &plan,
        &mut owner.group_scratch,
        RuntimeLimits::default(),
        EvaluationMode::Accelerated,
        body_config(0),
        JsonFactLimits {
            max_retained_bytes: cap,
            ..fact_limits()
        },
    );
    assert_eq!(
        session.add_query(b"q", b"hit"),
        Err(JsonExecutionError::Limit {
            kind: JsonExecutionLimit::RetainedBytes,
            limit: cap,
        })
    );
    assert_eq!(session.usage().retained_bytes, 0);
    assert_eq!(session.core.facts.query_headers.capacity(), 0);
    assert_eq!(session.core.facts.values.capacity(), 0);
}

#[test]
fn query_metadata_reuse_oracle_covers_aggregate_growth_boundaries() {
    let generation = compile(concat!(
        "SecRule ARGS \"@pm hit\" \"id:1,phase:2,pass,t:lowercase\"\n",
        "SecRule ARGS \"!@pm miss\" \"id:2,phase:2,pass,t:lowercase\"\n",
    ));
    let runtime = RuntimeLimits::default();
    let (work, _) = compare_scalar_reuse::<true>(&generation, runtime, EvaluationMode::Accelerated, fact_limits());
    for cap in 0..=work {
        compare_scalar_reuse::<true>(
            &generation,
            runtime,
            EvaluationMode::Accelerated,
            JsonFactLimits {
                max_work: cap,
                ..fact_limits()
            },
        );
    }
    for cap in 0..=12 * size_of::<Fact>() {
        compare_scalar_reuse::<true>(
            &generation,
            runtime,
            EvaluationMode::Reference,
            JsonFactLimits {
                max_retained_bytes: cap,
                ..fact_limits()
            },
        );
    }
}

#[test]
#[cfg(target_pointer_width = "64")]
fn single_subscriber_query_capacity_and_copy_work_match_previous_layout() {
    let generation = compile("SecRule ARGS \"@pm hit\" \"id:1,phase:2,pass\"\n");
    let plan = JsonSubscriberPlan::for_generation(&generation, plan_limits()).unwrap();
    let mut owner = Transaction::new(&generation, request(b"/"), RuntimeLimits::default()).unwrap();
    let mut session = JsonSubscriberSession::new(
        &plan,
        &mut owner.group_scratch,
        RuntimeLimits::default(),
        EvaluationMode::Accelerated,
        body_config(0),
        fact_limits(),
    );
    for count in 1usize..=65 {
        session.add_query(b"q", b"hit").unwrap();
        let capacity = count.next_power_of_two();
        // S17 used one 48-byte record per selected query. The two 24-byte
        // arenas match its capacity-byte and growth-copy budgets at all cliffs.
        // Their allocation count and fixed owner size are intentionally not equal.
        assert_eq!(session.usage().retained_bytes, capacity * 48);
        assert_eq!(session.usage().work, count * 16 + (capacity - 1) * 48);
    }
}

#[test]
fn abandonment_releases_populated_query_and_body_fact_arenas() {
    let generation = compile("SecRule ARGS \"@pm hit\" \"id:1,phase:2,pass\"\n");
    let plan = JsonSubscriberPlan::for_generation(&generation, plan_limits()).unwrap();
    let mut owner = Transaction::new(&generation, request(b"/"), RuntimeLimits::default()).unwrap();
    let mut session = JsonSubscriberSession::new(
        &plan,
        &mut owner.group_scratch,
        RuntimeLimits::default(),
        EvaluationMode::Accelerated,
        body_config(4),
        fact_limits(),
    );
    session.add_query(b"q", b"hit").unwrap();
    session.feed(JsonStreamInput::Data(br#"{"a":"hit"}"#)).unwrap();
    assert_eq!(session.usage().facts, 2);
    assert_eq!(session.core.facts.query_headers.len(), 1);
    assert!(session.core.facts.query_headers.capacity() > 0);
    assert!(session.core.facts.values.capacity() > 0);
    let error = JsonExecutionError::Body(JsonBodyError::Abandoned);
    assert_eq!(session.feed(JsonStreamInput::Abandoned), Err(error));
    assert_eq!(session.core.facts.query_headers.capacity(), 0);
    assert_eq!(session.core.facts.values.capacity(), 0);
    assert_eq!(session.usage().retained_bytes, 0);
    assert_eq!(session.feed(JsonStreamInput::Complete), Err(error));
}
