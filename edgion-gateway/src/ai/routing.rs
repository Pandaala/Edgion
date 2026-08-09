//! Pure, bounded AI route-candidate selection.
//!
//! Callers supply immutable eligibility/telemetry snapshots and entropy. This
//! module performs no locking, sorting, floating-point arithmetic, I/O, or
//! mutation of adaptive state on the request path.

use edgion_resources::resources::edgion_plugins::AiBackendSelection;

pub const MAX_AI_ROUTE_CANDIDATES: usize = 16;
const LATENCY_MATURE_SAMPLES: u32 = 8;
pub(crate) const LATENCY_EXPLORATION_INTERVAL: u8 = 20;
const SCORE_SCALE: u128 = 1_000_000;
const PRICE_DENOMINATOR: u128 = 1_000_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CandidateEligibility {
    /// Structural/ref validity used by the Gateway API weighted invalid gate.
    pub valid_ref: bool,
    /// The logical model resolves for this destination.
    pub model_compatible: bool,
    /// Credentials, circuit/ejection, and other runtime admission permit use.
    pub runtime_available: bool,
    /// This destination has not already been exhausted in the current round.
    pub untried: bool,
}

impl CandidateEligibility {
    fn selectable(self) -> bool {
        self.valid_ref && self.model_compatible && self.runtime_available && self.untried
    }
}

/// Integer values published by the bounded background telemetry aggregator.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AdaptiveSnapshot {
    pub latency_p50_micros: Option<u64>,
    pub latency_samples: u32,
    pub charged_tokens_60s: u64,
}

/// Candidate-local declared model and price data for CostWeighted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CostSnapshot {
    pub input_price_micro_usd_per_million: u64,
    pub output_price_micro_usd_per_million: u64,
    pub service_max_output_tokens: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RouteCandidate {
    /// Stable compiled backend slot used by later attempt construction.
    pub slot: u8,
    /// Post-delegation declaration order.
    pub order: u8,
    /// Raw Gateway API backendRef weight. Zero disables; negative fails closed.
    pub weight: i32,
    pub eligibility: CandidateEligibility,
    pub adaptive: AdaptiveSnapshot,
    /// `None` means the logical model or either required price is absent.
    pub cost: Option<CostSnapshot>,
}

#[derive(Clone, Copy, Debug)]
pub struct PickerInput<'a> {
    pub candidates: &'a [RouteCandidate],
    /// Caller-supplied request-stable entropy. Selection maps it into the
    /// current bounded aggregate weight without acquiring an RNG lock.
    pub entropy: u128,
    /// Request-independent sequence published with adaptive state. Every 20th
    /// LowestLatency selection explores a bootstrap candidate.
    pub selection_sequence: u64,
    pub input_tokens: u64,
    pub request_max_output_tokens: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConformanceGate {
    Valid,
    Invalid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickerError {
    TooManyCandidates,
    InvalidWeight,
    NoPositiveWeight,
    NoEligibleCandidate,
    MissingCostInput,
    ArithmeticOverflow,
}

pub fn invalid_weight_gate(candidates: &[RouteCandidate], entropy: u128) -> Result<ConformanceGate, PickerError> {
    validate_candidates(candidates)?;
    let mut valid = 0u128;
    let mut invalid = 0u128;
    for candidate in candidates {
        let weight = positive_weight(candidate.weight)?;
        if candidate.eligibility.valid_ref {
            valid = valid.checked_add(weight).ok_or(PickerError::ArithmeticOverflow)?;
        } else {
            invalid = invalid.checked_add(weight).ok_or(PickerError::ArithmeticOverflow)?;
        }
    }
    let total = valid.checked_add(invalid).ok_or(PickerError::ArithmeticOverflow)?;
    if total == 0 {
        return Err(PickerError::NoPositiveWeight);
    }
    Ok(if entropy % total < invalid {
        ConformanceGate::Invalid
    } else {
        ConformanceGate::Valid
    })
}

pub fn pick<'a>(input: PickerInput<'a>, selection: AiBackendSelection) -> Result<&'a RouteCandidate, PickerError> {
    validate_candidates(input.candidates)?;
    match selection {
        AiBackendSelection::Weighted => pick_weighted(input.candidates, input.entropy, |_| true),
        AiBackendSelection::Priority => pick_priority(input.candidates),
        AiBackendSelection::LowestLatency => pick_lowest_latency(input),
        AiBackendSelection::LowestUsage => pick_lowest_usage(input.candidates),
        AiBackendSelection::CostWeighted => pick_cost_weighted(input),
    }
}

