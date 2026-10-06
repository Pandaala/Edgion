use super::*;

fn candidate(slot: u8, order: u8, weight: i32) -> RouteCandidate {
    RouteCandidate {
        slot,
        order,
        weight,
        eligibility: CandidateEligibility {
            valid_ref: true,
            model_compatible: true,
            runtime_available: true,
            untried: true,
        },
        adaptive: AdaptiveSnapshot::default(),
        cost: Some(CostSnapshot {
            input_price_micro_usd_per_million: 1,
            output_price_micro_usd_per_million: 1,
            service_max_output_tokens: Some(1),
        }),
    }
}

fn input(candidates: &[RouteCandidate], entropy: u128) -> PickerInput<'_> {
    PickerInput {
        candidates,
        entropy,
        selection_sequence: 0,
        input_tokens: 1,
        request_max_output_tokens: Some(1),
    }
}

#[test]
fn invalid_gate_preserves_aggregate_weight_share() {
    let mut candidates = [candidate(0, 0, 1), candidate(1, 1, 3)];
    candidates[0].eligibility.valid_ref = false;
    let invalid = (0..400)
        .filter(|draw| invalid_weight_gate(&candidates, *draw).unwrap() == ConformanceGate::Invalid)
        .count();
    assert_eq!(invalid, 100);
    candidates[0].weight = 0;
    assert_eq!(invalid_weight_gate(&candidates, 0), Ok(ConformanceGate::Valid));
}

#[test]
fn defensive_weight_and_candidate_bounds_fail_closed() {
    let mut negative = candidate(0, 0, -1);
    assert_eq!(
        pick(input(std::slice::from_ref(&negative), 0), AiBackendSelection::Weighted),
        Err(PickerError::InvalidWeight)
    );
    negative.weight = 0;
    assert_eq!(
        invalid_weight_gate(std::slice::from_ref(&negative), 0),
        Err(PickerError::NoPositiveWeight)
    );
    let too_many = vec![candidate(0, 0, 1); MAX_AI_ROUTE_CANDIDATES + 1];
    assert_eq!(
        pick(input(&too_many, 0), AiBackendSelection::Priority),
        Err(PickerError::TooManyCandidates)
    );
}

#[test]
fn weighted_has_exact_small_integer_distribution() {
    let candidates = [candidate(4, 0, 1), candidate(9, 1, 3)];
    let first = (0..400)
        .filter(|draw| {
            pick(input(&candidates, *draw), AiBackendSelection::Weighted)
                .unwrap()
                .slot
                == 4
        })
        .count();
    assert_eq!(first, 100);
}

#[test]
fn priority_uses_declaration_order_and_skips_ineligible() {
    let mut candidates = [candidate(0, 8, 100), candidate(1, 2, 1), candidate(2, 0, 1)];
    candidates[2].eligibility.runtime_available = false;
    assert_eq!(
        pick(input(&candidates, 0), AiBackendSelection::Priority).unwrap().slot,
        1
    );
}

#[test]
fn lowest_latency_bootstraps_explores_and_weight_breaks_ties() {
    let mut candidates = [candidate(0, 0, 1), candidate(1, 1, 3), candidate(2, 2, 1)];
    candidates[0].adaptive = AdaptiveSnapshot {
        latency_p50_micros: Some(10),
        latency_samples: 8,
        charged_tokens_60s: 0,
    };
    candidates[1].adaptive = AdaptiveSnapshot {
        latency_p50_micros: Some(10),
        latency_samples: 8,
        charged_tokens_60s: 0,
    };
    // Normal mature choice: exact latency tie is weighted.
    assert_eq!(
        pick(input(&candidates, 2), AiBackendSelection::LowestLatency)
            .unwrap()
            .slot,
        1
    );
    // Every twentieth selection explores the only bootstrap candidate.
    let mut exploring = input(&candidates, 0);
    exploring.selection_sequence = 19;
    assert_eq!(pick(exploring, AiBackendSelection::LowestLatency).unwrap().slot, 2);

    // With no mature samples, bootstrap selection uses route weights.
    candidates[0].adaptive.latency_samples = 0;
    candidates[1].adaptive.latency_samples = 0;
    assert_eq!(
        pick(input(&candidates, 2), AiBackendSelection::LowestLatency)
            .unwrap()
            .slot,
        1
    );
}

#[test]
fn lowest_latency_selects_the_fastest_mature_candidate() {
    let mut candidates = [candidate(0, 0, 100), candidate(1, 1, 1)];
    candidates[0].adaptive = AdaptiveSnapshot {
        latency_p50_micros: Some(50),
        latency_samples: 8,
        charged_tokens_60s: 0,
    };
    candidates[1].adaptive = AdaptiveSnapshot {
        latency_p50_micros: Some(10),
        latency_samples: 8,
        charged_tokens_60s: 0,
    };

    assert!((0..128).all(|entropy| {
        pick(input(&candidates, entropy), AiBackendSelection::LowestLatency)
            .expect("a mature candidate must be selected")
            .slot
            == 1
    }));
}

