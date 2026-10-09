use super::super::json_inspection::{OutcomeValue, SelectedOutcome};
use super::*;
use crate::seclang::{
    analyze_one_semantic_draft, compile_validated, parse_syntax, scan_source, validate_semantic_draft, CompilerLimits,
    JsonChecker, JsonStreamInput, JsonStreamLimits, JsonStructureLimits, PhaseDefaults, SemanticLimits,
    SemanticValidationContext, SourceLimits, SyntaxLimits, ValidationLimits,
};

fn compile(text: &str) -> CompiledGeneration {
    let source = scan_source("collector-test", text.as_bytes(), SourceLimits::default()).unwrap();
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
fn request() -> RequestHead<'static> {
    RequestHead::new(
        b"POST / HTTP/1.1",
        b"POST",
        b"/",
        b"/",
        b"HTTP/1.1",
        b"192.0.2.1",
        b"collector",
        &[],
    )
}
fn collector<'a, 'g>(
    generation: &'g CompiledGeneration,
    transaction: &'a mut Transaction<'g>,
    runtime: RuntimeLimits,
    mode: EvaluationMode,
    limits: ResultLimits,
) -> JsonCollector<'a, 'g> {
    JsonCollector::new(
        GenerationHandle::Borrowed(generation),
        SingleInputContext {
            generation,
            request: &transaction.request,
            tx: &transaction.tx,
            group_scratch: &mut transaction.group_scratch,
            boolean_scratch: &mut transaction.boolean_scratch,
            capture_scratch: &mut transaction.capture_scratch,
            scan_cache: None,
            json_budget: None,
        },
        runtime,
        mode,
        limits,
    )
    .unwrap()
}
fn feed(collector: &mut JsonCollector<'_, '_>, bytes: &[u8], split: usize) {
    let mut checker =
        JsonChecker::<Receipt>::new("json", JsonStreamLimits::default(), JsonStructureLimits::default()).unwrap();
    checker.feed(JsonStreamInput::Data(&bytes[..split]), collector).unwrap();
    checker.feed(JsonStreamInput::Data(&bytes[split..]), collector).unwrap();
    checker.feed(JsonStreamInput::Complete, collector).unwrap();
}

#[test]
fn current_name_selector_cache_reuses_successes_with_original_work_charges() {
    let g = compile(concat!(
        "SecRule ARGS:/^json[.]probe$/ \"@contains hit\" \"id:901,phase:2,pass\"\n",
        "SecRule ARGS_NAMES:/^json[.]probe$/ \"@contains probe\" \"id:902,phase:2,pass\"\n",
    ));
    let selector = g.json_inspection.candidate_regexes[0];
    for name in [b"json.probe".as_slice(), b"json.other", b""] {
        let mut usages = Vec::new();
        for mode in [EvaluationMode::Accelerated, EvaluationMode::Reference] {
            let runtime = RuntimeLimits::default();
            let mut tx = Transaction::new(&g, request(), runtime).unwrap();
            let mut sink = collector(&g, &mut tx, runtime, mode, ResultLimits::default());
            let before = sink.usage().work;
            let scope = measurements::start();
            let mut cache = NameSelectorCache::default();
            let first = match_name(
                &g,
                sink.context.boolean_scratch,
                selector,
                name,
                &mut sink.state.store,
                &mut cache,
                mode,
            )
            .unwrap();
            let second = match_name(
                &g,
                sink.context.boolean_scratch,
                selector,
                name,
                &mut sink.state.store,
                &mut cache,
                mode,
            )
            .unwrap();
            assert_eq!(first, second);
            assert_eq!(
                scope.counts().get(Event::NameSelectorSearches),
                if mode == EvaluationMode::Accelerated { 1 } else { 2 }
            );
            assert_eq!(first, name == b"json.probe");
            usages.push(sink.usage().work - before);
        }
        assert_eq!(usages[0], usages[1]);
    }
}

#[test]
fn current_name_selector_cache_has_guarded_inline_bounds_and_success_only_publication() {
    let mut cache = NameSelectorCache::default();
    assert_eq!(size_of::<NameSelectorCache>(), 16);
    cache.insert(0, false);
    cache.insert(63, true);
    cache.insert(64, true);
    cache.insert(usize::MAX, true);
    assert_eq!(cache.get(0), Some(false));
    assert_eq!(cache.get(63), Some(true));
    assert_eq!(cache.get(64), None);
    assert_eq!(cache.get(usize::MAX), None);
    assert_eq!(cache.get(1), None);
    let g = compile("SecRule ARGS:/^json[.]probe$/ \"@contains hit\" \"id:903,phase:2,pass\"\n");
    let runtime = RuntimeLimits::default();
    let mut tx = Transaction::new(&g, request(), runtime).unwrap();
    let mut sink = collector(
        &g,
        &mut tx,
        runtime,
        EvaluationMode::Accelerated,
        ResultLimits::default(),
    );
    let selector = g.json_inspection.candidate_regexes[0];
    let scope = measurements::start();
    let mut cache = NameSelectorCache::default();
    assert!(match_name(
        &g,
        &mut [],
        selector,
        b"json.probe",
        &mut sink.state.store,
        &mut cache,
        EvaluationMode::Accelerated
    )
    .is_err());
    assert_eq!(cache.seen, 0);
    assert_eq!(scope.counts().get(Event::NameSelectorSearches), 0);
    assert!(match_name(
        &g,
        sink.context.boolean_scratch,
        selector,
        b"json.probe",
        &mut sink.state.store,
        &mut cache,
        EvaluationMode::Accelerated
    )
    .unwrap());
    assert!(match_name(
        &g,
        &mut [],
        selector,
        b"json.probe",
        &mut sink.state.store,
        &mut cache,
        EvaluationMode::Accelerated
    )
    .is_err());
}

#[test]
fn current_name_selector_cache_overflow_uses_original_searches() {
    let source = (0..65)
        .map(|i| {
            format!(
                "SecRule ARGS:/^json[.]key{i}$/ \"@contains hit\" \"id:{},phase:2,pass\"\n",
                1000 + i
            )
        })
        .collect::<String>();
    let g = compile(&source);
    let selector = *g
        .json_inspection
        .candidate_regexes
        .iter()
        .find(|selector| {
            matches!(
                selector,
                crate::seclang::compiler::CompiledTargetSelector::Regex { program: 64, .. }
            )
        })
        .unwrap();
    let runtime = RuntimeLimits::default();
    let mut tx = Transaction::new(&g, request(), runtime).unwrap();
    let mut sink = collector(
        &g,
        &mut tx,
        runtime,
        EvaluationMode::Accelerated,
        ResultLimits::default(),
    );
    let scope = measurements::start();
    let mut cache = NameSelectorCache::default();
    for _ in 0..2 {
        assert!(match_name(
            &g,
            sink.context.boolean_scratch,
            selector,
            b"json.key64",
            &mut sink.state.store,
            &mut cache,
            EvaluationMode::Accelerated
        )
        .unwrap());
    }
    assert_eq!(scope.counts().get(Event::NameSelectorSearches), 2);
    assert_eq!(cache.seen, 0);
}

