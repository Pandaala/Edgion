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
    ResultStore::new(
        GenerationHandle::Borrowed(g),
        RuntimeLimits::default(),
        EvaluationMode::Accelerated,
        ResultLimits::default(),
    )
    .unwrap()
}
fn finish(s: &mut ResultStore<'_>, id: CandidateId, profiles: &[usize]) -> Receipt {
    let proof = s.completion_proof(id, profiles).unwrap();
    s.complete_candidate(proof).unwrap()
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
            sealed.lookup_selected(
                &mut budget,
                receipt.unwrap().head,
                0,
                &g,
                RuntimeLimits::default(),
                EvaluationMode::Accelerated
            ),
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
        assert_eq!(capture.key, key);
        assert_eq!(capture.value, b"hit");
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
            s.add_capture(alias, b"0", b"second").unwrap();
            assert_eq!(s.usage().matched_bytes, 7);
            assert_eq!(s.usage().live_outcomes, 2);
            if owner_first {
                s.records.candidates.get_mut(id.0).unwrap().outcomes = Some(first);
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
        assert_eq!(
            sealed
                .capture(&mut budget, id, outcome, first_capture.unwrap())
                .unwrap()
                .value,
            expected
        );
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