fn validate_candidates(candidates: &[RouteCandidate]) -> Result<(), PickerError> {
    if candidates.len() > MAX_AI_ROUTE_CANDIDATES {
        return Err(PickerError::TooManyCandidates);
    }
    if candidates.iter().any(|candidate| candidate.weight < 0) {
        return Err(PickerError::InvalidWeight);
    }
    Ok(())
}

fn positive_weight(weight: i32) -> Result<u128, PickerError> {
    if weight < 0 {
        Err(PickerError::InvalidWeight)
    } else {
        Ok(weight as u128)
    }
}

fn is_selectable(candidate: &RouteCandidate) -> bool {
    candidate.weight > 0 && candidate.eligibility.selectable()
}

fn pick_weighted<'a>(
    candidates: &'a [RouteCandidate],
    entropy: u128,
    extra: impl Fn(&RouteCandidate) -> bool,
) -> Result<&'a RouteCandidate, PickerError> {
    let total = candidates
        .iter()
        .filter(|candidate| is_selectable(candidate) && extra(candidate))
        .try_fold(0u128, |sum, candidate| {
            sum.checked_add(candidate.weight as u128)
                .ok_or(PickerError::ArithmeticOverflow)
        })?;
    if total == 0 {
        return Err(PickerError::NoEligibleCandidate);
    }
    let mut ticket = entropy % total;
    for candidate in candidates {
        if !is_selectable(candidate) || !extra(candidate) {
            continue;
        }
        let weight = candidate.weight as u128;
        if ticket < weight {
            return Ok(candidate);
        }
        ticket -= weight;
    }
    Err(PickerError::ArithmeticOverflow)
}

fn pick_priority(candidates: &[RouteCandidate]) -> Result<&RouteCandidate, PickerError> {
    candidates
        .iter()
        .filter(|candidate| is_selectable(candidate))
        .min_by_key(|candidate| candidate.order)
        .ok_or(PickerError::NoEligibleCandidate)
}

fn pick_lowest_latency(input: PickerInput<'_>) -> Result<&RouteCandidate, PickerError> {
    let mature_best = input
        .candidates
        .iter()
        .filter(|candidate| {
            is_selectable(candidate)
                && candidate.adaptive.latency_samples >= LATENCY_MATURE_SAMPLES
                && candidate.adaptive.latency_p50_micros.is_some()
        })
        .filter_map(|candidate| candidate.adaptive.latency_p50_micros)
        .min();
    let has_bootstrap = input
        .candidates
        .iter()
        .any(|candidate| is_selectable(candidate) && candidate.adaptive.latency_samples < LATENCY_MATURE_SAMPLES);

    let Some(best) = mature_best else {
        return pick_weighted(input.candidates, input.entropy, |candidate| {
            candidate.adaptive.latency_samples < LATENCY_MATURE_SAMPLES
        });
    };
    if has_bootstrap
        && input.selection_sequence % u64::from(LATENCY_EXPLORATION_INTERVAL)
            == u64::from(LATENCY_EXPLORATION_INTERVAL - 1)
    {
        return pick_weighted(input.candidates, input.entropy, |candidate| {
            candidate.adaptive.latency_samples < LATENCY_MATURE_SAMPLES
        });
    }
    pick_weighted(input.candidates, input.entropy, |candidate| {
        candidate.adaptive.latency_samples >= LATENCY_MATURE_SAMPLES
            && candidate.adaptive.latency_p50_micros == Some(best)
    })
}

