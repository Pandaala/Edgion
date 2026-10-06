use super::*;
use crate::seclang::{
    analyze_one_semantic_draft, parse_syntax, scan_source, validate_semantic_draft, PhaseDefaults, SemanticLimits,
    SemanticValidationContext, SourceLimits, SyntaxLimits, ValidationLimits,
};
use crate::seclang::{compile_validated, CompilerError, CompilerLimits};

fn compile(text: &str, limits: CompilerLimits) -> std::result::Result<CompiledGeneration, CompilerError> {
    let source = scan_source("json-plan-test", text.as_bytes(), SourceLimits::default()).unwrap();
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
    compile_validated(validated, None, limits)
}

fn generation() -> CompiledGeneration {
    compile(
        r#"SecRule ARGS "@contains blocked" "id:1,phase:2,t:none,t:lowercase,pass""#,
        CompilerLimits::default(),
    )
    .unwrap()
}
fn store(g: &CompiledGeneration) -> ResultStore<'_> {
    ResultStore::new_domain(
        GenerationHandle::Borrowed(g),
        RuntimeLimits::default(),
        EvaluationMode::Accelerated,
        ResultLimits::default(),
        InspectionDomain::Json,
    )
    .unwrap()
}
fn finish(s: &mut ResultStore<'_>, id: CandidateId, profiles: &[usize]) -> Receipt {
    let proof = s.completion_proof(id, profiles).unwrap();
    s.complete_candidate(proof).unwrap()
}

#[test]
fn selected_lookup_returns_the_checked_profile_with_two_implicit_reads() {
    let g = generation();
    let mut s = store(&g);
    let id = s.begin_candidate(b"a", 4, JsonScalarKind::String, None).unwrap();
    s.complete_profile(id, 0).unwrap();
    let receipt = finish(&mut s, id, &[0]);
    s.seal(Some(receipt), JsonSemanticSummary::default()).unwrap();
    let (sealed, mut budget) = s.into_sealed().unwrap();
    let before = budget.usage().work;
    let (outcome, profile) = sealed
        .lookup_selected(
            &mut budget,
            id,
            0,
            &g,
            RuntimeLimits::default(),
            EvaluationMode::Accelerated,
        )
        .unwrap();
    assert_eq!(outcome, SelectedOutcome::NoMatch);
    assert!(profile.record.is_none());
    assert!(profile.first_change.is_none());
    assert!(profile.error.is_none());
    assert_eq!(budget.usage().work - before, 2);
}

#[test]
fn summary_no_hit_requires_completed_sole_profile_and_exact_survivor_count() {
    let g = generation();
    assert_eq!(g.json_inspection.summary_profile(), Some(0));
    for (complete, count, expected) in [(true, 1, true), (false, 1, false), (true, 0, false)] {
        let mut s = store(&g);
        let id = s.begin_candidate(b"json.a", 4, JsonScalarKind::String, None).unwrap();
        if complete {
            s.complete_profile(id, 0).unwrap();
        }
        let receipt = finish(&mut s, id, if complete { &[0] } else { &[] });
        s.seal(
            Some(receipt),
            JsonSemanticSummary {
                argument_count: count,
                argument_value_bytes: 4,
                ..JsonSemanticSummary::default()
            },
        )
        .unwrap();
        let (results, budget) = s.into_sealed().unwrap();
        assert_eq!(
            results.admits_summary_no_hit(&budget, &g, RuntimeLimits::default(), EvaluationMode::Accelerated, 7),
            expected
        );
        assert!(!results.admits_summary_no_hit(&budget, &g, RuntimeLimits::default(), EvaluationMode::Reference, 7));
        assert!(!results.admits_summary_no_hit(
            &budget,
            &g,
            RuntimeLimits {
                max_selected_values: 1,
                ..RuntimeLimits::default()
            },
            EvaluationMode::Accelerated,
            7
        ));
        let other = generation();
        assert!(!results.admits_summary_no_hit(
            &budget,
            &other,
            RuntimeLimits::default(),
            EvaluationMode::Accelerated,
            7
        ));
        let foreign = InspectionBudget::empty_publication(ResultLimits::default()).unwrap();
        assert!(!results.admits_summary_no_hit(&foreign, &g, RuntimeLimits::default(), EvaluationMode::Accelerated, 7));
    }
}