#[test]
fn lowest_usage_uses_weighted_capacity_score_and_order_tie() {
    let mut candidates = [candidate(0, 5, 1), candidate(1, 1, 2), candidate(2, 0, 1)];
    candidates[0].adaptive.charged_tokens_60s = 100;
    candidates[1].adaptive.charged_tokens_60s = 100; // score 50: wins
    candidates[2].adaptive.charged_tokens_60s = 50; // score 50: lower order wins
    assert_eq!(
        pick(input(&candidates, 0), AiBackendSelection::LowestUsage)
            .unwrap()
            .slot,
        2
    );
}

#[test]
fn cost_weighted_requires_every_eligible_candidates_cost_inputs() {
    let mut candidates = [candidate(0, 0, 1), candidate(1, 1, 1)];
    candidates[1].cost = None;
    assert_eq!(
        pick(input(&candidates, 0), AiBackendSelection::CostWeighted),
        Err(PickerError::MissingCostInput)
    );
    candidates[1].eligibility.runtime_available = false;
    assert_eq!(
        pick(input(&candidates, 0), AiBackendSelection::CostWeighted)
            .unwrap()
            .slot,
        0
    );

    candidates[0].cost.as_mut().unwrap().service_max_output_tokens = None;
    let mut no_bound = input(&candidates, 0);
    no_bound.request_max_output_tokens = None;
    assert_eq!(
        pick(no_bound, AiBackendSelection::CostWeighted),
        Err(PickerError::MissingCostInput)
    );
}

#[test]
fn cost_weighted_uses_inverse_square_integer_weights() {
    let mut candidates = [candidate(0, 0, 1), candidate(1, 1, 1)];
    candidates[0].cost = Some(CostSnapshot {
        input_price_micro_usd_per_million: 1,
        output_price_micro_usd_per_million: 0,
        service_max_output_tokens: Some(0),
    });
    candidates[1].cost = Some(CostSnapshot {
        input_price_micro_usd_per_million: 2,
        output_price_micro_usd_per_million: 0,
        service_max_output_tokens: Some(0),
    });
    let mut picker = input(&candidates, 1_100_000);
    picker.input_tokens = 1_000_000;
    picker.request_max_output_tokens = Some(0);
    // Weights are 1_000_000 and 250_000; this ticket lands on expensive.
    assert_eq!(pick(picker, AiBackendSelection::CostWeighted).unwrap().slot, 1);
}

#[test]
fn zero_cost_candidates_exclude_nonzero_cost_candidates() {
    let mut candidates = [candidate(0, 0, 1), candidate(1, 1, 100)];
    candidates[0].cost = Some(CostSnapshot {
        input_price_micro_usd_per_million: 0,
        output_price_micro_usd_per_million: 0,
        service_max_output_tokens: Some(1),
    });
    let mut picker = input(&candidates, u128::MAX);
    picker.input_tokens = 1_000_000;
    assert_eq!(pick(picker, AiBackendSelection::CostWeighted).unwrap().slot, 0);
}

#[test]
fn cost_weighted_minimum_applies_after_route_weight_multiplication() {
    let mut candidates = [candidate(0, 0, 1), candidate(1, 1, 100)];
    candidates[0].cost = Some(CostSnapshot {
        input_price_micro_usd_per_million: 1,
        output_price_micro_usd_per_million: 0,
        service_max_output_tokens: Some(0),
    });
    candidates[1].cost = Some(CostSnapshot {
        input_price_micro_usd_per_million: 2_000,
        output_price_micro_usd_per_million: 0,
        service_max_output_tokens: Some(0),
    });
    let mut picker = input(&candidates, 1_000_001);
    picker.input_tokens = 1_000_000;
    picker.request_max_output_tokens = Some(0);
    // Effective weights are 1_000_000 and the post-product minimum 1.
    // The entropy wraps to zero and therefore selects the cheap target.
    assert_eq!(pick(picker, AiBackendSelection::CostWeighted).unwrap().slot, 0);
}

#[test]
fn cost_weighted_overflow_fails_closed() {
    let mut candidate = candidate(0, 0, 1);
    candidate.cost = Some(CostSnapshot {
        input_price_micro_usd_per_million: u64::MAX,
        output_price_micro_usd_per_million: u64::MAX,
        service_max_output_tokens: Some(u64::MAX),
    });
    let mut picker = input(std::slice::from_ref(&candidate), 0);
    picker.input_tokens = u64::MAX;
    picker.request_max_output_tokens = None;
    assert_eq!(
        pick(picker, AiBackendSelection::CostWeighted),
        Err(PickerError::ArithmeticOverflow)
    );
}

#[test]
fn every_picker_fails_when_no_candidate_is_eligible() {
    let mut candidate = candidate(0, 0, 1);
    candidate.eligibility.model_compatible = false;
    for selection in [
        AiBackendSelection::Weighted,
        AiBackendSelection::Priority,
        AiBackendSelection::LowestLatency,
        AiBackendSelection::LowestUsage,
        AiBackendSelection::CostWeighted,
    ] {
        assert_eq!(
            pick(input(std::slice::from_ref(&candidate), 0), selection),
            Err(PickerError::NoEligibleCandidate)
        );
    }
}