fn pick_lowest_usage(candidates: &[RouteCandidate]) -> Result<&RouteCandidate, PickerError> {
    let mut selected: Option<&RouteCandidate> = None;
    let mut best_score = u128::MAX;
    for candidate in candidates.iter().filter(|candidate| is_selectable(candidate)) {
        let score = u128::from(candidate.adaptive.charged_tokens_60s)
            .checked_mul(SCORE_SCALE)
            .ok_or(PickerError::ArithmeticOverflow)?
            / candidate.weight as u128;
        if score < best_score || (score == best_score && selected.is_none_or(|current| candidate.order < current.order))
        {
            best_score = score;
            selected = Some(candidate);
        }
    }
    selected.ok_or(PickerError::NoEligibleCandidate)
}

fn pick_cost_weighted(input: PickerInput<'_>) -> Result<&RouteCandidate, PickerError> {
    let mut costs = [None; MAX_AI_ROUTE_CANDIDATES];
    let mut min_nonzero = u128::MAX;
    let mut has_zero = false;
    let mut eligible = 0usize;
    for (index, candidate) in input.candidates.iter().enumerate() {
        if !is_selectable(candidate) {
            continue;
        }
        eligible += 1;
        let price = candidate.cost.ok_or(PickerError::MissingCostInput)?;
        let output_bound = input
            .request_max_output_tokens
            .or(price.service_max_output_tokens)
            .ok_or(PickerError::MissingCostInput)?;
        let input_cost = ceil_product_div(
            input.input_tokens,
            price.input_price_micro_usd_per_million,
            PRICE_DENOMINATOR,
        )?;
        let output_cost = ceil_product_div(
            output_bound,
            price.output_price_micro_usd_per_million,
            PRICE_DENOMINATOR,
        )?;
        let cost = input_cost
            .checked_add(output_cost)
            .ok_or(PickerError::ArithmeticOverflow)?;
        costs[index] = Some(cost);
        if cost == 0 {
            has_zero = true;
        } else {
            min_nonzero = min_nonzero.min(cost);
        }
    }
    if eligible == 0 {
        return Err(PickerError::NoEligibleCandidate);
    }

    let mut weights = [0u128; MAX_AI_ROUTE_CANDIDATES];
    let mut total = 0u128;
    for (index, candidate) in input.candidates.iter().enumerate() {
        let Some(cost) = costs[index] else { continue };
        let selection_weight = if has_zero {
            if cost == 0 {
                candidate.weight as u128
            } else {
                0
            }
        } else {
            let numerator = SCORE_SCALE
                .checked_mul(
                    min_nonzero
                        .checked_mul(min_nonzero)
                        .ok_or(PickerError::ArithmeticOverflow)?,
                )
                .ok_or(PickerError::ArithmeticOverflow)?;
            let denominator = cost.checked_mul(cost).ok_or(PickerError::ArithmeticOverflow)?;
            let inverse_square = numerator / denominator;
            (candidate.weight as u128)
                .checked_mul(inverse_square)
                .ok_or(PickerError::ArithmeticOverflow)?
                .max(1)
        };
        weights[index] = selection_weight;
        total = total
            .checked_add(selection_weight)
            .ok_or(PickerError::ArithmeticOverflow)?;
    }
    if total == 0 {
        return Err(PickerError::NoEligibleCandidate);
    }
    let mut ticket = input.entropy % total;
    for (index, candidate) in input.candidates.iter().enumerate() {
        let weight = weights[index];
        if ticket < weight {
            return Ok(candidate);
        }
        ticket -= weight;
    }
    Err(PickerError::ArithmeticOverflow)
}

fn ceil_product_div(left: u64, right: u64, denominator: u128) -> Result<u128, PickerError> {
    let product = u128::from(left)
        .checked_mul(u128::from(right))
        .ok_or(PickerError::ArithmeticOverflow)?;
    if product == 0 {
        return Ok(0);
    }
    product
        .checked_add(denominator - 1)
        .map(|value| value / denominator)
        .ok_or(PickerError::ArithmeticOverflow)
}

#[cfg(test)]
mod tests {
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
}