#[test]
fn summary_no_hit_rejects_reclaimed_profile_and_change_slots() {
    let g = generation();
    let mut s = store(&g);
    let old = s.begin_candidate(b"json.a", 4, JsonScalarKind::String, None).unwrap();
    s.record_changed_step(old, 0, 0, 4).unwrap();
    s.complete_profile(old, 0).unwrap();
    let old_receipt = finish(&mut s, old, &[0]);
    s.discard(old_receipt).unwrap();
    assert_eq!(s.usage().live_profiles, 0);
    assert_eq!(s.usage().live_changes, 0);
    let id = s.begin_candidate(b"json.a", 4, JsonScalarKind::String, None).unwrap();
    s.complete_profile(id, 0).unwrap();
    let receipt = finish(&mut s, id, &[0]);
    s.seal(
        Some(receipt),
        JsonSemanticSummary {
            argument_count: 1,
            argument_value_bytes: 4,
            ..JsonSemanticSummary::default()
        },
    )
    .unwrap();
    let (results, mut budget) = s.into_sealed().unwrap();
    assert!(!results.admits_summary_no_hit(&budget, &g, RuntimeLimits::default(), EvaluationMode::Accelerated, 7));
    budget.work(usize::MAX).unwrap_err();
    assert!(!results.admits_summary_no_hit(&budget, &g, RuntimeLimits::default(), EvaluationMode::Accelerated, 0));
}
#[test]
fn wide_no_hit_storage_uses_sparse_profile_records() {
    let mut source = String::new();
    for i in 0..97 {
        let transforms = ",t:lowercase".repeat(i % 39);
        let target = if i == 96 { "ARGS" } else { "ARGS|ARGS_NAMES" };
        source.push_str(&format!(
            "SecRule {target} \"@contains blocked-{i}\" \"id:{},phase:2{transforms},t:lowercase,pass\"\n",
            i + 1
        ));
    }
    let g = compile(&source, CompilerLimits::default()).unwrap();
    assert_eq!(g.json_inspection.groups.len(), 193);
    assert_eq!(g.json_inspection.profiles.len(), 78);
    let profiles: Vec<_> = (0..78).collect();
    for changed in [0, 6] {
        let mut s = store(&g);
        let mut receipt = None;
        for i in 0..10_000 {
            let id = s
                .begin_candidate(format!("json.field{i:05}").as_bytes(), 4, JsonScalarKind::String, None)
                .unwrap();
            for profile in 0..78 {
                if profile < changed {
                    s.record_changed_step(id, profile, 0, 4).unwrap();
                }
                s.complete_profile(id, profile).unwrap();
            }
            let current = finish(&mut s, id, &profiles);
            receipt = s.concat(receipt, Some(current)).unwrap();
        }
        s.seal(receipt, JsonSemanticSummary::default()).unwrap();
        let usage = s.usage();
        eprintln!("10k x 78, changed={changed}: {usage:?}");
        assert_eq!(usage.live_profiles, 10_000 * changed);
        assert_eq!(usage.live_changes, 10_000 * changed);
        assert_eq!(
            usage.live_outcomes + usage.matched_bytes + usage.capture_bytes + usage.deferred_bytes,
            0
        );
        assert!(usage.peak_retained_bytes <= 32 << 20);
        let (sealed, mut budget) = s.into_sealed().unwrap();
        assert!(matches!(
            sealed
                .lookup_selected(
                    &mut budget,
                    receipt.unwrap().head,
                    0,
                    &g,
                    RuntimeLimits::default(),
                    EvaluationMode::Accelerated
                )
                .map(|(outcome, _)| outcome),
            Ok(SelectedOutcome::NoMatch)
        ));
    }
}
#[test]
fn sealed_captures_and_changes_preserve_order() {
    let g = generation();
    let mut s = store(&g);
    let id = s.begin_candidate(b"json.x", 7, JsonScalarKind::String, None).unwrap();
    s.record_changed_step(id, 0, 1, 7).unwrap();
    s.record_changed_step(id, 0, 75, 7).unwrap();
    let outcome = s.record_match(id, 0, 2, b"blocked").unwrap();
    for key in [b"0", b"1", b"2"] {
        s.add_capture(outcome, key, b"hit").unwrap();
    }
    s.complete_profile(id, 0).unwrap();
    let receipt = finish(&mut s, id, &[0]);
    s.seal(Some(receipt), JsonSemanticSummary::default()).unwrap();
    let (sealed, mut budget) = s.into_sealed().unwrap();
    let p = sealed.profile(&mut budget, id, 0).unwrap();
    assert!(p.record.is_some());
    assert!(p.error.is_none());
    let first = sealed
        .change(&mut budget, id, p.record.unwrap(), p.first_change.unwrap())
        .unwrap();
    assert_eq!(sealed.records.changes.get(p.first_change.unwrap().0).unwrap().step, 1);
    sealed
        .change(&mut budget, id, p.record.unwrap(), first.next.unwrap())
        .unwrap();
    assert_eq!(sealed.records.changes.get(first.next.unwrap().0).unwrap().step, 75);
    let OutcomeValue::Match {
        value, first_capture, ..
    } = sealed.outcome(&mut budget, id, outcome).unwrap().result
    else {
        panic!("expected match")
    };
    assert_eq!(sealed.match_value(&mut budget, value).unwrap(), b"blocked");
    let mut next = first_capture;
    for key in [b"0", b"1", b"2"] {
        let capture = sealed.capture(&mut budget, id, outcome, next.unwrap()).unwrap();
        assert_eq!(sealed.match_value(&mut budget, capture.key).unwrap(), key);
        assert_eq!(sealed.match_value(&mut budget, capture.value).unwrap(), b"hit");
        next = capture.next;
    }
    assert!(next.is_none());
}
#[test]
fn late_transform_error_reclaims_payload_and_overrides_hits() {
    let g = generation();
    let mut s = store(&g);
    let id = s.begin_candidate(b"json.x", 7, JsonScalarKind::String, None).unwrap();
    let outcome = s.record_match(id, 0, 0, b"blocked").unwrap();
    s.add_capture(outcome, b"0", b"blocked").unwrap();
    let error = CheckpointError {
        step: 1,
        checkpoint: 2,
        error: RuntimeError::SelectedValueLimit { limit: 1 },
    };
    s.record_transform_error(id, 0, error).unwrap();
    assert_eq!(
        s.usage().matched_bytes + s.usage().capture_bytes + s.usage().live_outcomes,
        0
    );
    let receipt = finish(&mut s, id, &[0]);
    s.seal(Some(receipt), JsonSemanticSummary::default()).unwrap();
    let (sealed, mut budget) = s.into_sealed().unwrap();
    assert_eq!(
        sealed
            .lookup_selected(
                &mut budget,
                id,
                0,
                &g,
                RuntimeLimits::default(),
                EvaluationMode::Accelerated
            )
            .map(|(outcome, _)| outcome)
            .unwrap(),
        SelectedOutcome::TransformError(error)
    );
}
#[test]
fn stale_generation_and_unseen_profile_fail_closed() {
    let g = generation();
    let mut s = store(&g);
    let old = s.begin_candidate(b"a", 0, JsonScalarKind::Null, None).unwrap();
    let receipt = finish(&mut s, old, &[]);
    s.discard(receipt).unwrap();
    let new = s.begin_candidate(b"b", 0, JsonScalarKind::Null, None).unwrap();
    assert_eq!(old.0.slot, new.0.slot);
    assert_ne!(old.0.generation, new.0.generation);
    assert!(matches!(s.records.candidates.get(old.0), Err(StoreError::Identity)));
    assert!(matches!(s.completion_proof(new, &[0]), Err(StoreError::State)));
    assert_eq!(s.usage().abandoned, 1);
    s.abandon();
    assert_eq!(s.usage().abandoned, 1);
}
#[test]
fn sealed_budget_failure_is_sticky_and_external_scratch_survives_abandon() {
    let g = generation();
    let mut s = store(&g);
    s.scratch_capacity(64).unwrap();
    s.abandon();
    assert_eq!(s.usage().retained_bytes, 64);
    s.budget.release_scratch();
    assert_eq!(s.usage().retained_bytes, 0);
    let mut s = store(&g);
    s.seal(None, JsonSemanticSummary::default()).unwrap();
    let (_, mut budget) = s.into_sealed().unwrap();
    assert_eq!(budget.work(usize::MAX), Err(StoreError::Work));
    assert_eq!(budget.work(1), Err(StoreError::Work));
    assert_eq!(budget.work(0), Err(StoreError::Work));
}

#[test]
fn byte_pool_reuses_rounded_classes_and_rejects_overflow() {
    let g = generation();
    let mut s = store(&g);
    for len in [0, 1, 7, 8, 9, 15, 16, 17, 65] {
        let bytes = vec![b'x'; len];
        let first = s.records.bytes.copy(&bytes, &mut s.budget).unwrap();
        assert_eq!(s.records.bytes.read(first).unwrap(), bytes);
        let retained = s.usage().retained_bytes;
        s.records.bytes.free(first, &mut s.budget).unwrap();
        let second = s.records.bytes.copy(&bytes, &mut s.budget).unwrap();
        assert_eq!(first.offset, second.offset);
        assert_eq!(s.usage().retained_bytes, retained);
        s.records.bytes.free(second, &mut s.budget).unwrap();
    }
    assert!(matches!(
        s.records.bytes.alloc(usize::MAX, &mut s.budget),
        Err(StoreError::Arithmetic)
    ));
}
#[test]
fn record_capacity_named_limits_and_generation_exhaustion_are_bounded() {
    let g = generation();
    let mut s = store(&g);
    s.budget.limits.candidates = 1;
    s.begin_candidate(b"a", 1, JsonScalarKind::String, Some(b"x")).unwrap();
    assert_eq!(s.records.candidates.slots.capacity(), 1);
    assert!(matches!(
        s.begin_candidate(b"b", 0, JsonScalarKind::Null, None),
        Err(StoreError::Candidates)
    ));
    assert_eq!(s.usage().retained_bytes, 0);
    let mut s = store(&g);
    s.budget.limits.deferred_entries = 0;
    assert!(matches!(
        s.begin_candidate(b"a", 1, JsonScalarKind::String, Some(b"x")),
        Err(StoreError::Deferred)
    ));
    let mut s = store(&g);
    let id = s.begin_candidate(b"a", 0, JsonScalarKind::Null, None).unwrap();
    s.records.candidates.slots[id.0.slot as usize].generation = u32::MAX;
    let exhausted = CandidateId(RawId {
        generation: u32::MAX,
        ..id.0
    });
    let receipt = finish(&mut s, exhausted, &[]);
    assert_eq!(s.discard(receipt), Err(StoreError::Generation));
    assert_eq!(s.usage().retained_bytes, 0);
    let _ = id;
}

#[test]
fn same_generation_stores_reject_foreign_handles_and_budgets_stickily() {
    let g = generation();
    let mut first = store(&g);
    let mut second = store(&g);
    let a = first.begin_candidate(b"a", 0, JsonScalarKind::Null, None).unwrap();
    let b = second.begin_candidate(b"b", 0, JsonScalarKind::Null, None).unwrap();
    assert_eq!(a.0.slot, b.0.slot);
    assert_eq!(a.0.generation, b.0.generation);
    assert_ne!(a.0.owner, b.0.owner);
    let receipt = finish(&mut first, a, &[]);
    first.seal(Some(receipt), JsonSemanticSummary::default()).unwrap();
    let (sealed, mut budget) = first.into_sealed().unwrap();
    assert!(matches!(sealed.candidate(&mut budget, b), Err(StoreError::Identity)));
    assert!(matches!(sealed.candidate(&mut budget, a), Err(StoreError::Identity)));
    assert!(matches!(
        sealed.candidate(&mut second.budget, a),
        Err(StoreError::Identity)
    ));
    assert_eq!(second.budget.work(0), Err(StoreError::Identity));
    let counter = AtomicU64::new(u64::MAX);
    assert_eq!(allocate_owner(&counter), Err(StoreError::Generation));
    assert_eq!(counter.load(Ordering::Relaxed), u64::MAX);
}