#[test]
fn current_name_selector_cache_resets_per_field_and_preserves_exclusion_replay() {
    let g = compile(concat!(
        "SecRule ARGS:/^json[.]probe$/ \"@contains hit\" \"id:904,phase:2,pass\"\n",
        "SecRule ARGS|!ARGS:/^json[.]probe$/ \"@contains hit\" \"id:905,phase:2,pass\"\n",
        "SecRule ARGS_NAMES:/^json[.]probe$/ \"@contains probe\" \"id:906,phase:2,pass,t:lowercase\"\n",
    ));
    let input = br#"{"probe":"hit","other":"hit","nested":{"probe":"hit"},"probe":"safe"}"#;
    for split in 0..=input.len() {
        let mut outcomes = Vec::new();
        for mode in [EvaluationMode::Accelerated, EvaluationMode::Reference] {
            let runtime = RuntimeLimits::default();
            let mut tx = Transaction::new_with_evaluation(&g, request(), runtime, mode).unwrap();
            tx.execute_request_headers().unwrap();
            tx.publish_request_body(RequestBody::new(
                bytes::Bytes::copy_from_slice(input),
                crate::seclang::BodyProcessorSelection::Auto,
            ))
            .unwrap();
            outcomes.push(tx.execute_request_body().unwrap());
            let mut tx = Transaction::new(&g, request(), runtime).unwrap();
            let mut sink = collector(&g, &mut tx, runtime, mode, ResultLimits::default());
            feed(&mut sink, input, split);
            assert_eq!(sink.usage().live_outcomes, 3);
        }
        assert_eq!(outcomes[0], outcomes[1]);
    }
}

