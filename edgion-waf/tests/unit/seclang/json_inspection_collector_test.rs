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
        assert_eq!(sealed.summary.argument_count, 2);
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
    for mode in [EvaluationMode::Accelerated, EvaluationMode::Reference] {
        let runtime = RuntimeLimits::default();
        let mut tx = Transaction::new(&generation, request(), runtime).unwrap();
        let mut sink = collector(&generation, &mut tx, runtime, mode, ResultLimits::default());
        feed(&mut sink, br#"{"a":"SAFE","b":"safe"}"#, 9);
        assert_eq!(sink.dispatch.cache.scans, 3);
        assert_eq!(sink.usage().live_outcomes, 0);
        assert_eq!(sink.usage().live_changes, 1);
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
    assert_eq!((capture.key, capture.value), (b"0".as_slice(), b"secret123".as_slice()));
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
        scanned: Vec::with_capacity(1),
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
    assert!(cache.scanned.is_empty());
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
    assert!(cache.scanned.is_empty());
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
        self.store.usage()
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