#[test]
fn missing_record_never_becomes_no_match_and_failed_seal_preserves_error() {
    let g = generation();
    let mut s = store(&g);
    let id = s.begin_candidate(b"a", 1, JsonScalarKind::String, None).unwrap();
    s.record_changed_step(id, 0, 1, 1).unwrap();
    s.complete_profile(id, 0).unwrap();
    let receipt = finish(&mut s, id, &[0]);
    s.records.candidates.get_mut(id.0).unwrap().profiles = None;
    s.seal(Some(receipt), JsonSemanticSummary::default()).unwrap();
    let (sealed, mut budget) = s.into_sealed().unwrap();
    assert!(matches!(sealed.profile(&mut budget, id, 0), Err(StoreError::Identity)));
    assert!(matches!(sealed.candidate(&mut budget, id), Err(StoreError::Identity)));
    let mut s = store(&g);
    s.budget.limits.work = 0;
    assert_eq!(s.charge_work(1), Err(StoreError::Work));
    assert!(matches!(s.into_sealed(), Err(StoreError::Work)));
}

#[test]
fn one_sided_concat_validates_foreign_and_stale_receipts_and_sticks() {
    let g = generation();
    for left in [false, true] {
        let mut owner = store(&g);
        let id = owner.begin_candidate(b"a", 0, JsonScalarKind::Null, None).unwrap();
        let receipt = finish(&mut owner, id, &[]);
        let mut foreign = store(&g);
        let result = if left {
            foreign.concat(Some(receipt), None)
        } else {
            foreign.concat(None, Some(receipt))
        };
        assert!(matches!(result, Err(StoreError::Identity)));
        assert!(matches!(foreign.concat(None, None), Err(StoreError::Identity)));
        owner.discard(receipt).unwrap();
        assert!(matches!(owner.concat(None, Some(receipt)), Err(StoreError::Identity)));
        assert!(matches!(owner.concat(None, None), Err(StoreError::Identity)));
    }
}

#[test]
fn named_first_index_growth_and_work_failures_are_sticky() {
    let g = compile(
        r#"SecAction "id:1,phase:2,pass,setvar:'tx.value=%{ARGS.json.a}'""#,
        CompilerLimits::default(),
    )
    .unwrap();
    for fail_work in [false, true] {
        let mut s = store(&g);
        let id = s
            .begin_candidate(b"json.a", 1, JsonScalarKind::String, Some(b"x"))
            .unwrap();
        let receipt = finish(&mut s, id, &[]);
        if fail_work {
            s.budget.limits.work = s.usage().work;
        } else {
            s.budget.limits.retained_bytes = s.usage().retained_bytes;
        }
        let expected = if fail_work {
            StoreError::Work
        } else {
            StoreError::Capacity
        };
        assert_eq!(s.seal(Some(receipt), JsonSemanticSummary::default()), Err(expected));
        assert_eq!(s.seal(Some(receipt), JsonSemanticSummary::default()), Err(expected));
        assert_eq!(s.usage().retained_bytes, 0);
        assert!(s.records.named_first.is_empty());
        assert!(matches!(s.into_sealed(), Err(error) if error == expected));
    }
}

impl InspectionBudget {
    pub(in crate::seclang::compiler::transaction) fn usage(&self) -> StoreUsage {
        self.usage
    }
}
impl ResultStore<'_> {
    pub(in crate::seclang::compiler::transaction) fn usage(&self) -> StoreUsage {
        self.budget.usage
    }
}

fn sharing_generation() -> CompiledGeneration {
    compile(
        concat!(
            "SecRule ARGS \"@contains a\" \"id:1,phase:2,pass,t:none\"\n",
            "SecRule ARGS \"@contains b\" \"id:2,phase:2,pass,t:none\"\n",
            "SecRule ARGS \"@contains c\" \"id:3,phase:2,pass,t:none,t:lowercase\"\n",
        ),
        CompilerLimits::default(),
    )
    .unwrap()
}

#[test]
fn shared_stage_payload_bulk_cleanup_is_order_independent() {
    let g = sharing_generation();
    for owner_first in [false, true] {
        for transform_error in [false, true] {
            let mut s = store(&g);
            let id = s.begin_candidate(b"json.x", 7, JsonScalarKind::String, None).unwrap();
            let first = s.record_match(id, 0, 0, b"blocked").unwrap();
            let token = s.stage_match_token(first).unwrap();
            let alias = s.record_shared_match(id, 1, 0, token).unwrap();
            s.add_capture(first, b"0", b"first").unwrap();
            s.add_capture_range(first, b"1", 0, 7).unwrap();
            s.add_capture(alias, b"0", b"second").unwrap();
            s.add_capture_range(alias, b"1", 2, 3).unwrap();
            assert_eq!(s.usage().matched_bytes, 7);
            assert_eq!(s.usage().live_outcomes, 2);
            if owner_first {
                let local = LocalOutcomeId::from_full(first, s.records.outcomes.owner).unwrap();
                s.records.candidates.get_mut(id.0).unwrap().outcomes = Some(local);
                s.records.outcomes.get_mut(first.0).unwrap().next = Some(alias);
                s.records.outcomes.get_mut(alias.0).unwrap().next = None;
            }
            if transform_error {
                s.record_transform_error(
                    id,
                    0,
                    CheckpointError {
                        step: 1,
                        checkpoint: 2,
                        error: RuntimeError::SelectedValueLimit { limit: 1 },
                    },
                )
                .unwrap();
            } else {
                s.complete_profile(id, 0).unwrap();
                let receipt = finish(&mut s, id, &[0]);
                s.discard(receipt).unwrap();
            }
            assert_eq!(
                s.usage().matched_bytes + s.usage().capture_bytes + s.usage().live_outcomes,
                0
            );
            assert_eq!(s.usage().live_captures, 0);
        }
    }
}

#[test]
fn shared_stage_token_rejects_cross_scope_and_stale_reuse() {
    let g = sharing_generation();
    for case in 0..5 {
        let mut s = store(&g);
        let id = s.begin_candidate(b"json.x", 3, JsonScalarKind::String, None).unwrap();
        let first = s.record_match(id, 0, 0, b"abc").unwrap();
        let token = s.stage_match_token(first).unwrap();
        let result = match case {
            0 => {
                let other = s.begin_candidate(b"json.y", 3, JsonScalarKind::String, None).unwrap();
                s.record_shared_match(other, 1, 0, token)
            }
            1 => s.record_shared_match(id, 2, 0, token),
            2 => s.record_shared_match(id, 1, 1, token),
            3 => {
                s.complete_profile(id, 0).unwrap();
                let receipt = finish(&mut s, id, &[0]);
                s.discard(receipt).unwrap();
                let replacement = s.begin_candidate(b"json.x", 3, JsonScalarKind::String, None).unwrap();
                s.record_match(replacement, 0, 0, b"abc").unwrap();
                s.record_shared_match(replacement, 1, 0, token)
            }
            _ => {
                let mut other = store(&g);
                let candidate = other
                    .begin_candidate(b"json.x", 3, JsonScalarKind::String, None)
                    .unwrap();
                other.record_shared_match(candidate, 1, 0, token)
            }
        };
        assert!(matches!(result, Err(StoreError::Identity)));
    }
}