#[test]
fn no_hit_payloads_are_absent_and_summary_only_has_no_candidates() {
    for source in [
        r#"SecRule ARGS "@contains blocked" "id:1,phase:2,pass,t:lowercase""#,
        r#"SecRule &ARGS "@gt 9" "id:1,phase:2,pass""#,
    ] {
        let generation = compile(source);
        let runtime = RuntimeLimits::default();
        let mut tx = Transaction::new(&generation, request(), runtime).unwrap();
        let mut sink = collector(
            &generation,
            &mut tx,
            runtime,
            EvaluationMode::Accelerated,
            ResultLimits::default(),
        );
        feed(&mut sink, br#"{"a":"safe","b":"okay"}"#, 8);
        let usage = sink.usage();
        assert_eq!(
            (
                usage.matched_bytes,
                usage.deferred_bytes,
                usage.capture_bytes,
                usage.live_outcomes,
                usage.live_profiles
            ),
            (0, 0, 0, 0, 0)
        );
        assert_eq!(
            usage.live_candidates,
            if generation.json_inspection.groups.is_empty() {
                0
            } else {
                2
            }
        );
        let (sealed, _) = sink.into_sealed().unwrap();
        assert_eq!(sealed.json_summary().unwrap().argument_count, 2);
    }
}

#[test]
fn duplicate_errors_and_payloads_are_replaced_at_every_split() {
    let generation = compile(r#"SecRule ARGS "@contains hit" "id:1,phase:2,pass""#);
    let runtime = RuntimeLimits {
        max_transformation_output_bytes: 3,
        ..RuntimeLimits::default()
    };
    let input = br#"{"z":"hit","a":"too-long","a":"hit"}"#;
    for split in 0..=input.len() {
        let mut tx = Transaction::new(&generation, request(), runtime).unwrap();
        let mut sink = collector(
            &generation,
            &mut tx,
            runtime,
            EvaluationMode::Accelerated,
            ResultLimits::default(),
        );
        feed(&mut sink, input, split);
        assert_eq!(sink.usage().live_outcomes, 2);
        assert_eq!(sink.usage().live_profiles, 0);
        let (sealed, mut budget) = sink.into_sealed().unwrap();
        let first = sealed.receipt.unwrap().head;
        assert_eq!(sealed.candidate(&mut budget, first).unwrap().name, b"json.a");
        assert!(matches!(
            sealed
                .lookup_selected(&mut budget, first, 0, &generation, runtime, EvaluationMode::Accelerated)
                .map(|(outcome, _)| outcome)
                .unwrap(),
            SelectedOutcome::MatchOrError(_)
        ));
    }
}

#[test]
fn changed_topology_and_late_transform_error_override_early_match() {
    let generation = compile(r#"SecRule ARGS "@contains A" "id:1,phase:2,pass,multiMatch,t:lowercase,t:hexEncode""#);
    let runtime = RuntimeLimits {
        max_transformation_output_bytes: 3,
        ..RuntimeLimits::default()
    };
    let mut tx = Transaction::new(&generation, request(), runtime).unwrap();
    let mut sink = collector(
        &generation,
        &mut tx,
        runtime,
        EvaluationMode::Accelerated,
        ResultLimits::default(),
    );
    feed(&mut sink, br#"{"a":"ABC"}"#, 5);
    assert_eq!(sink.usage().live_outcomes, 0);
    assert_eq!(sink.usage().matched_bytes, 0);
    let (sealed, mut budget) = sink.into_sealed().unwrap();
    let id = sealed.receipt.unwrap().head;
    let profile = sealed.profile(&mut budget, id, 0).unwrap();
    assert!(profile.error.is_some());
    let change = sealed
        .change(&mut budget, id, profile.record.unwrap(), profile.first_change.unwrap())
        .unwrap();
    assert_eq!(change.output_len, 3);
    assert!(change.next.is_none());
    assert!(matches!(
        sealed
            .lookup_selected(&mut budget, id, 0, &generation, runtime, EvaluationMode::Accelerated)
            .map(|(outcome, _)| outcome)
            .unwrap(),
        SelectedOutcome::TransformError(_)
    ));
}

#[test]
fn thirty_two_subscribers_share_one_scan_per_stage_in_both_modes() {
    let mut source = String::new();
    for id in 1..=32 {
        source.push_str(&format!(
            "SecRule ARGS \"@pm needle{id}\" \"id:{id},phase:2,pass,multiMatch,t:lowercase\"\n"
        ));
    }
    let generation = compile(&source);
    assert_eq!(generation.groups.len(), 1);
    assert_eq!(generation.json_inspection.groups.len(), 32);
    assert_eq!(generation.json_inspection.matcher_scan_capacity, 1);
    for mode in [EvaluationMode::Accelerated, EvaluationMode::Reference] {
        let runtime = RuntimeLimits::default();
        let mut tx = Transaction::new(&generation, request(), runtime).unwrap();
        let mut sink = collector(&generation, &mut tx, runtime, mode, ResultLimits::default());
        assert_eq!(sink.state.dispatch.cache.heap_capacity(), 0);
        feed(&mut sink, br#"{"a":"SAFE","b":"safe"}"#, 9);
        assert_eq!(sink.state.dispatch.cache.scans, 3);
        assert_eq!(sink.usage().live_outcomes, 0);
        assert_eq!(sink.usage().live_changes, 1);
    }
}

#[test]
fn capture_only_and_mixed_profiles_use_compiled_matcher_cache_bound() {
    for (source, expected) in [
        (
            concat!(
                "SecRule ARGS \"@rx (?P<word>a)\" \"id:1,phase:2,pass,capture,t:none\"\n",
                "SecRule ARGS \"@rx (?P<word>a)\" \"id:2,phase:2,pass,capture,t:none\"\n",
                "SecRule ARGS \"@contains z\" \"id:3,phase:2,pass,t:none\"\n",
            ),
            0,
        ),
        (
            concat!(
                "SecRule ARGS \"@pm first\" \"id:1,phase:2,pass,t:none\"\n",
                "SecRule ARGS \"@pm second\" \"id:2,phase:2,pass,t:none\"\n",
                "SecRule ARGS \"@pm third\" \"id:3,phase:2,pass,t:none,t:lowercase,multiMatch\"\n",
                "SecRule ARGS \"@rx (?P<word>a)\" \"id:4,phase:2,pass,capture,t:none\"\n",
            ),
            2,
        ),
    ] {
        let generation = compile(source);
        assert_eq!(generation.json_inspection.matcher_scan_capacity, expected);
        for mode in [EvaluationMode::Accelerated, EvaluationMode::Reference] {
            let runtime = RuntimeLimits::default();
            let mut tx = Transaction::new(&generation, request(), runtime).unwrap();
            let mut sink = collector(&generation, &mut tx, runtime, mode, ResultLimits::default());
            let capacity = sink.state.dispatch.cache.heap_capacity();
            assert_eq!(capacity, 0);
            feed(&mut sink, br#"{"x":"FIRST third a","y":"safe"}"#, 9);
            assert_eq!(sink.state.dispatch.cache.heap_capacity(), capacity);
            assert!(sink.state.dispatch.cache.len() <= expected);
            assert_eq!(sink.state.dispatch.cache.scans == 0, expected == 0);
        }
    }
}

#[test]
fn shared_capture_program_aliases_resolve_identical_ordered_captures_in_both_modes() {
    let generation = compile(concat!(
        "SecRule ARGS \"@rx ^(?P<word>a)(?P<empty>)$\" \"id:1,phase:2,pass,capture,t:none\"\n",
        "SecRule ARGS \"@rx ^(?P<word>a)(?P<empty>)$\" \"id:2,phase:2,pass,capture,t:none\"\n",
        "SecRule ARGS \"!@rx ^(?P<word>a)(?P<empty>)$\" \"id:3,phase:2,pass,capture,t:none\"\n",
    ));
    assert_eq!(generation.capture_regexes.len(), 1);
    assert_eq!(generation.json_inspection.groups.len(), 2);
    for mode in [EvaluationMode::Accelerated, EvaluationMode::Reference] {
        let runtime = RuntimeLimits::default();
        let mut tx = Transaction::new(&generation, request(), runtime).unwrap();
        let mut sink = collector(&generation, &mut tx, runtime, mode, ResultLimits::default());
        feed(&mut sink, br#"{"x":"a"}"#, 5);
        assert_eq!(sink.usage().live_outcomes, 1);
        assert_eq!(sink.usage().live_captures, 5);
        let (sealed, mut budget) = sink.into_sealed().unwrap();
        let id = sealed.receipt.unwrap().head;
        let mut alias_outcome = None;
        for member in [0, 1] {
            let super::super::super::json_inspection_plan::JsonMemberMode::StaticFact(group) =
                generation.json_inspection.members[member].value
            else {
                panic!("expected alias group");
            };
            let SelectedOutcome::MatchOrError(outcome) = sealed
                .lookup_selected(&mut budget, id, group, &generation, runtime, mode)
                .map(|(outcome, _)| outcome)
                .unwrap()
            else {
                panic!("expected alias match");
            };
            if let Some(previous) = alias_outcome {
                assert_eq!(outcome, previous);
            }
            alias_outcome = Some(outcome);
            let OutcomeValue::Match { first_capture, .. } = sealed.outcome(&mut budget, id, outcome).unwrap().result
            else {
                panic!("expected alias captures");
            };
            let mut next = first_capture;
            for (key, value) in [
                (b"0".as_slice(), b"a".as_slice()),
                (b"1", b"a"),
                (b"word", b"a"),
                (b"2", b""),
                (b"empty", b""),
            ] {
                let capture = sealed.capture(&mut budget, id, outcome, next.unwrap()).unwrap();
                assert_eq!(sealed.match_value(&mut budget, capture.key).unwrap(), key);
                assert_eq!(sealed.match_value(&mut budget, capture.value).unwrap(), value);
                next = capture.next;
            }
            assert!(next.is_none());
        }
        let super::super::super::json_inspection_plan::JsonMemberMode::StaticFact(group) =
            generation.json_inspection.members[2].value
        else {
            panic!("expected negated group");
        };
        assert!(matches!(
            sealed
                .lookup_selected(&mut budget, id, group, &generation, runtime, mode)
                .map(|(outcome, _)| outcome)
                .unwrap(),
            SelectedOutcome::NoMatch
        ));
    }
}

#[test]
fn compiled_matcher_cache_constructor_has_exact_physical_admission() {
    for header_count in [0, 64] {
        let mut source = (0..header_count)
            .map(|index| {
                format!(
                    "SecRule REQUEST_HEADERS:X-{index} \"@pm needle\" \"id:{},phase:1,pass,t:none\"\n",
                    100 + index
                )
            })
            .collect::<String>();
        source.push_str(concat!(
            "SecRule ARGS \"@pm first\" \"id:1,phase:2,pass,t:none\"\n",
            "SecRule ARGS \"@pm second\" \"id:2,phase:2,pass,t:none\"\n",
            "SecRule ARGS \"@rx (a)\" \"id:3,phase:2,pass,capture,t:none\"\n",
        ));
        let generation = compile(&source);
        assert_eq!(generation.matcher_count(), header_count + 1);
        assert_eq!(generation.json_inspection.matcher_scan_capacity, 1);
        let runtime = RuntimeLimits::default();
        let required = {
            let mut tx = Transaction::new(&generation, request(), runtime).unwrap();
            let sink = collector(
                &generation,
                &mut tx,
                runtime,
                EvaluationMode::Accelerated,
                ResultLimits::default(),
            );
            assert_eq!(sink.usage().retained_bytes, sink.usage().peak_retained_bytes);
            sink.usage().peak_retained_bytes
        };
        for (bound, succeeds) in [(required, true), (required - 1, false)] {
            let mut tx = Transaction::new(&generation, request(), runtime).unwrap();
            let result = JsonCollector::new(
                GenerationHandle::Borrowed(&generation),
                SingleInputContext {
                    generation: &generation,
                    request: &tx.request,
                    tx: &tx.tx,
                    group_scratch: &mut tx.group_scratch,
                    boolean_scratch: &mut tx.boolean_scratch,
                    capture_scratch: &mut tx.capture_scratch,
                    scan_cache: None,
                    json_budget: None,
                },
                runtime,
                EvaluationMode::Accelerated,
                ResultLimits {
                    retained_bytes: bound,
                    ..ResultLimits::default()
                },
            );
            assert_eq!(result.is_ok(), succeeds);
            if !succeeds {
                assert!(matches!(result, Err(CollectorError::Store(StoreError::Capacity))));
            }
        }
    }
}

#[test]
fn literal_regex_dispatch_named_dependencies_and_negated_captures() {
    let generation = compile(concat!(
        "SecRule ARGS:json.a|ARGS:/^json\\.a$/ \"!@rx blocked\" \"id:1,phase:2,pass,capture\"\n",
        "SecRule REQUEST_HEADERS:X \"@streq %{ARGS.json.keep}\" \"id:2,phase:2,pass\"\n",
    ));
    let runtime = RuntimeLimits::default();
    let mut tx = Transaction::new(&generation, request(), runtime).unwrap();
    let mut sink = collector(
        &generation,
        &mut tx,
        runtime,
        EvaluationMode::Accelerated,
        ResultLimits::default(),
    );
    feed(&mut sink, br#"{"a":"safe","keep":"secret","other":"discard"}"#, 10);
    assert_eq!(sink.usage().live_candidates, 2);
    assert_eq!(sink.usage().live_outcomes, 1);
    assert_eq!(sink.usage().live_captures, 0);
    assert_eq!(sink.usage().deferred_bytes, 6);
    let (sealed, mut budget) = sink.into_sealed().unwrap();
    let first = sealed.receipt.unwrap().head;
    let SelectedOutcome::MatchOrError(outcome) = sealed
        .lookup_selected(&mut budget, first, 0, &generation, runtime, EvaluationMode::Accelerated)
        .map(|(outcome, _)| outcome)
        .unwrap()
    else {
        panic!("missing negated match")
    };
    let view = sealed.outcome(&mut budget, first, outcome).unwrap();
    let OutcomeValue::Match { value, .. } = view.result else {
        panic!("expected match")
    };
    assert_eq!(sealed.match_value(&mut budget, value).unwrap(), b"safe");
}

#[test]
fn physical_failure_is_saved_and_does_not_seal_partial_results() {
    let generation = compile(r#"SecRule ARGS "@contains hit" "id:1,phase:2,pass""#);
    let runtime = RuntimeLimits::default();
    let mut tx = Transaction::new(&generation, request(), runtime).unwrap();
    let mut sink = collector(
        &generation,
        &mut tx,
        runtime,
        EvaluationMode::Accelerated,
        ResultLimits {
            candidates: 0,
            ..ResultLimits::default()
        },
    );
    let mut checker =
        JsonChecker::<Receipt>::new("json", JsonStreamLimits::default(), JsonStructureLimits::default()).unwrap();
    assert!(checker
        .feed(JsonStreamInput::Data(br#"{"a":"hit"}"#), &mut sink)
        .is_err());
    let error = sink.failure();
    assert_eq!(error, Some(CollectorError::Store(StoreError::Candidates)));
    assert!(checker.feed(JsonStreamInput::Complete, &mut sink).is_err());
    assert_eq!(sink.failure(), error);
    assert_eq!(sink.usage().live_candidates, 0);
    assert_eq!(sink.usage().retained_bytes, 0);
    assert!(sink.into_sealed().is_err());
}

#[test]
fn positive_captures_and_stored_payload_share_the_peak_capacity_budget() {
    let generation = compile(r#"SecRule ARGS "@rx (secret)([0-9]+)" "id:1,phase:2,pass,capture""#);
    let runtime = RuntimeLimits::default();
    let input = br#"{"a":"secret123"}"#;
    let mut tx = Transaction::new(&generation, request(), runtime).unwrap();
    let mut sink = collector(
        &generation,
        &mut tx,
        runtime,
        EvaluationMode::Accelerated,
        ResultLimits::default(),
    );
    feed(&mut sink, input, 4);
    let usage = sink.usage();
    assert_eq!(usage.live_captures, 3);
    assert_eq!(usage.matched_bytes, 9);
    let peak = usage.peak_retained_bytes;
    let (sealed, mut budget) = sink.into_sealed().unwrap();
    assert!(peak > budget.usage().retained_bytes);
    let id = sealed.receipt.unwrap().head;
    let SelectedOutcome::MatchOrError(outcome) = sealed
        .lookup_selected(&mut budget, id, 0, &generation, runtime, EvaluationMode::Accelerated)
        .map(|(outcome, _)| outcome)
        .unwrap()
    else {
        panic!("missing capture match")
    };
    let OutcomeValue::Match {
        value,
        first_capture: captures,
    } = sealed.outcome(&mut budget, id, outcome).unwrap().result
    else {
        panic!("unexpected predicate error")
    };
    assert_eq!(sealed.match_value(&mut budget, value).unwrap(), b"secret123");
    let capture = sealed.capture(&mut budget, id, outcome, captures.unwrap()).unwrap();
    assert_eq!(
        (
            sealed.match_value(&mut budget, capture.key).unwrap(),
            sealed.match_value(&mut budget, capture.value).unwrap()
        ),
        (b"0".as_slice(), b"secret123".as_slice())
    );
    // Store growth clips geometric reservations to the available cap. Find the
    // actual minimum admission, rather than treating an unconstrained high-water
    // mark as the required minimum of a differently constrained allocation.
    let succeeds = |limit| {
        let mut tx = Transaction::new(&generation, request(), runtime).unwrap();
        let context = SingleInputContext {
            generation: &generation,
            request: &tx.request,
            tx: &tx.tx,
            group_scratch: &mut tx.group_scratch,
            boolean_scratch: &mut tx.boolean_scratch,
            capture_scratch: &mut tx.capture_scratch,
            scan_cache: None,
            json_budget: None,
        };
        let Ok(mut sink) = JsonCollector::new(
            GenerationHandle::Borrowed(&generation),
            context,
            runtime,
            EvaluationMode::Accelerated,
            ResultLimits {
                retained_bytes: limit,
                ..ResultLimits::default()
            },
        ) else {
            return false;
        };
        let mut checker =
            JsonChecker::<Receipt>::new("json", JsonStreamLimits::default(), JsonStructureLimits::default()).unwrap();
        let result = checker
            .feed(JsonStreamInput::Data(input), &mut sink)
            .and_then(|()| checker.feed(JsonStreamInput::Complete, &mut sink));
        if result.is_err() {
            assert_eq!(sink.failure(), Some(CollectorError::Store(StoreError::Capacity)));
            assert_eq!(sink.usage().retained_bytes, 0);
        }
        result.is_ok()
    };
    let (mut low, mut high) = (0, peak);
    while low < high {
        let mid = low + (high - low) / 2;
        if succeeds(mid) {
            high = mid;
        } else {
            low = mid + 1;
        }
    }
    assert!(!succeeds(low - 1));
    assert!(succeeds(low));
    assert!(succeeds(low + 1));
}

#[test]
fn incomplete_stage_failures_are_semantic_but_work_exhaustion_is_physical() {
    let generation = compile(r#"SecRule ARGS "@eq 7" "id:1,phase:2,pass""#);
    let runtime = RuntimeLimits::default();
    let input = br#"{"a":"not-a-number"}"#;
    let mut tx = Transaction::new(&generation, request(), runtime).unwrap();
    let mut sink = collector(
        &generation,
        &mut tx,
        runtime,
        EvaluationMode::Accelerated,
        ResultLimits::default(),
    );
    feed(&mut sink, input, 7);
    let work = sink.usage().work;
    let (sealed, mut budget) = sink.into_sealed().unwrap();
    let id = sealed.receipt.unwrap().head;
    let SelectedOutcome::MatchOrError(outcome) = sealed
        .lookup_selected(&mut budget, id, 0, &generation, runtime, EvaluationMode::Accelerated)
        .map(|(outcome, _)| outcome)
        .unwrap()
    else {
        panic!("missing integer error")
    };
    assert!(
        matches!(sealed.outcome(&mut budget, id, outcome).unwrap().result, OutcomeValue::Error(CheckpointError { checkpoint, .. }) if checkpoint == StoredCheckpoint::Operator as usize)
    );
    for limit in [work - 1, work, work + 1] {
        let mut tx = Transaction::new(&generation, request(), runtime).unwrap();
        let mut sink = collector(
            &generation,
            &mut tx,
            runtime,
            EvaluationMode::Accelerated,
            ResultLimits {
                work: limit,
                ..ResultLimits::default()
            },
        );
        let mut checker =
            JsonChecker::<Receipt>::new("json", JsonStreamLimits::default(), JsonStructureLimits::default()).unwrap();
        let result = checker
            .feed(JsonStreamInput::Data(input), &mut sink)
            .and_then(|()| checker.feed(JsonStreamInput::Complete, &mut sink));
        assert_eq!(result.is_ok(), limit >= work);
        if limit < work {
            assert_eq!(sink.failure(), Some(CollectorError::Store(StoreError::Work)));
        }
    }
}

#[test]
fn borrowed_static_operand_preserves_first_prefix_error_and_empty_identity() {
    let mut generation = compile(r#"SecRule ARGS "@streq abcdef" "id:1,phase:2,pass""#);
    let super::super::super::CompiledOperatorOperand::String { template, .. } =
        generation.operator_plans[0].unwrap().operand
    else {
        panic!("string operand")
    };
    let segment = generation.segments[generation.templates[template].segment_start];
    let super::super::super::CompiledSegmentKind::Literal(range) = segment.kind else {
        panic!("literal operand")
    };
    let start = generation.segments.len();
    for offset in [0, 2, 4] {
        generation.segments.push(super::super::super::CompiledTemplateSegment {
            kind: super::super::super::CompiledSegmentKind::Literal(super::super::super::ByteRange {
                start: range.start + offset,
                len: 2,
            }),
        });
    }
    generation.templates[template].segment_start = start;
    generation.templates[template].segment_count = 3;
    assert_eq!(
        static_template_operand(&generation, template, 3),
        Err(RuntimeError::ValueByteLimit { actual: 4, limit: 3 })
    );
    let value = static_template_operand(&generation, template, 6).unwrap().unwrap();
    assert_eq!(value, b"abcdef");
    assert_eq!(value.as_ptr(), generation.bytes[range.start..].as_ptr());
    generation.templates[template].runtime_byte_limit = 1;
    assert_eq!(
        static_template_operand(&generation, template, 6),
        Err(RuntimeError::ValueByteLimit { actual: 2, limit: 1 })
    );
    generation.templates[template].segment_count = 0;
    assert_eq!(
        static_template_operand(&generation, template, 0),
        Ok(Some(b"".as_slice()))
    );
}

#[test]
fn invalid_matcher_range_never_publishes_scan_completion() {
    let generation = compile(r#"SecRule ARGS "@pm needle" "id:1,phase:2,pass""#);
    let runtime = RuntimeLimits::default();
    let mut tx = Transaction::new(&generation, request(), runtime).unwrap();
    let measurements = crate::seclang::measurements::start();
    let mut cache = StageScanCache {
        storage: ScanCacheStorage::Overflow(Vec::with_capacity(1)),
        scans: 0,
    };
    assert!(evaluate_matcher_operand_cached(
        &generation,
        &mut tx.group_scratch,
        0,
        usize::MAX,
        1,
        b"needle",
        EvaluationMode::Accelerated,
        Some(&mut cache)
    )
    .is_err());
    let incompatible = compile(r#"SecRule ARGS "@pm one two" "id:1,phase:2,pass""#);
    let mut other = Transaction::new(&incompatible, request(), runtime).unwrap();
    other.group_scratch[0] = Some(incompatible.groups[0].matcher.create_scratch());
    assert!(evaluate_matcher_operand_cached(
        &generation,
        &mut other.group_scratch,
        0,
        0,
        1,
        b"needle",
        EvaluationMode::Accelerated,
        Some(&mut cache)
    )
    .is_err());
    assert!(cache.len() == 0);
    assert_eq!(cache.scans, 0);
    assert_eq!(
        measurements
            .counts()
            .get(crate::seclang::measurements::Event::FullScans),
        0
    );
    assert!(evaluate_matcher_operand_cached(
        &generation,
        &mut tx.group_scratch,
        0,
        0,
        2,
        b"needle",
        EvaluationMode::Accelerated,
        Some(&mut cache)
    )
    .is_err());
    assert!(cache.len() == 0);
    assert_eq!(cache.scans, 0);
    assert_eq!(
        measurements
            .counts()
            .get(crate::seclang::measurements::Event::FullScans),
        1
    );
    assert_eq!(
        measurements
            .counts()
            .get(crate::seclang::measurements::Event::RangeQueries),
        1
    );
    assert_eq!(
        measurements
            .counts()
            .get(crate::seclang::measurements::Event::RangeQueryFailures),
        1
    );
    assert!(evaluate_matcher_operand_cached(
        &generation,
        &mut tx.group_scratch,
        0,
        0,
        1,
        b"needle",
        EvaluationMode::Accelerated,
        Some(&mut cache)
    )
    .unwrap());
    assert_eq!(cache.scans, 1);
    assert_eq!(
        measurements
            .counts()
            .get(crate::seclang::measurements::Event::FullScans),
        2
    );
    assert_eq!(
        measurements
            .counts()
            .get(crate::seclang::measurements::Event::RangeQueries),
        2
    );
}

impl JsonCollector<'_, '_> {
    fn usage(&self) -> json_inspection::StoreUsage {
        self.state.store.usage()
    }
}

#[test]
fn distinct_matching_groups_copy_stage_payload_once_and_reclaim_duplicates() {
    let mut source = String::new();
    let mut value = String::new();
    for i in 0..32 {
        source.push_str(&format!(
            "SecRule ARGS \"@contains token{i:02}\" \"id:{},phase:2,pass,t:none\"\n",
            i + 1
        ));
        value.push_str(&format!("token{i:02} "));
    }
    let generation = compile(&source);
    assert_eq!(generation.json_inspection.groups.len(), 32);
    assert_eq!(generation.json_inspection.profiles.len(), 1);
    for duplicate in [false, true] {
        let mut tx = Transaction::new(&generation, request(), RuntimeLimits::default()).unwrap();
        let mut sink = collector(
            &generation,
            &mut tx,
            RuntimeLimits::default(),
            EvaluationMode::Accelerated,
            ResultLimits::default(),
        );
        let body = if duplicate {
            format!("{{\"x\":\"{value}\",\"x\":\"safe\"}}")
        } else {
            format!("{{\"x\":\"{value}\"}}")
        };
        let scope = measurements::start();
        feed(&mut sink, body.as_bytes(), body.len());
        assert_eq!(scope.counts().get(Event::MatchedCopyBytes), value.len());
        assert_eq!(sink.usage().matched_bytes, if duplicate { 0 } else { value.len() });
        assert_eq!(sink.usage().live_outcomes, if duplicate { 0 } else { 32 });
    }
}

#[test]
fn shared_matches_remain_separate_across_profiles_and_multimatch_stages() {
    let generation = compile(concat!(
        "SecRule ARGS \"@contains A\" \"id:1,phase:2,pass,t:none,t:lowercase,multiMatch\"\n",
        "SecRule ARGS \"@contains a\" \"id:2,phase:2,pass,t:none,t:lowercase,multiMatch\"\n",
        "SecRule ARGS \"@contains A\" \"id:3,phase:2,pass,t:none\"\n",
    ));
    let mut tx = Transaction::new(&generation, request(), RuntimeLimits::default()).unwrap();
    let mut sink = collector(
        &generation,
        &mut tx,
        RuntimeLimits::default(),
        EvaluationMode::Accelerated,
        ResultLimits::default(),
    );
    let scope = measurements::start();
    feed(&mut sink, br#"{"x":"ABC"}"#, 0);
    assert_eq!(sink.usage().live_outcomes, 3);
    assert_eq!(sink.usage().matched_bytes, 9);
    assert_eq!(scope.counts().get(Event::MatchedCopyBytes), 9);
}

#[test]
fn range_capture_collection_preserves_aliases_offsets_and_generated_fingerprints() {
    for (operator, transforms, input, expected) in [
        (
            "@rx ^pre(?P<word>hit)(?P<empty>)(x)?$",
            ",t:lowercase",
            br#"{"a":"PRE\u0048IT"}"#.as_slice(),
            vec![
                (b"0".as_slice(), b"prehit".as_slice(), Some(0)),
                (b"1", b"hit", Some(3)),
                (b"word", b"hit", Some(3)),
                (b"2", b"", Some(6)),
                (b"empty", b"", Some(6)),
            ],
        ),
        (
            "@pm token",
            ",t:lowercase",
            br#"{"a":"A TOKEN HERE"}"#,
            vec![(b"0".as_slice(), b"token".as_slice(), Some(2))],
        ),
        (
            "@detectSQLi",
            "",
            br#"{"a":"1 OR 1=1"}"#,
            vec![(b"0".as_slice(), b"1&1".as_slice(), None)],
        ),
    ] {
        for mode in [EvaluationMode::Accelerated, EvaluationMode::Reference] {
            let generation = compile(&format!(
                "SecRule ARGS \"{operator}\" \"id:1,phase:2,pass,capture{transforms}\""
            ));
            let runtime = RuntimeLimits::default();
            let mut tx = Transaction::new(&generation, request(), runtime).unwrap();
            let scope = measurements::start();
            let mut sink = collector(&generation, &mut tx, runtime, mode, ResultLimits::default());
            feed(&mut sink, input, 5);
            let logical = expected
                .iter()
                .map(|(key, value, _)| key.len() + value.len())
                .sum::<usize>();
            let keys = expected.iter().map(|(key, _, _)| key.len()).sum::<usize>();
            assert_eq!(sink.usage().capture_bytes, logical);
            assert_eq!(scope.counts().get(Event::TemporaryCaptureCopyBytes), 0);
            assert_eq!(
                scope.counts().get(Event::CaptureCopyBytes),
                keys + if operator == "@detectSQLi" { 3 } else { 0 }
            );
            let (sealed, mut budget) = sink.into_sealed().unwrap();
            let id = sealed.receipt.unwrap().head;
            let SelectedOutcome::MatchOrError(outcome) = sealed
                .lookup_selected(&mut budget, id, 0, &generation, runtime, mode)
                .map(|(outcome, _)| outcome)
                .unwrap()
            else {
                panic!("expected outcome")
            };
            let OutcomeValue::Match { value, first_capture } = sealed.outcome(&mut budget, id, outcome).unwrap().result
            else {
                panic!("expected match")
            };
            let parent = sealed.match_value(&mut budget, value).unwrap();
            let mut next = first_capture;
            for (key, expected, offset) in &expected {
                let capture = sealed.capture(&mut budget, id, outcome, next.unwrap()).unwrap();
                let bytes = sealed.match_value(&mut budget, capture.value).unwrap();
                assert_eq!(sealed.match_value(&mut budget, capture.key).unwrap(), *key);
                assert_eq!(bytes, *expected);
                if let Some(offset) = offset {
                    assert_eq!(bytes.as_ptr(), parent[*offset..].as_ptr());
                } else {
                    assert_ne!(bytes.as_ptr(), parent.as_ptr());
                }
                next = capture.next;
            }
            assert!(next.is_none());
        }
    }
}

#[test]
fn range_capture_logical_limits_keep_alias_lengths_and_order() {
    let generation = compile("SecRule ARGS \"@rx ^pre(?P<word>hit)(?P<empty>)(x)?$\" \"id:1,phase:2,pass,capture\"");
    for (entries, bytes, expected) in [
        (5, 24, None),
        (4, 24, Some(RuntimeError::CaptureEntryLimit { limit: 4 })),
        (0, 24, Some(RuntimeError::CaptureEntryLimit { limit: 0 })),
        (5, 23, Some(RuntimeError::CaptureByteLimit { limit: 23 })),
        (5, 0, Some(RuntimeError::CaptureByteLimit { limit: 0 })),
    ] {
        let runtime = RuntimeLimits {
            max_capture_entries: entries,
            max_capture_bytes: bytes,
            ..RuntimeLimits::default()
        };
        let mut tx = Transaction::new(&generation, request(), runtime).unwrap();
        let mut sink = collector(
            &generation,
            &mut tx,
            runtime,
            EvaluationMode::Accelerated,
            ResultLimits::default(),
        );
        feed(&mut sink, br#"{"a":"prehit"}"#, 4);
        assert_eq!(sink.usage().live_captures, if expected.is_none() { 5 } else { 0 });
        let (sealed, mut budget) = sink.into_sealed().unwrap();
        let id = sealed.receipt.unwrap().head;
        let SelectedOutcome::MatchOrError(outcome) = sealed
            .lookup_selected(&mut budget, id, 0, &generation, runtime, EvaluationMode::Accelerated)
            .map(|(outcome, _)| outcome)
            .unwrap()
        else {
            panic!("expected outcome")
        };
        match sealed.outcome(&mut budget, id, outcome).unwrap().result {
            OutcomeValue::Match { .. } => assert_eq!(expected, None),
            OutcomeValue::Error(error) => {
                assert_eq!(Some(error.error), expected);
                assert_eq!(error.checkpoint, StoredCheckpoint::Captures as usize);
            }
        }
    }
}

#[test]
fn range_capture_cleanup_discards_replaced_and_late_invalidated_hits() {
    for (source, input) in [
        (
            "SecRule ARGS \"@rx (hit)\" \"id:1,phase:2,pass,capture\"",
            br#"{"a":{"b":"hit"},"a":{"b":"miss"}}"#.as_slice(),
        ),
        (
            "SecRule ARGS \"@rx (HIT)\" \"id:1,phase:2,pass,capture,multiMatch,t:lowercase,t:hexEncode\"",
            br#"{"a":"HIT"}"#,
        ),
    ] {
        let generation = compile(source);
        let runtime = RuntimeLimits {
            max_transformation_output_bytes: if source.contains("hexEncode") {
                3
            } else {
                RuntimeLimits::default().max_transformation_output_bytes
            },
            ..RuntimeLimits::default()
        };
        let mut tx = Transaction::new(&generation, request(), runtime).unwrap();
        let scope = measurements::start();
        let mut sink = collector(
            &generation,
            &mut tx,
            runtime,
            EvaluationMode::Accelerated,
            ResultLimits::default(),
        );
        feed(&mut sink, input, 5);
        assert!(scope.counts().get(Event::MatchedCopyBytes) > 0);
        assert_eq!(scope.counts().get(Event::TemporaryCaptureCopyBytes), 0);
        assert_eq!(sink.usage().live_captures, 0);
        assert_eq!(sink.usage().capture_bytes, 0);
        assert_eq!(sink.usage().matched_bytes, 0);
        assert!(sink.into_sealed().is_ok());
    }
}

#[test]
fn capture_builder_reuses_high_water_capacity_and_releases_on_abandon_and_eos() {
    let generation = compile(concat!(
        "SecRule ARGS \"@rx (?P<a>hit)(?P<b>hit)(?P<c>hit)\" \"id:1,phase:2,pass,capture\"\n",
        "SecRule ARGS \"@rx hit\" \"id:2,phase:2,pass,capture\"\n",
    ));
    let runtime = RuntimeLimits::default();
    let mut tx = Transaction::new(&generation, request(), runtime).unwrap();
    let mut sink = collector(
        &generation,
        &mut tx,
        runtime,
        EvaluationMode::Accelerated,
        ResultLimits::default(),
    );
    assert_eq!(sink.state.captures.bytes().unwrap(), 0);
    let scope = measurements::start();
    for (name, value) in [(b"json.a".as_slice(), b"miss".as_slice()), (b"json.b", b"hithithit")] {
        sink.collect_scalar(name, value, JsonScalarKind::String).unwrap();
    }
    let capacity = sink.state.captures.builder.entries.capacity();
    let pointer = sink.state.captures.builder.entries.as_ptr();
    assert!(capacity >= 7);
    for (name, value) in [
        (b"json.c".as_slice(), b"hit".as_slice()),
        (b"json.d", b"miss"),
        (b"json.e", b"hit"),
    ] {
        sink.collect_scalar(name, value, JsonScalarKind::String).unwrap();
        assert_eq!(sink.state.captures.builder.entries.capacity(), capacity);
        assert_eq!(sink.state.captures.builder.entries.as_ptr(), pointer);
        assert!(sink.state.captures.builder.entries.is_empty());
        assert_eq!(sink.state.captures.builder.semantic_bytes, 0);
        assert_eq!(sink.state.captures.builder.heap_bytes, 0);
    }
    assert_eq!(scope.counts().get(Event::TemporaryCaptureCopyBytes), 0);
    sink.abandon();
    assert_eq!(sink.state.captures.bytes().unwrap(), 0);
    assert_eq!(sink.usage().retained_bytes, 0);
    assert_eq!(sink.into_abandoned_budget().unwrap().usage().retained_bytes, 0);

    let mut tx = Transaction::new(&generation, request(), runtime).unwrap();
    let mut sink = collector(
        &generation,
        &mut tx,
        runtime,
        EvaluationMode::Accelerated,
        ResultLimits::default(),
    );
    feed(&mut sink, br#"{"a":"hithithit","b":"hit"}"#, 9);
    assert_eq!(sink.state.captures.bytes().unwrap(), 0);
    assert_eq!(sink.into_sealed().unwrap().0.json_summary().unwrap().argument_count, 2);
}

#[test]
fn capture_keys_reject_foreign_generation_and_invalid_named_references() {
    let generation = compile("SecRule ARGS \"@rx (?P<word>hit)\" \"id:1,phase:2,pass,capture\"");
    let other = compile("SecRule ARGS \"@rx (?P<foreign>hit)\" \"id:1,phase:2,pass,capture\"");
    let mut tx = Transaction::new(&generation, request(), RuntimeLimits::default()).unwrap();
    let mut sink = collector(
        &generation,
        &mut tx,
        RuntimeLimits::default(),
        EvaluationMode::Accelerated,
        ResultLimits::default(),
    );
    sink.state.captures.generation = GenerationHandle::Borrowed(&other);
    assert!(sink.scalar(b"json.a", b"hit", JsonScalarKind::String).is_err());
    assert_eq!(sink.failure(), Some(CollectorError::State));
    assert!(sink.state.captures.builder.entries.is_empty());
    for key in [
        JsonCaptureKey::Named { program: 1, group: 1 },
        JsonCaptureKey::Named { program: 0, group: 0 },
        JsonCaptureKey::Named { program: 0, group: 2 },
    ] {
        assert!(matches!(
            key.with_bytes(&generation, |bytes| Ok::<_, RuntimeError>(bytes.len())),
            Err(RuntimeError::GenerationInvariant)
        ));
    }
    assert_eq!(
        JsonCaptureKey::Named { program: 0, group: 1 }
            .with_bytes(&generation, |bytes| Ok::<_, RuntimeError>(bytes.to_vec()))
            .unwrap(),
        b"word"
    );
    assert_eq!(
        JsonCaptureKey::Numeric(123)
            .with_bytes(&generation, |bytes| Ok::<_, RuntimeError>(bytes.to_vec()))
            .unwrap(),
        b"123"
    );
}

#[test]
fn capture_builder_clears_after_partial_store_transfer_failure() {
    let generation = compile("SecRule ARGS \"@rx (?P<a>hit)(?P<b>hit)(?P<c>hit)\" \"id:1,phase:2,pass,capture\"");
    let runtime = RuntimeLimits::default();
    let mut tx = Transaction::new(&generation, request(), runtime).unwrap();
    let mut sink = collector(
        &generation,
        &mut tx,
        runtime,
        EvaluationMode::Accelerated,
        ResultLimits {
            records: 12,
            ..ResultLimits::default()
        },
    );
    let scope = measurements::start();
    assert!(sink.scalar(b"json.a", b"hithithit", JsonScalarKind::String).is_err());
    assert_eq!(sink.failure(), Some(CollectorError::Store(StoreError::Records)));
    assert!(scope.counts().get(Event::CaptureCopyBytes) >= 5);
    assert!(sink.state.captures.builder.entries.is_empty());
    assert_eq!(sink.state.captures.builder.semantic_bytes, 0);
    assert_eq!(sink.state.captures.builder.heap_bytes, 0);
    assert!(sink.state.captures.bytes().unwrap() > 0);
    assert_eq!(sink.usage().live_captures, 0);
    assert!(sink.scalar(b"json.b", b"hit", JsonScalarKind::String).is_err());
    assert_eq!(sink.failure(), Some(CollectorError::Store(StoreError::Records)));
    sink.abandon();
    assert_eq!(sink.state.captures.bytes().unwrap(), 0);
    assert_eq!(sink.usage().retained_bytes, 0);
}

#[test]
fn transform_growth_reads_capture_capacity_created_after_profile_construction() {
    let generation = compile("SecRule ARGS \"@rx (?P<word>hit)\" \"id:1,phase:2,pass,capture,multiMatch,t:hexEncode\"");
    let runtime = RuntimeLimits::default();
    let mut tx = Transaction::new(&generation, request(), runtime).unwrap();
    let mut sink = collector(
        &generation,
        &mut tx,
        runtime,
        EvaluationMode::Accelerated,
        ResultLimits::default(),
    );
    let candidate = sink
        .store
        .begin_candidate(b"json.a", 3, JsonScalarKind::String, None)
        .unwrap();
    let mut groups = [GroupRun { id: 0, terminal: false }];
    let mut observer = ProfileObserver {
        fixed_scratch: sink.state.dispatch.bytes().unwrap(),
        store: &mut sink.state.store,
        context: &mut sink.context,
        groups: &mut groups,
        cache: &mut sink.state.dispatch.cache,
        hit_eligible: &mut sink.state.dispatch.hit_eligible,
        stamps: &mut sink.state.dispatch.stamps,
        epoch: &mut sink.state.dispatch.epoch,
        captures: &mut sink.state.captures,
        candidate,
        profile: 0,
        runtime,
        mode: EvaluationMode::Accelerated,
        transform_bytes: 0,
        last: StoredCheckpoint::Initial,
        step: 0,
    };
    assert_eq!(observer.captures.bytes().unwrap(), 0);
    observer.stage(0, b"hit").unwrap();
    assert!(observer.captures.bytes().unwrap() > 0);
    let before = observer.store.usage().retained_bytes;
    observer.admit_scratch(8192).unwrap();
    let mut transformation = Vec::<u8>::new();
    transformation.try_reserve_exact(8192).unwrap();
    observer.scratch_capacity(transformation.capacity()).unwrap();
    assert_eq!(
        observer.store.usage().retained_bytes,
        before + transformation.capacity()
    );
    // Drop actual external storage before terminal-safe scratch release.
    drop(transformation);
    sink.abandon();
    assert_eq!(sink.usage().retained_bytes, 0);
}

#[test]
fn capture_hit_batches_share_miss_searches_without_capture_caches() {
    let generation = compile(concat!(
        "SecRule ARGS \"@rx (evil)\" \"id:1,phase:2,pass,capture\"\n",
        "SecRule ARGS \"@rx (attack)\" \"id:2,phase:2,pass,capture\"\n",
        "SecRule ARGS \"@rx (evil)\" \"id:3,phase:2,pass,capture\"\n",
    ));
    assert_eq!(generation.capture_hits.matchers.len(), 1);
    assert_eq!(generation.json_inspection.capture_hits.batches.len(), 1);
    assert_eq!(generation.json_inspection.capture_hits.peers.len(), 2);
    assert!(generation.xml_inspection.capture_hits.bindings.is_empty());
    for mode in [EvaluationMode::Accelerated, EvaluationMode::Reference] {
        let runtime = RuntimeLimits::default();
        let scope = measurements::start();
        let mut tx = Transaction::new(&generation, request(), runtime).unwrap();
        assert!(tx.group_scratch.iter().all(Option::is_none));
        let mut sink = collector(&generation, &mut tx, runtime, mode, ResultLimits::default());
        feed(&mut sink, br#"{"a":"safe","b":"okay"}"#, 7);
        let counts = scope.counts();
        if mode == EvaluationMode::Accelerated {
            assert_eq!(counts.get(Event::FullScans), 2);
            assert_eq!(counts.get(Event::CaptureBooleanSearches), 0);
            assert_eq!(counts.get(Event::CaptureScratchInitializations), 0);
            assert_eq!(counts.get(Event::GroupScratchInitializations), 1);
        } else {
            assert_eq!(counts.get(Event::FullScans), 0);
            assert_eq!(counts.get(Event::CaptureBooleanSearches), 4);
            assert_eq!(counts.get(Event::CaptureScratchInitializations), 2);
        }
        assert_eq!(sink.usage().capture_bytes + sink.usage().matched_bytes, 0);
        assert!(sink.into_sealed().is_ok());
    }
}

#[test]
fn capture_hit_stage_eligibility_recomputes_after_terminal_multimatch_peer() {
    let generation = compile(concat!(
        "SecRule ARGS \"@rx (HIT)\" \"id:1,phase:2,pass,capture,multiMatch,t:lowercase\"\n",
        "SecRule ARGS \"@rx (hit)\" \"id:2,phase:2,pass,capture,multiMatch,t:lowercase\"\n",
    ));
    let runtime = RuntimeLimits::default();
    let scope = measurements::start();
    let mut tx = Transaction::new(&generation, request(), runtime).unwrap();
    let mut sink = collector(
        &generation,
        &mut tx,
        runtime,
        EvaluationMode::Accelerated,
        ResultLimits::default(),
    );
    sink.collect_scalar(b"json.a", b"HIT", JsonScalarKind::String).unwrap();
    assert_eq!(scope.counts().get(Event::FullScans), 1);
    assert_eq!(scope.counts().get(Event::CaptureBooleanSearches), 1);
    assert_eq!(sink.usage().live_outcomes, 2);
    assert_eq!(scope.counts().get(Event::CaptureScratchInitializations), 2);
}

#[test]
fn capture_hit_epoch_wrap_clears_old_selected_peer_proof() {
    let generation = compile(concat!(
        "SecRule ARGS \"@rx (evil)\" \"id:1,phase:2,pass,capture\"\n",
        "SecRule ARGS \"@rx (attack)\" \"id:2,phase:2,pass,capture\"\n",
    ));
    let runtime = RuntimeLimits::default();
    let scope = measurements::start();
    let mut tx = Transaction::new(&generation, request(), runtime).unwrap();
    let mut sink = collector(
        &generation,
        &mut tx,
        runtime,
        EvaluationMode::Accelerated,
        ResultLimits::default(),
    );
    let candidate = sink
        .state
        .store
        .begin_candidate(b"json.a", 4, JsonScalarKind::String, None)
        .unwrap();
    sink.state.dispatch.epoch = usize::MAX;
    sink.state.dispatch.stamps.fill(1);
    let mut groups = [GroupRun { id: 0, terminal: false }];
    let mut observer = ProfileObserver {
        fixed_scratch: sink.state.dispatch.bytes().unwrap(),
        store: &mut sink.state.store,
        context: &mut sink.context,
        groups: &mut groups,
        cache: &mut sink.state.dispatch.cache,
        hit_eligible: &mut sink.state.dispatch.hit_eligible,
        stamps: &mut sink.state.dispatch.stamps,
        epoch: &mut sink.state.dispatch.epoch,
        captures: &mut sink.state.captures,
        candidate,
        profile: 0,
        runtime,
        mode: EvaluationMode::Accelerated,
        transform_bytes: 0,
        last: StoredCheckpoint::Initial,
        step: 0,
    };
    observer.stage(0, b"safe").unwrap();
    assert_eq!(*observer.epoch, 1);
    assert!(!observer.hit_eligible[0]);
    assert_eq!(scope.counts().get(Event::FullScans), 0);
    assert_eq!(scope.counts().get(Event::CaptureBooleanSearches), 1);
}

#[test]
#[ignore = "release timing witness; run explicitly with --release and --nocapture"]
fn capture_hit_release_timing_witness() {
    use crate::seclang::BodyProcessorSelection;
    use bytes::Bytes;
    let rules = (0..11)
        .map(|id| {
            format!(
                "SecRule ARGS \"@rx (?P<word{id}>attack{id}(?:x|y)+)\" \"id:{},phase:2,pass,capture\"\n",
                id + 1
            )
        })
        .collect::<String>();
    let generation = compile(&rules);
    assert_eq!(generation.capture_hits.matchers.len(), 1);
    let optional_retained = generation
        .capture_hits
        .matchers
        .iter()
        .map(|m| m.memory_usage())
        .sum::<usize>();
    for value_bytes in [32, 1024, 8192] {
        let value = "s".repeat(value_bytes);
        let input = format!(
            "{{{}}}",
            (0..64)
                .map(|id| format!("\"k{id}\":\"{value}\""))
                .collect::<Vec<_>>()
                .join(",")
        );
        for mode in [EvaluationMode::Accelerated, EvaluationMode::Reference] {
            let mut samples = Vec::new();
            let mut scratch_envelope = 0;
            for sample in 0..24 {
                let start = std::time::Instant::now();
                let mut tx = Transaction::new(&generation, request(), RuntimeLimits::default()).unwrap();
                tx.evaluation_mode = mode;
                scratch_envelope = tx.scratch_bytes;
                tx.execute_request_headers().unwrap();
                tx.publish_request_body(RequestBody::new(
                    Bytes::copy_from_slice(input.as_bytes()),
                    BodyProcessorSelection::Json,
                ))
                .unwrap();
                assert!(tx.execute_request_body().unwrap().findings().is_empty());
                let elapsed = start.elapsed().as_nanos();
                if sample >= 3 {
                    samples.push(elapsed);
                }
            }
            samples.sort_unstable();
            println!("CAPTURE_HIT_TIMING,mode={mode:?},rules=11,fields=64,value_bytes={value_bytes},samples=21,p50_ns={},optional_engine_bytes={optional_retained},scratch_envelope_bytes={scratch_envelope}", samples[samples.len() / 2]);
        }
    }
}

#[test]
fn stage_scan_cache_selects_inline_or_sparse_overflow_at_namespace_boundary() {
    let mut admissions = Vec::new();
    for namespace in [58, 64, 65] {
        let mut source = (0..namespace - 1)
            .map(|index| {
                format!(
                    "SecRule REQUEST_HEADERS:X-{index} \"@pm needle\" \"id:{},phase:1,pass,t:none\"\n",
                    100 + index
                )
            })
            .collect::<String>();
        source.push_str("SecRule ARGS:json.x \"@pm needle\" \"id:1,phase:2,pass,t:none\"\n");
        let generation = compile(&source);
        assert_eq!(generation.matcher_count(), namespace);
        assert_eq!(generation.json_inspection.matcher_scan_capacity, 1);
        let mut outcomes = Vec::new();
        for mode in [EvaluationMode::Accelerated, EvaluationMode::Reference] {
            let runtime = RuntimeLimits::default();
            let mut tx = Transaction::new(&generation, request(), runtime).unwrap();
            let mut sink = collector(&generation, &mut tx, runtime, mode, ResultLimits::default());
            admissions.push((sink.usage().work, sink.usage().retained_bytes));
            let capacity = sink.state.dispatch.cache.heap_capacity();
            assert_eq!(capacity, usize::from(namespace > 64));
            assert_eq!(sink.state.dispatch.cache.len(), 0);
            feed(&mut sink, br#"{"x":"safe","x":"needle"}"#, 9);
            assert_eq!(sink.state.dispatch.cache.scans, 2);
            assert_eq!(sink.state.dispatch.cache.heap_capacity(), capacity);
            assert_eq!(sink.usage().live_outcomes, 1);
            assert_eq!(sink.state.dispatch.cache.len(), 1);
            assert!(sink.state.dispatch.cache.contains(namespace - 1));
            assert!(!sink.state.dispatch.cache.contains(0));
            outcomes.push((
                sink.usage().work,
                sink.usage().live_candidates,
                sink.usage().live_changes,
            ));
        }
        assert_eq!(outcomes[0], outcomes[1]);
    }
    assert_eq!(admissions[0], admissions[2]);
    assert_eq!(admissions[4].0, admissions[2].0);
    assert_eq!(admissions[4].1, admissions[2].1 + std::mem::size_of::<usize>());
}
