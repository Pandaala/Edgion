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

/// Fixed selection clock for idle-probe tests.
const NOW: u64 = 1_000;

fn input(candidates: &[RouteCandidate], entropy: u128) -> PickerInput<'_> {
    PickerInput {
        candidates,
        entropy,
        now_second: NOW,
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

/// Entropy whose P2C draw equals `draw`.
fn p2c(draw: u128) -> u128 {
    draw << P2C_ENTROPY_SHIFT
}

fn mature(p50: u64) -> AdaptiveSnapshot {
    AdaptiveSnapshot {
        latency_p50_micros: Some(p50),
        latency_samples: LATENCY_MATURE_SAMPLES,
        ..AdaptiveSnapshot::default()
    }
}

fn winners(candidates: &[RouteCandidate], selection: AiBackendSelection, draws: u128) -> Vec<u8> {
    (0..draws)
        .map(|draw| pick(input(candidates, p2c(draw)), selection).unwrap().slot)
        .collect()
}

#[test]
fn adaptive_p2c_samples_every_distinct_pair_and_never_the_worst() {
    let mut candidates = [candidate(0, 0, 1), candidate(1, 1, 1), candidate(2, 2, 1)];
    for (candidate, p50) in candidates.iter_mut().zip([10, 20, 30]) {
        candidate.adaptive = mature(p50);
    }
    // Six draws cover all six ordered distinct pairs of three candidates.
    let picked = winners(&candidates, AiBackendSelection::LowestLatency, 6);
    let count = |slot| picked.iter().filter(|picked| **picked == slot).count();
    assert_eq!((count(0), count(1), count(2)), (4, 2, 0));

    // Ineligible candidates are never sampled.
    candidates[0].eligibility.runtime_available = false;
    assert!(winners(&candidates, AiBackendSelection::LowestLatency, 6)
        .iter()
        .all(|slot| *slot == 1));

    // A single eligible candidate is returned without sampling.
    candidates[1].eligibility.untried = false;
    assert!(winners(&candidates, AiBackendSelection::LowestLatency, 4)
        .iter()
        .all(|slot| *slot == 2));
}

#[test]
fn lowest_latency_selects_the_fastest_mature_candidate() {
    // The slower candidate has the larger weight, but not enough capacity to
    // offset a five-times-higher p50, so every pairing picks the faster one.
    let mut candidates = [candidate(0, 0, 2), candidate(1, 1, 1)];
    candidates[0].adaptive = mature(50);
    candidates[1].adaptive = mature(10);
    assert!(winners(&candidates, AiBackendSelection::LowestLatency, 128)
        .iter()
        .all(|slot| *slot == 1));
}

#[test]
fn lowest_latency_in_flight_load_pushes_traffic_away() {
    let mut candidates = [candidate(0, 0, 1), candidate(1, 1, 1)];
    candidates[0].adaptive = mature(10);
    candidates[1].adaptive = mature(10);
    // Equal scores keep the random first draw.
    assert_eq!(winners(&candidates, AiBackendSelection::LowestLatency, 2), [0, 1]);

    candidates[1].adaptive.in_flight = 3;
    assert!(winners(&candidates, AiBackendSelection::LowestLatency, 8)
        .iter()
        .all(|slot| *slot == 0));

    // A fast backend accumulating hanging requests loses to a slower idle one
    // before any completion is published.
    candidates[0].adaptive = AdaptiveSnapshot {
        in_flight: 10,
        ..mature(10)
    };
    candidates[1].adaptive = mature(30);
    assert!(winners(&candidates, AiBackendSelection::LowestLatency, 8)
        .iter()
        .all(|slot| *slot == 1));
}

#[test]
fn lowest_latency_weight_scales_capacity() {
    // Equal latency: in-flight load balances in proportion to weight, so a
    // weight-3 candidate takes up to three times the idle peer's (in_flight + 1).
    let mut candidates = [candidate(0, 0, 1), candidate(1, 1, 3)];
    candidates[0].adaptive = mature(10);
    candidates[1].adaptive = AdaptiveSnapshot {
        in_flight: 1,
        ..mature(10)
    };
    assert!(winners(&candidates, AiBackendSelection::LowestLatency, 8)
        .iter()
        .all(|slot| *slot == 1));
    candidates[1].adaptive.in_flight = 2;
    assert_eq!(winners(&candidates, AiBackendSelection::LowestLatency, 2), [0, 1]);
    candidates[1].adaptive.in_flight = 3;
    assert!(winners(&candidates, AiBackendSelection::LowestLatency, 8)
        .iter()
        .all(|slot| *slot == 0));
}

#[test]
fn lowest_latency_failures_penalize_fast_failing_backends() {
    let mut candidates = [candidate(0, 0, 1), candidate(1, 1, 1)];
    candidates[0].adaptive = AdaptiveSnapshot {
        latency_failures: LATENCY_MATURE_SAMPLES,
        ..mature(10)
    };
    candidates[1].adaptive = mature(10);
    assert!(winners(&candidates, AiBackendSelection::LowestLatency, 8)
        .iter()
        .all(|slot| *slot == 1));

    // A backend that only fails never matures and, recently admitted, is not
    // probed, so its pessimistic score cannot beat a healthy one.
    candidates[0].adaptive = AdaptiveSnapshot {
        latency_failures: 5,
        last_attempt_second: NOW,
        ..AdaptiveSnapshot::default()
    };
    assert!(winners(&candidates, AiBackendSelection::LowestLatency, 8)
        .iter()
        .all(|slot| *slot == 1));
}

#[test]
fn lowest_latency_probes_unknown_candidates_once_per_interval() {
    // The unknown candidate has a third of the mature one's weight, so on
    // a borrowed p50 alone it would never win at low concurrency.
    let mut candidates = [candidate(0, 0, 3), candidate(1, 1, 1)];
    candidates[0].adaptive = mature(100);
    // Never admitted, or not admitted for a full interval: probed. A sample
    // count without p50 is not latency evidence either.
    for adaptive in [
        AdaptiveSnapshot::default(),
        AdaptiveSnapshot {
            latency_samples: LATENCY_MATURE_SAMPLES,
            last_attempt_second: NOW - LATENCY_PROBE_INTERVAL_SECONDS,
            ..AdaptiveSnapshot::default()
        },
    ] {
        candidates[1].adaptive = adaptive;
        assert!(winners(&candidates, AiBackendSelection::LowestLatency, 8)
            .iter()
            .all(|slot| *slot == 1));
    }

    // A probe in flight, or one admitted within the interval however it
    // ended (no sample, no failure verdict), is scored pessimistically.
    for adaptive in [
        AdaptiveSnapshot {
            in_flight: 1,
            ..AdaptiveSnapshot::default()
        },
        AdaptiveSnapshot {
            last_attempt_second: NOW - LATENCY_PROBE_INTERVAL_SECONDS + 1,
            ..AdaptiveSnapshot::default()
        },
    ] {
        candidates[1].adaptive = adaptive;
        assert!(winners(&candidates, AiBackendSelection::LowestLatency, 8)
            .iter()
            .all(|slot| *slot == 0));
    }
    // Against an equal-weight best candidate the unknown one yields until
    // that candidate carries more load than the penalty (2x worst p50).
    candidates[0].weight = 1;
    assert!(winners(&candidates, AiBackendSelection::LowestLatency, 8)
        .iter()
        .all(|slot| *slot == 0));
    candidates[0].adaptive.in_flight = 2;
    assert!(winners(&candidates, AiBackendSelection::LowestLatency, 8)
        .iter()
        .all(|slot| *slot == 1));

    // With no mature candidate the probe is off: weighted least-request.
    let mut cold = [candidate(0, 0, 1), candidate(1, 1, 3)];
    cold[0].adaptive.in_flight = 1;
    cold[1].adaptive.in_flight = 2;
    assert!(winners(&cold, AiBackendSelection::LowestLatency, 8)
        .iter()
        .all(|slot| *slot == 1));
}

#[test]
fn lowest_latency_immature_candidates_use_their_own_p50() {
    let mut candidates = [candidate(0, 0, 1), candidate(1, 1, 1)];
    candidates[0].adaptive = mature(100);
    candidates[1].adaptive = AdaptiveSnapshot {
        latency_p50_micros: Some(500),
        latency_samples: 1,
        last_attempt_second: NOW,
        ..AdaptiveSnapshot::default()
    };
    assert!(winners(&candidates, AiBackendSelection::LowestLatency, 8)
        .iter()
        .all(|slot| *slot == 0));
    candidates[1].adaptive.latency_p50_micros = Some(50);
    assert!(winners(&candidates, AiBackendSelection::LowestLatency, 8)
        .iter()
        .all(|slot| *slot == 1));
}

#[test]
fn lowest_latency_extreme_inputs_saturate_without_panicking() {
    let mut candidates = [candidate(0, 0, 1), candidate(1, 1, i32::MAX)];
    for candidate in &mut candidates {
        candidate.adaptive = AdaptiveSnapshot {
            latency_p50_micros: Some(u64::MAX),
            latency_samples: u32::MAX,
            latency_failures: u32::MAX,
            in_flight: u32::MAX,
            ..AdaptiveSnapshot::default()
        };
    }
    assert!(pick(input(&candidates, p2c(0)), AiBackendSelection::LowestLatency).is_ok());
}

#[test]
fn lowest_usage_uses_weighted_capacity_score_and_in_flight_tokens() {
    let mut candidates = [candidate(0, 5, 1), candidate(1, 1, 2)];
    candidates[0].adaptive.charged_tokens_60s = 100; // score 100
    candidates[1].adaptive.charged_tokens_60s = 100; // score 50: wins
    assert!(winners(&candidates, AiBackendSelection::LowestUsage, 8)
        .iter()
        .all(|slot| *slot == 1));

    // Provisional tokens of in-flight attempts count before settlement.
    candidates[1].adaptive.in_flight_tokens = 200; // score 150
    assert!(winners(&candidates, AiBackendSelection::LowestUsage, 8)
        .iter()
        .all(|slot| *slot == 0));

    // Ties keep the random first draw instead of declaration order.
    candidates[1].adaptive.in_flight_tokens = 100; // score 100
    assert_eq!(winners(&candidates, AiBackendSelection::LowestUsage, 2), [0, 1]);
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