#[test]
fn shared_payload_preserves_sealed_values_and_independent_captures() {
    let g = sharing_generation();
    let mut s = store(&g);
    let id = s.begin_candidate(b"json.x", 3, JsonScalarKind::String, None).unwrap();
    let first = s.record_match(id, 0, 0, b"abc").unwrap();
    let token = s.stage_match_token(first).unwrap();
    let second = s.record_shared_match(id, 1, 0, token).unwrap();
    for (outcome, value) in [(first, b"one"), (second, b"two")] {
        s.add_capture(outcome, b"0", value).unwrap();
    }
    s.complete_profile(id, 0).unwrap();
    let receipt = finish(&mut s, id, &[0]);
    s.seal(Some(receipt), JsonSemanticSummary::default()).unwrap();
    let (sealed, mut budget) = s.into_sealed().unwrap();
    let mut shared_value = None;
    for (outcome, expected) in [(first, b"one"), (second, b"two")] {
        let OutcomeValue::Match {
            value, first_capture, ..
        } = sealed.outcome(&mut budget, id, outcome).unwrap().result
        else {
            panic!("expected match")
        };
        assert_eq!(sealed.match_value(&mut budget, value).unwrap(), b"abc");
        if let Some(previous) = shared_value {
            assert_eq!(value, previous);
        }
        shared_value = Some(value);
        let capture = sealed
            .capture(&mut budget, id, outcome, first_capture.unwrap())
            .unwrap();
        assert_eq!(sealed.match_value(&mut budget, capture.value).unwrap(), expected);
    }
}

#[test]
fn sealed_match_handles_reject_foreign_ranges_and_latch_work_failure() {
    let g = generation();
    let seal = || {
        let mut s = store(&g);
        let id = s.begin_candidate(b"json.x", 7, JsonScalarKind::String, None).unwrap();
        let outcome = s.record_match(id, 0, 0, b"blocked").unwrap();
        s.complete_profile(id, 0).unwrap();
        let receipt = finish(&mut s, id, &[0]);
        s.seal(Some(receipt), JsonSemanticSummary::default()).unwrap();
        let (sealed, mut budget) = s.into_sealed().unwrap();
        let OutcomeValue::Match { value, .. } = sealed.outcome(&mut budget, id, outcome).unwrap().result else {
            panic!("expected match")
        };
        (sealed, budget, value)
    };
    let (original, mut original_budget, foreign) = seal();
    for case in 0..5 {
        let (sealed, mut budget, local) = seal();
        let (value, expected) = match case {
            0 => (foreign, StoreError::Identity),
            1 => (
                SealedMatchValue {
                    offset: sealed.records.bytes.bytes.len(),
                    ..local
                },
                StoreError::Identity,
            ),
            2 => (
                SealedMatchValue {
                    offset: usize::MAX,
                    ..local
                },
                StoreError::Arithmetic,
            ),
            3 => {
                budget.limits.work = budget.usage.work;
                (local, StoreError::Work)
            }
            _ => {
                budget.owner = foreign.owner;
                (local, StoreError::Identity)
            }
        };
        assert_eq!(sealed.match_value(&mut budget, value).unwrap_err(), expected);
        assert_eq!(budget.failure(), Some(expected));
        assert!(sealed.match_value(&mut budget, local).is_err());
    }
    assert_eq!(original.match_value(&mut original_budget, foreign).unwrap(), b"blocked");
}

#[test]
fn empty_sealed_match_value_resolves_without_payload_allocation() {
    let g = generation();
    let mut s = store(&g);
    let id = s.begin_candidate(b"json.x", 0, JsonScalarKind::String, None).unwrap();
    let outcome = s.record_match(id, 0, 0, b"").unwrap();
    s.complete_profile(id, 0).unwrap();
    let receipt = finish(&mut s, id, &[0]);
    s.seal(Some(receipt), JsonSemanticSummary::default()).unwrap();
    let (sealed, mut budget) = s.into_sealed().unwrap();
    let OutcomeValue::Match { value, .. } = sealed.outcome(&mut budget, id, outcome).unwrap().result else {
        panic!("expected match")
    };
    assert_eq!(value.len(), 0);
    assert!(sealed.match_value(&mut budget, value).unwrap().is_empty());
    assert_eq!(budget.usage().matched_bytes, 0);
}

#[test]
fn shared_outcomes_still_obey_record_work_and_capacity_limits() {
    let g = sharing_generation();
    for expected in [StoreError::Work, StoreError::Records, StoreError::Capacity] {
        let mut s = store(&g);
        let id = s.begin_candidate(b"json.x", 3, JsonScalarKind::String, None).unwrap();
        let first = s.record_match(id, 0, 0, b"abc").unwrap();
        let token = s.stage_match_token(first).unwrap();
        match expected {
            StoreError::Work => s.budget.limits.work = s.usage().work,
            StoreError::Records => s.budget.limits.records = s.usage().allocated_records,
            StoreError::Capacity => s.budget.limits.retained_bytes = s.usage().retained_bytes,
            _ => unreachable!(),
        }
        // Reuse any already-admitted record slots before requiring growth.
        let result = loop {
            match s.record_shared_match(id, 1, 0, token) {
                Ok(_) => {
                    assert!(s.usage().live_outcomes <= 16);
                    assert_eq!(s.usage().matched_bytes, 3);
                }
                Err(error) => break error,
            }
        };
        assert_eq!(result, expected);
        assert_eq!(s.usage().matched_bytes, 0);
        assert_eq!(s.record_shared_match(id, 1, 0, token), Err(expected));
    }
}

#[test]
fn byte_pool_large_page_boundaries_and_sorted_insertion_keep_leases_stable() {
    for (len, size) in [
        (0, 0),
        (4095, 4096),
        (4096, 4096),
        (4097, 8192),
        (8191, 8192),
        (8192, 8192),
        (8193, 12288),
        (12289, 16384),
    ] {
        assert_eq!(BytePool::allocation_size(len), Ok(size));
    }
    assert_eq!(BytePool::allocation_size(usize::MAX), Err(StoreError::Arithmetic));
    assert_eq!(BytePool::allocation_size(usize::MAX - 4095), Ok(usize::MAX - 4095));
    let mut budget = InspectionBudget::empty_publication(ResultLimits::default()).unwrap();
    let mut pool = BytePool::new(budget.owner);
    assert_eq!(pool.heads.len(), 13);
    assert_eq!(pool.large_heads.capacity(), 0);
    let large = pool.copy(&vec![b'z'; 12289], &mut budget).unwrap();
    pool.free(large, &mut budget).unwrap();
    let middle = pool.copy(&vec![b'm'; 8193], &mut budget).unwrap();
    let small = pool.copy(&vec![b's'; 4097], &mut budget).unwrap();
    assert_eq!(
        pool.large_heads.iter().map(|class| class.size).collect::<Vec<_>>(),
        [8192, 12288, 16384]
    );
    assert_eq!(pool.read(middle).unwrap(), vec![b'm'; 8193]);
    assert_eq!(pool.read(small).unwrap(), vec![b's'; 4097]);
    let retained = budget.usage.retained_bytes;
    let reused = pool.copy(&vec![b'r'; 12290], &mut budget).unwrap();
    assert_eq!(reused.offset, large.offset);
    assert_eq!(reused.size, large.size);
    assert_eq!(budget.usage.retained_bytes, retained);
    // Free pre-insertion leases after their class indices have moved.
    pool.free(middle, &mut budget).unwrap();
    pool.free(small, &mut budget).unwrap();
    assert_eq!(pool.alloc(9000, &mut budget).unwrap().offset, middle.offset);
    assert_eq!(pool.alloc(5000, &mut budget).unwrap().offset, small.offset);
    assert_eq!(
        budget.usage.retained_bytes,
        pool.bytes.capacity() + pool.large_heads.capacity() * size_of::<LargeClass>()
    );
}

#[test]
fn byte_pool_large_heads_capacity_work_and_owner_failures_are_sticky() {
    for failure in [
        StoreError::Capacity,
        StoreError::Work,
        StoreError::Identity,
        StoreError::Arithmetic,
    ] {
        let mut budget = InspectionBudget::empty_publication(ResultLimits::default()).unwrap();
        let mut pool = BytePool::new(budget.owner);
        let lease = pool.alloc(8193, &mut budget).unwrap();
        pool.free(lease, &mut budget).unwrap();
        if failure == StoreError::Capacity {
            for len in [12289, 16385, 20481] {
                let lease = pool.alloc(len, &mut budget).unwrap();
                pool.free(lease, &mut budget).unwrap();
            }
            assert_eq!(pool.large_heads.len(), pool.large_heads.capacity());
        }
        let heads = pool.large_heads.len();
        let bytes = pool.bytes.len();
        let expected = match failure {
            StoreError::Capacity => {
                budget.limits.retained_bytes = budget.usage.retained_bytes;
                pool.alloc(4097, &mut budget).map(|_| ())
            }
            StoreError::Work => {
                // One comparison is allowed; insertion/shift work must fail before mutation.
                budget.limits.work = budget.usage.work + 1;
                pool.alloc(4097, &mut budget).map(|_| ())
            }
            StoreError::Identity => {
                let mut foreign = lease;
                foreign.owner += 1;
                assert!(matches!(pool.read(foreign), Err(StoreError::Identity)));
                pool.free(foreign, &mut budget)
            }
            StoreError::Arithmetic => pool.alloc(usize::MAX, &mut budget).map(|_| ()),
            _ => unreachable!(),
        };
        assert_eq!(expected, Err(failure));
        assert_eq!(pool.large_heads.len(), heads);
        assert_eq!(pool.bytes.len(), bytes);
        assert_eq!(pool.alloc(0, &mut budget).err(), Some(failure));
        assert_eq!(pool.free(lease, &mut budget), Err(failure));
        assert_eq!(budget.work(0), Err(failure));
    }
    let mut budget = InspectionBudget::empty_publication(ResultLimits {
        retained_bytes: 8192,
        ..ResultLimits::default()
    })
    .unwrap();
    let mut pool = BytePool::new(budget.owner);
    assert_eq!(pool.alloc(4097, &mut budget).err(), Some(StoreError::Capacity));
    assert_eq!(pool.bytes.len(), 0);
    assert_eq!(
        budget.usage.retained_bytes,
        pool.large_heads.capacity() * size_of::<LargeClass>()
    );
    assert!(budget.usage.retained_bytes > 0);
    // The same budget and payload fit exactly only when class-head metadata fits too.
    let mut budget = InspectionBudget::empty_publication(ResultLimits {
        retained_bytes: 8192 + 4 * size_of::<LargeClass>(),
        ..ResultLimits::default()
    })
    .unwrap();
    let mut pool = BytePool::new(budget.owner);
    let lease = pool.alloc(4097, &mut budget).unwrap();
    assert_eq!(lease.size, 8192);
    assert_eq!(pool.large_heads.capacity(), 4);
    assert_eq!(budget.usage.retained_bytes, 8192 + 4 * size_of::<LargeClass>());
}

#[test]
fn byte_pool_foreign_budget_and_malformed_large_lease_fail_without_reuse() {
    let mut budget = InspectionBudget::empty_publication(ResultLimits::default()).unwrap();
    let mut pool = BytePool::new(budget.owner);
    let lease = pool.alloc(8193, &mut budget).unwrap();
    let mut foreign = InspectionBudget::empty_publication(ResultLimits::default()).unwrap();
    assert_eq!(pool.alloc(8193, &mut foreign).err(), Some(StoreError::Identity));
    assert_eq!(foreign.failure(), Some(StoreError::Identity));
    assert_eq!(budget.failure(), None);
    assert_eq!(pool.bytes.len(), lease.size);
    let mut invalid = lease;
    invalid.size = 8192;
    assert_eq!(pool.read(invalid).err(), Some(StoreError::Identity));
    invalid = lease;
    invalid.offset = pool.bytes.len();
    assert_eq!(pool.free(invalid, &mut budget), Err(StoreError::Identity));
    assert_eq!(pool.large_heads[0].head, None);
    assert_eq!(pool.alloc(0, &mut budget).err(), Some(StoreError::Identity));
}

#[test]
fn byte_pool_large_class_exact_retained_boundary_includes_heads() {
    let exact = 8192 + 4 * size_of::<LargeClass>();
    for cap in [exact - 1, exact] {
        let mut budget = InspectionBudget::empty_publication(ResultLimits {
            retained_bytes: cap,
            ..ResultLimits::default()
        })
        .unwrap();
        let mut pool = BytePool::new(budget.owner);
        let result = pool.alloc(4097, &mut budget);
        if cap < exact {
            assert_eq!(result.err(), Some(StoreError::Capacity));
            assert_eq!(pool.bytes.len(), 0);
            assert_eq!(budget.failure(), Some(StoreError::Capacity));
        } else {
            assert_eq!(result.unwrap().size, 8192);
            assert_eq!(budget.usage.retained_bytes, exact);
        }
    }
}

#[test]
fn sparse_qualified_names_seal_only_surviving_hit_views_and_validate_generation() {
    let g = compile(
        concat!(
            "SecRule ARGS|ARGS_NAMES \"@contains blocked\" \"id:1,phase:2,pass\"\n",
            "SecRule ARGS \"@contains blocked\" \"id:2,phase:2,pass\"\n",
        ),
        CompilerLimits::default(),
    )
    .unwrap();
    let mut s = store(&g);
    let stale = s
        .begin_candidate(b"json.replaced", 7, JsonScalarKind::String, None)
        .unwrap();
    s.record_match(stale, 0, 0, b"blocked").unwrap();
    for profile in 0..g.json_inspection.profiles.len() {
        s.complete_profile(stale, profile).unwrap();
    }
    let old = finish(
        &mut s,
        stale,
        &(0..g.json_inspection.profiles.len()).collect::<Vec<_>>(),
    );
    s.discard(old).unwrap();
    let id = s
        .begin_candidate(b"json.replaced", 7, JsonScalarKind::String, None)
        .unwrap();
    assert_eq!(id.0.slot, stale.0.slot);
    assert_ne!(id.0.generation, stale.0.generation);
    for group in 0..g.json_inspection.groups.len() {
        s.record_match(id, group, 0, b"blocked").unwrap();
    }
    for profile in 0..g.json_inspection.profiles.len() {
        s.complete_profile(id, profile).unwrap();
    }
    let receipt = finish(&mut s, id, &(0..g.json_inspection.profiles.len()).collect::<Vec<_>>());
    s.seal(Some(receipt), JsonSemanticSummary::default()).unwrap();
    let (sealed, mut budget) = s.into_sealed().unwrap();
    assert_eq!(sealed.records.qualified_names.len(), 1);
    for collection in [CollectionKind::Args, CollectionKind::ArgsNames] {
        let handle = sealed.qualified_name(&mut budget, id, collection).unwrap();
        let again = sealed.qualified_name(&mut budget, id, collection).unwrap();
        assert_eq!(handle, again);
        let expected = format!("{}:json.replaced", collection.as_str());
        assert_eq!(sealed.match_value(&mut budget, handle).unwrap(), expected.as_bytes());
    }
    assert_eq!(
        sealed.qualified_name(&mut budget, stale, CollectionKind::Args),
        Err(StoreError::Identity)
    );
    assert_eq!(budget.failure(), Some(StoreError::Identity));

    let mut no_hit = store(&g);
    let id = no_hit
        .begin_candidate(b"json.miss", 4, JsonScalarKind::String, None)
        .unwrap();
    for profile in 0..g.json_inspection.profiles.len() {
        no_hit.complete_profile(id, profile).unwrap();
    }
    let receipt = finish(
        &mut no_hit,
        id,
        &(0..g.json_inspection.profiles.len()).collect::<Vec<_>>(),
    );
    no_hit.seal(Some(receipt), JsonSemanticSummary::default()).unwrap();
    let (sealed, _) = no_hit.into_sealed().unwrap();
    assert_eq!(sealed.records.qualified_names.capacity(), 0);
}

#[test]
fn qualified_name_direct_lookup_checks_bounds_and_exact_work() {
    let g = generation();
    let make = || {
        let mut s = store(&g);
        let id = s.begin_candidate(b"json.x", 3, JsonScalarKind::String, None).unwrap();
        s.record_match(id, 0, 0, b"hit").unwrap();
        s.complete_profile(id, 0).unwrap();
        let receipt = finish(&mut s, id, &[0]);
        s.seal(Some(receipt), JsonSemanticSummary::default()).unwrap();
        let (sealed, budget) = s.into_sealed().unwrap();
        (sealed, budget, id)
    };
    assert_eq!(size_of::<QualifiedNamePair>(), 2 * size_of::<QualifiedSpan>());
    #[cfg(target_pointer_width = "64")]
    assert_eq!(
        (
            size_of::<Candidate>(),
            size_of::<Slot<Candidate>>(),
            size_of::<QualifiedNamePair>()
        ),
        (160, 176, 32)
    );
    for allowance in [0, 1] {
        let (sealed, mut budget, id) = make();
        budget.limits.work = budget.usage.work + allowance;
        let result = sealed.qualified_name(&mut budget, id, CollectionKind::Args);
        assert_eq!(result.is_ok(), allowance == 1);
        if allowance == 0 {
            assert_eq!(budget.failure(), Some(StoreError::Work));
        }
    }
    for bad in 0..5 {
        let (mut sealed, mut budget, id) = make();
        let candidate = sealed.records.candidates.get_mut(id.0).unwrap();
        let collection = match bad {
            0 => {
                candidate.qualified_name_pair = NonZeroU32::new(u32::MAX);
                CollectionKind::Args
            }
            1 => {
                sealed.records.qualified_names[0].values[0].as_mut().unwrap().len = usize::MAX;
                CollectionKind::Args
            }
            2 => {
                sealed.records.qualified_names[0].values[0].as_mut().unwrap().offset =
                    NonZeroUsize::new(usize::MAX).unwrap();
                CollectionKind::Args
            }
            3 => CollectionKind::ArgsNames,
            _ => CollectionKind::RequestHeaders,
        };
        assert!(matches!(
            sealed.qualified_name(&mut budget, id, collection),
            Err(StoreError::Identity | StoreError::Arithmetic)
        ));
        assert!(budget.failure().is_some());
    }
}

#[test]
fn sealed_capture_and_name_handles_reject_foreign_or_out_of_bounds_observation() {
    let g = generation();
    let make = || {
        let mut s = store(&g);
        let id = s.begin_candidate(b"json.x", 3, JsonScalarKind::String, None).unwrap();
        let outcome = s.record_match(id, 0, 0, b"hit").unwrap();
        s.add_capture(outcome, b"1", b"hit").unwrap();
        s.complete_profile(id, 0).unwrap();
        let receipt = finish(&mut s, id, &[0]);
        s.seal(Some(receipt), JsonSemanticSummary::default()).unwrap();
        let (sealed, mut budget) = s.into_sealed().unwrap();
        let name = sealed.qualified_name(&mut budget, id, CollectionKind::Args).unwrap();
        let OutcomeValue::Match { first_capture, .. } = sealed.outcome(&mut budget, id, outcome).unwrap().result else {
            panic!("expected match");
        };
        let capture = sealed
            .capture(&mut budget, id, outcome, first_capture.unwrap())
            .unwrap();
        (sealed, budget, name, capture.value, capture.key)
    };
    let (first, _, foreign_name, foreign_capture, foreign_key) = make();
    let (second, mut budget, name, capture, key) = make();
    assert_eq!(second.observe_match_value(key).unwrap(), b"1");
    for foreign in [foreign_name, foreign_capture, foreign_key] {
        assert_eq!(second.observe_match_value(foreign), Err(StoreError::Identity));
    }
    for local in [name, capture, key] {
        let invalid = SealedMatchValue {
            offset: second.records.bytes.bytes.len(),
            ..local
        };
        assert_eq!(second.observe_match_value(invalid), Err(StoreError::Identity));
        assert_eq!(
            second.observe_match_value(SealedMatchValue {
                offset: usize::MAX,
                ..local
            }),
            Err(StoreError::Arithmetic)
        );
    }
    assert_eq!(budget.failure(), None);
    budget.limits.work = budget.usage.work;
    assert_eq!(second.match_value(&mut budget, capture), Err(StoreError::Work));
    assert_eq!(second.observe_match_value(capture).unwrap(), b"hit");
    assert_eq!(first.observe_match_value(foreign_name).unwrap(), b"ARGS:json.x");
}

#[test]
fn sealed_capture_key_mint_rejects_invalid_identity_and_lease_before_publication() {
    let g = generation();
    for fault in 0..4 {
        let mut s = store(&g);
        let id = s.begin_candidate(b"json.x", 1, JsonScalarKind::String, None).unwrap();
        let outcome = s.record_match(id, 0, 0, b"a").unwrap();
        s.add_capture_range(outcome, b"Name", 0, 1).unwrap();
        s.complete_profile(id, 0).unwrap();
        let receipt = finish(&mut s, id, &[0]);
        s.seal(Some(receipt), JsonSemanticSummary::default()).unwrap();
        let (mut sealed, mut budget) = s.into_sealed().unwrap();
        let OutcomeValue::Match {
            first_capture: Some(capture),
            ..
        } = sealed.outcome(&mut budget, id, outcome).unwrap().result
        else {
            panic!("capture");
        };
        let mut attempted = capture;
        match fault {
            0 => attempted.0.owner += 1,
            1 => attempted.0.generation += 1,
            2 => sealed.records.captures.get_mut(capture.0).unwrap().key.owner += 1,
            3 => sealed.records.captures.get_mut(capture.0).unwrap().key.offset = usize::MAX,
            _ => unreachable!(),
        }
        assert!(matches!(
            sealed.capture(&mut budget, id, outcome, attempted),
            Err(StoreError::Identity | StoreError::Arithmetic)
        ));
        assert!(budget.failure().is_some());
    }
}

#[test]
fn arena_page_hint_avoids_tiny_growth_after_a_large_lease() {
    let mut budget = InspectionBudget::empty_publication(ResultLimits::default()).unwrap();
    let mut pool = BytePool::new(budget.owner);
    let prefix = pool.copy(b"key", &mut budget).unwrap();
    let value = pool.alloc(262_147, &mut budget).unwrap();
    let len = pool.bytes.len();
    let capacity = pool.bytes.capacity();
    let pointer = pool.bytes.as_ptr();
    assert_eq!(capacity % 4096, 0);
    assert!(capacity > len);
    let metadata = pool.copy(b"ARGS:json.key", &mut budget).unwrap();
    assert_eq!(pool.bytes.capacity(), capacity);
    assert_eq!(pool.bytes.as_ptr(), pointer);
    assert_eq!(pool.read(prefix).unwrap(), b"key");
    assert_eq!(pool.read(metadata).unwrap(), b"ARGS:json.key");
    assert_eq!(value.len, 262_147);
    assert_eq!(value.size, 266_240);
    assert_eq!(
        budget.usage.retained_bytes,
        capacity + pool.large_heads.capacity() * size_of::<LargeClass>()
    );
}

#[test]
fn arena_page_hint_falls_back_without_work_or_failure_at_tight_caps() {
    let required = 8 + 8192;
    let rounded = 12288;
    for allowance in [required - 1, required, rounded - 1, rounded] {
        let mut budget = InspectionBudget::empty_publication(ResultLimits::default()).unwrap();
        let mut pool = BytePool::new(budget.owner);
        pool.alloc(1, &mut budget).unwrap();
        // Register the class first, matching production admission order.
        pool.class_index(8192, true, &mut budget).unwrap();
        let start_work = budget.usage.work;
        let before = budget.usage.retained_bytes;
        budget.limits.retained_bytes = before + allowance;
        let result = pool.alloc(4097, &mut budget);
        assert_eq!(result.is_ok(), allowance >= required);
        if allowance < required {
            assert_eq!(budget.failure(), Some(StoreError::Capacity));
        } else {
            assert_eq!(budget.failure(), None);
            assert_eq!(pool.bytes.len(), required);
            assert_eq!(
                pool.bytes.capacity(),
                if allowance == rounded { rounded } else { required }
            );
            assert_eq!(budget.usage.retained_bytes, before - 8 + pool.bytes.capacity());
            // Class lookup, old live bytes copied and the new lease initialized;
            // optional unused capacity is neither copied nor zero-initialized.
            assert_eq!(budget.usage.work - start_work, 1 + 8 + 8192);
        }
    }
    assert_eq!(
        BytePool::arena_reserve_amount(usize::MAX - 4095, 0, 8, usize::MAX).unwrap(),
        8
    );
    assert_eq!(
        BytePool::arena_reserve_amount(usize::MAX, 0, 1, usize::MAX),
        Err(StoreError::Arithmetic)
    );
}

#[test]
fn capture_ranges_share_parent_payload_but_charge_each_alias() {
    let g = sharing_generation();
    let mut s = store(&g);
    let id = s.begin_candidate(b"json.x", 7, JsonScalarKind::String, None).unwrap();
    let first = s.record_match(id, 0, 0, b"blocked").unwrap();
    let token = s.stage_match_token(first).unwrap();
    let alias = s.record_shared_match(id, 1, 0, token).unwrap();
    for outcome in [first, alias] {
        s.add_capture_range(outcome, b"1", 1, 3).unwrap();
        s.add_capture_range(outcome, b"name", 1, 3).unwrap();
        s.add_capture_range(outcome, b"empty", 7, 0).unwrap();
    }
    assert_eq!(s.usage().capture_bytes, 2 * (4 + 7 + 5));
    s.complete_profile(id, 0).unwrap();
    let receipt = finish(&mut s, id, &[0]);
    s.seal(Some(receipt), JsonSemanticSummary::default()).unwrap();
    let (sealed, mut budget) = s.into_sealed().unwrap();
    let mut previous = None;
    for outcome in [first, alias] {
        let OutcomeValue::Match {
            value, first_capture, ..
        } = sealed.outcome(&mut budget, id, outcome).unwrap().result
        else {
            panic!("expected match")
        };
        let parent = sealed.match_value(&mut budget, value).unwrap();
        let mut next = first_capture;
        for (offset, expected) in [(1, b"loc".as_slice()), (1, b"loc"), (7, b"")] {
            let capture = sealed.capture(&mut budget, id, outcome, next.unwrap()).unwrap();
            let bytes = sealed.match_value(&mut budget, capture.value).unwrap();
            assert_eq!(bytes, expected);
            assert_eq!(bytes.as_ptr(), parent[offset..].as_ptr());
            if offset == 1 {
                if let Some(value) = previous {
                    assert_eq!(capture.value, value);
                }
                previous = Some(capture.value);
            }
            next = capture.next;
        }
        assert!(next.is_none());
    }
}

#[test]
fn capture_range_admission_rejects_foreign_stale_and_invalid_ranges() {
    let g = sharing_generation();
    for case in 0..5 {
        let mut s = store(&g);
        let id = s.begin_candidate(b"json.x", 3, JsonScalarKind::String, None).unwrap();
        let outcome = s.record_match(id, 0, 0, b"abc").unwrap();
        let expected = if case == 2 {
            StoreError::Arithmetic
        } else {
            StoreError::Identity
        };
        let result = match case {
            0 => s.add_capture_range(outcome, b"0", 2, 2),
            1 => s.add_capture_range(outcome, b"0", 4, 0),
            2 => s.add_capture_range(outcome, b"0", usize::MAX, 1),
            3 => {
                s.complete_profile(id, 0).unwrap();
                let receipt = finish(&mut s, id, &[0]);
                s.discard(receipt).unwrap();
                let replacement = s.begin_candidate(b"json.x", 3, JsonScalarKind::String, None).unwrap();
                s.record_match(replacement, 0, 0, b"abc").unwrap();
                s.add_capture_range(outcome, b"0", 0, 1)
            }
            _ => {
                let mut other = store(&g);
                assert_eq!(other.add_capture_range(outcome, b"0", 0, 1), Err(StoreError::Identity));
                assert_eq!(other.add_capture_range(outcome, b"0", 0, 0), Err(StoreError::Identity));
                continue;
            }
        };
        assert_eq!(result, Err(expected));
        assert_eq!(s.add_capture_range(outcome, b"0", 0, 0), Err(expected));
        assert_eq!(s.usage().retained_bytes, 0);
    }
}

#[test]
fn sealed_capture_ranges_validate_parent_bounds_at_read_time() {
    let g = sharing_generation();
    let mut s = store(&g);
    let id = s.begin_candidate(b"json.x", 3, JsonScalarKind::String, None).unwrap();
    let outcome = s.record_match(id, 0, 0, b"abc").unwrap();
    s.add_capture_range(outcome, b"0", 1, 1).unwrap();
    let StoredOutcome::Match {
        captures: Some(capture),
        ..
    } = s.records.outcomes.get(outcome.0).unwrap().result
    else {
        panic!("missing capture")
    };
    s.complete_profile(id, 0).unwrap();
    let receipt = finish(&mut s, id, &[0]);
    s.seal(Some(receipt), JsonSemanticSummary::default()).unwrap();
    let (mut sealed, mut budget) = s.into_sealed().unwrap();
    sealed.records.captures.get_mut(capture.0).unwrap().value = StoredCaptureValue::MatchRange { start: 3, len: 1 };
    assert!(matches!(
        sealed.capture(&mut budget, id, outcome, capture),
        Err(StoreError::Identity)
    ));
    assert_eq!(budget.failure(), Some(StoreError::Identity));
}

#[test]
fn candidate_local_links_preserve_full_identity_domain_and_actual_layout() {
    let owner = 77;
    for slot in [0, u32::MAX] {
        for generation in [0, u32::MAX] {
            let raw = RawId {
                owner,
                slot,
                generation,
            };
            let candidate = CandidateId(raw);
            let profile = ProfileId(raw);
            let outcome = OutcomeId(raw);
            assert_eq!(
                LocalCandidateId::from_full(candidate, owner).unwrap().full(owner),
                candidate
            );
            assert_eq!(LocalProfileId::from_full(profile, owner).unwrap().full(owner), profile);
            assert_eq!(LocalOutcomeId::from_full(outcome, owner).unwrap().full(owner), outcome);
            assert_eq!(
                LocalCandidateId::from_full(candidate, owner + 1),
                Err(StoreError::Identity)
            );
            assert_eq!(LocalProfileId::from_full(profile, owner + 1), Err(StoreError::Identity));
            assert_eq!(LocalOutcomeId::from_full(outcome, owner + 1), Err(StoreError::Identity));
        }
    }
    #[cfg(target_pointer_width = "64")]
    {
        assert_eq!(size_of::<RawId>(), 16);
        for full in [
            size_of::<Option<CandidateId>>(),
            size_of::<Option<ProfileId>>(),
            size_of::<Option<OutcomeId>>(),
        ] {
            assert_eq!(full, 24);
        }
        for local in [
            size_of::<LocalCandidateId>(),
            size_of::<LocalProfileId>(),
            size_of::<LocalOutcomeId>(),
        ] {
            assert_eq!(local, 8);
        }
        for optional in [
            size_of::<Option<LocalCandidateId>>(),
            size_of::<Option<LocalProfileId>>(),
            size_of::<Option<LocalOutcomeId>>(),
        ] {
            assert_eq!(optional, 12);
        }
        assert_eq!((size_of::<Candidate>(), size_of::<Slot<Candidate>>()), (160, 176));
    }
}

#[test]
fn candidate_local_links_retain_stale_generations_after_pool_reuse() {
    let g = generation();
    let mut s = store(&g);
    let old = s.begin_candidate(b"json.old", 3, JsonScalarKind::String, None).unwrap();
    s.record_changed_step(old, 0, 0, 3).unwrap();
    s.record_match(old, 0, 0, b"old").unwrap();
    let old_candidate = LocalCandidateId::from_full(old, s.records.candidates.owner).unwrap();
    let candidate = s.records.candidates.get(old.0).unwrap();
    let old_profile = candidate.profiles.unwrap();
    let old_outcome = candidate.outcomes.unwrap();
    s.complete_profile(old, 0).unwrap();
    let receipt = finish(&mut s, old, &[0]);
    s.discard(receipt).unwrap();

    let new = s.begin_candidate(b"json.new", 3, JsonScalarKind::String, None).unwrap();
    s.record_changed_step(new, 0, 0, 3).unwrap();
    s.record_match(new, 0, 0, b"new").unwrap();
    let candidate = s.records.candidates.get(new.0).unwrap();
    let new_profile = candidate.profiles.unwrap();
    let new_outcome = candidate.outcomes.unwrap();
    assert_eq!(old.0.slot, new.0.slot);
    assert_ne!(old.0.generation, new.0.generation);
    assert_eq!(old_profile.slot, new_profile.slot);
    assert_ne!(old_profile.generation, new_profile.generation);
    assert_eq!(old_outcome.slot, new_outcome.slot);
    assert_ne!(old_outcome.generation, new_outcome.generation);
    assert_eq!(old_candidate.full(s.records.candidates.owner), old);
    assert!(matches!(
        s.records
            .candidates
            .get(old_candidate.full(s.records.candidates.owner).0),
        Err(StoreError::Identity)
    ));
    assert!(matches!(
        s.records.profiles.get(old_profile.full(s.records.profiles.owner).0),
        Err(StoreError::Identity)
    ));
    assert!(matches!(
        s.records.outcomes.get(old_outcome.full(s.records.outcomes.owner).0),
        Err(StoreError::Identity)
    ));
    s.complete_profile(new, 0).unwrap();
    let receipt = finish(&mut s, new, &[0]);
    s.seal(Some(receipt), JsonSemanticSummary::default()).unwrap();
    let (sealed, mut budget) = s.into_sealed().unwrap();
    assert_eq!(sealed.candidate(&mut budget, new).unwrap().name, b"json.new");
    assert!(matches!(sealed.candidate(&mut budget, old), Err(StoreError::Identity)));
    assert_eq!(budget.failure(), Some(StoreError::Identity));
    assert!(matches!(sealed.candidate(&mut budget, new), Err(StoreError::Identity)));
}

#[test]
fn candidate_local_heads_keep_profile_and_outcome_parent_checks_sticky() {
    let g = generation();
    for corrupt_profile in [true, false] {
        let mut s = store(&g);
        let mut receipts = None;
        let mut ids = Vec::new();
        for name in [b"json.a".as_slice(), b"json.b".as_slice()] {
            let id = s.begin_candidate(name, 3, JsonScalarKind::String, None).unwrap();
            s.record_changed_step(id, 0, 0, 3).unwrap();
            s.record_match(id, 0, 0, b"hit").unwrap();
            s.complete_profile(id, 0).unwrap();
            let receipt = finish(&mut s, id, &[0]);
            receipts = s.concat(receipts, Some(receipt)).unwrap();
            ids.push(id);
        }
        s.seal(receipts, JsonSemanticSummary::default()).unwrap();
        let (mut sealed, mut budget) = s.into_sealed().unwrap();
        if corrupt_profile {
            let foreign = sealed.records.candidates.get(ids[1].0).unwrap().profiles;
            sealed.records.candidates.get_mut(ids[0].0).unwrap().profiles = foreign;
            assert!(matches!(
                sealed.profile(&mut budget, ids[0], 0),
                Err(StoreError::Identity)
            ));
        } else {
            let foreign = sealed.records.candidates.get(ids[1].0).unwrap().outcomes;
            sealed.records.candidates.get_mut(ids[0].0).unwrap().outcomes = foreign;
            assert!(matches!(
                sealed
                    .lookup_selected(
                        &mut budget,
                        ids[0],
                        0,
                        &g,
                        RuntimeLimits::default(),
                        EvaluationMode::Accelerated
                    )
                    .map(|(outcome, _)| outcome),
                Err(StoreError::Identity)
            ));
        }
        assert_eq!(budget.failure(), Some(StoreError::Identity));
        assert!(matches!(
            sealed.candidate(&mut budget, ids[1]),
            Err(StoreError::Identity)
        ));
    }
}

#[test]
fn candidate_local_chain_concat_discard_and_multi_object_reuse_preserve_order() {
    let g = generation();
    let mut s = store(&g);
    let mut discarded = None;
    let mut old = Vec::new();
    for name in [b"json.old.a".as_slice(), b"json.old.b", b"json.old.c"] {
        let id = s.begin_candidate(name, 3, JsonScalarKind::String, None).unwrap();
        s.record_changed_step(id, 0, 0, 3).unwrap();
        s.record_match(id, 0, 0, b"old").unwrap();
        s.complete_profile(id, 0).unwrap();
        let receipt = finish(&mut s, id, &[0]);
        discarded = s.concat(discarded, Some(receipt)).unwrap();
        old.push(id);
    }
    s.discard(discarded.unwrap()).unwrap();
    assert_eq!(
        (
            s.usage().live_candidates,
            s.usage().live_profiles,
            s.usage().live_outcomes
        ),
        (0, 0, 0)
    );
    let retained = s.usage().retained_bytes;
    let names = [b"json.new.a".as_slice(), b"json.new.b", b"json.new.c"];
    let mut survivors = None;
    let mut new = Vec::new();
    for name in names {
        let id = s.begin_candidate(name, 3, JsonScalarKind::String, None).unwrap();
        s.record_changed_step(id, 0, 0, 3).unwrap();
        s.record_match(id, 0, 0, b"new").unwrap();
        s.complete_profile(id, 0).unwrap();
        let receipt = finish(&mut s, id, &[0]);
        survivors = s.concat(survivors, Some(receipt)).unwrap();
        new.push(id);
    }
    assert_eq!(s.usage().retained_bytes, retained);
    for id in &new {
        assert!(old
            .iter()
            .any(|old| old.0.slot == id.0.slot && old.0.generation != id.0.generation));
    }
    s.seal(survivors, JsonSemanticSummary::default()).unwrap();
    let (sealed, mut budget) = s.into_sealed().unwrap();
    let mut next = sealed.receipt.map(|receipt| receipt.head);
    for (index, name) in names.into_iter().enumerate() {
        assert_eq!(next, Some(new[index]));
        let candidate = sealed.candidate(&mut budget, next.unwrap()).unwrap();
        assert_eq!(candidate.name, name);
        next = candidate.next;
        assert!(sealed.profile(&mut budget, new[index], 0).unwrap().record.is_some());
        assert!(matches!(
            sealed
                .lookup_selected(
                    &mut budget,
                    new[index],
                    0,
                    &g,
                    RuntimeLimits::default(),
                    EvaluationMode::Accelerated
                )
                .map(|(outcome, _)| outcome)
                .unwrap(),
            SelectedOutcome::MatchOrError(_)
        ));
    }
    assert_eq!(next, None);
}

#[test]
fn candidate_local_slot_growth_uses_actual_size_at_physical_and_work_boundaries() {
    let unit = size_of::<Slot<Candidate>>();
    for limit in [
        StoreError::Capacity,
        StoreError::Work,
        StoreError::Records,
        StoreError::Candidates,
    ] {
        for allowance in [0, 1, 2] {
            let mut budget = InspectionBudget::empty_publication(ResultLimits {
                retained_bytes: 4 * unit,
                ..ResultLimits::default()
            })
            .unwrap();
            let mut slots: Vec<Slot<Candidate>> = Vec::new();
            let owner = budget.owner;
            budget
                .checked(owner, |budget| budget.grow(&mut slots, 4, true, true))
                .unwrap();
            assert_eq!(slots.capacity(), 4);
            assert_eq!(budget.usage.retained_bytes, slots.capacity() * unit);
            slots.resize_with(4, || Slot {
                generation: 0,
                free_next: None,
                value: None,
            });
            budget.limits.retained_bytes = 9 * unit;
            match limit {
                StoreError::Capacity => budget.limits.retained_bytes = 9 * unit - 1 + allowance,
                StoreError::Work => budget.limits.work = 4 * unit - 1 + allowance,
                StoreError::Records => budget.limits.records = 4 + allowance,
                StoreError::Candidates => budget.limits.candidates = 4 + allowance,
                _ => unreachable!(),
            }
            let result = budget.checked(owner, |budget| budget.grow(&mut slots, 1, true, true));
            if allowance == 0 {
                assert_eq!(result, Err(limit));
                assert_eq!(slots.capacity(), 4);
                assert_eq!(budget.failure(), Some(limit));
                assert_eq!(budget.checked(owner, |_| Ok(())), Err(limit));
            } else {
                result.unwrap();
                assert_eq!(slots.capacity(), 5);
                assert_eq!(budget.usage.retained_bytes, 5 * unit);
                assert_eq!(budget.usage.peak_retained_bytes, 9 * unit);
                assert_eq!(budget.usage.work, 4 * unit);
                assert_eq!(budget.usage.allocated_records, slots.capacity());
            }
        }
    }
}
