//! Pure, bounded AI route-candidate selection.
//!
//! Callers supply immutable eligibility/telemetry snapshots and entropy. This
//! module performs no locking, sorting, floating-point arithmetic, I/O, or
//! mutation of adaptive state on the request path.

use edgion_resources::resources::edgion_plugins::AiBackendSelection;

use crate::lb::selection::p2c::skip_excluded_index;

pub const MAX_AI_ROUTE_CANDIDATES: usize = 16;
const LATENCY_MATURE_SAMPLES: u32 = 8;
/// An unknown candidate is probed again only after this long without any
/// admission; it matches the adaptive latency window.
const LATENCY_PROBE_INTERVAL_SECONDS: u64 = 30;
/// A sample-less candidate outside its probe is scored as this many times
/// the worst mature p50: exploration belongs to the probe, and attempts that
/// end without a sample or failure verdict must not look optimistic.
const UNKNOWN_LATENCY_PENALTY: u64 = 2;
const SCORE_SCALE: u128 = 1_000_000;
/// P2C draws use entropy bits 80..128: a UUIDv4's fixed version and variant
/// bits sit below bit 80, and the credential pick consumes the low 64 bits.
const P2C_ENTROPY_SHIFT: u32 = 80;
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

/// Integer values published by the bounded background telemetry aggregator,
/// plus the live in-flight counters read at selection time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AdaptiveSnapshot {
    pub latency_p50_micros: Option<u64>,
    /// Successful TTFT samples in the latency window.
    pub latency_samples: u32,
    /// Attempts with a backend-failure verdict in the latency window.
    pub latency_failures: u32,
    pub charged_tokens_60s: u64,
    /// Admitted attempts that have not finished yet (live).
    pub in_flight: u32,
    /// Provisional tokens of in-flight attempts (live).
    pub in_flight_tokens: u64,
    /// Unix second of the latest admission attempt, 0 if none (live).
    pub last_attempt_second: u64,
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
    /// Caller-supplied current Unix second for the LowestLatency idle probe.
    pub now_second: u64,
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
        AiBackendSelection::LowestLatency => pick_lowest_latency(input.candidates, input.entropy, input.now_second),
        AiBackendSelection::LowestUsage => pick_lowest_usage(input.candidates, input.entropy),
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

fn pick_weighted(
    candidates: &[RouteCandidate],
    entropy: u128,
    extra: impl Fn(&RouteCandidate) -> bool,
) -> Result<&RouteCandidate, PickerError> {
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

/// Power-of-Two-Choices over the selectable candidates: two distinct
/// candidates are drawn from request entropy and the lower score wins; a tie
/// keeps the first draw, which is itself random. Adaptive scores lag behind
/// completions, so a global argmin would herd every concurrent request onto
/// one backend; the random pair plus the live in-flight term in each score
/// spreads load the way the general `Ewma`/`LeastConn` balancer does.
fn pick_p2c(
    candidates: &[RouteCandidate],
    entropy: u128,
    score: impl Fn(&RouteCandidate) -> Result<u128, PickerError>,
) -> Result<&RouteCandidate, PickerError> {
    let mut eligible = [0u8; MAX_AI_ROUTE_CANDIDATES];
    let mut count = 0usize;
    for (index, candidate) in candidates.iter().enumerate() {
        if is_selectable(candidate) {
            // validate_candidates bounds the slice, so index fits in u8.
            eligible[count] = index as u8;
            count += 1;
        }
    }
    match count {
        0 => Err(PickerError::NoEligibleCandidate),
        1 => Ok(&candidates[usize::from(eligible[0])]),
        n => {
            let draw = entropy >> P2C_ENTROPY_SHIFT;
            let n_wide = n as u128;
            let first = (draw % n_wide) as usize;
            let second = skip_excluded_index(((draw / n_wide) % (n_wide - 1)) as usize, first);
            let a = &candidates[usize::from(eligible[first])];
            let b = &candidates[usize::from(eligible[second])];
            Ok(if score(b)? < score(a)? { b } else { a })
        }
    }
}

/// Lowest-is-better LowestLatency score:
/// `p50 * (in_flight + 1) * (successes + failures + 1) / ((successes + 1) * weight)`.
///
/// The in-flight factor reacts at admission, so a burst or a hanging backend
/// is pushed away before any completion is published. The success-ratio factor
/// keeps fast failures from looking like a fast backend. With no mature
/// candidate every latency term is 1 (immature p50s are ignored) and the
/// score is weighted least-request scaled by the success ratio.
///
/// A candidate without a mature p50 (fewer than 8 successes in the window)
/// uses its own immature p50 when it has one. With no sample at all it scores
/// zero (an idle probe) when nothing is in flight and nothing was admitted to
/// it for the last probe interval, admitted or refused; otherwise the weight divisor would starve
/// a lower-weight unknown backend at low concurrency. Gating on admission
/// rather than on outcomes bounds probing to about one attempt per interval
/// however the probe ends. Outside the probe it is scored pessimistically at
/// `UNKNOWN_LATENCY_PENALTY` times the worst mature p50.
fn pick_lowest_latency(
    candidates: &[RouteCandidate],
    entropy: u128,
    now_second: u64,
) -> Result<&RouteCandidate, PickerError> {
    let mut mature = candidates
        .iter()
        .filter(|candidate| is_selectable(candidate))
        .filter_map(mature_latency);
    let worst_mature = mature.next().map(|first| mature.fold(first, u64::max));
    pick_p2c(candidates, entropy, |candidate| {
        let adaptive = candidate.adaptive;
        let latency = match (mature_latency(candidate), adaptive.latency_p50_micros, worst_mature) {
            (Some(latency), _, _) => latency,
            (None, Some(own), Some(_)) => own,
            (None, None, Some(_))
                if adaptive.in_flight == 0
                    && (adaptive.last_attempt_second == 0
                        || now_second.saturating_sub(adaptive.last_attempt_second)
                            >= LATENCY_PROBE_INTERVAL_SECONDS) =>
            {
                return Ok(0)
            }
            (None, _, Some(worst)) => worst.saturating_mul(UNKNOWN_LATENCY_PENALTY),
            (None, _, None) => 1,
        };
        let successes = u128::from(adaptive.latency_samples);
        let attempts = successes + u128::from(adaptive.latency_failures) + 1;
        let numerator = u128::from(latency)
            .saturating_mul(u128::from(adaptive.in_flight) + 1)
            .saturating_mul(attempts)
            .saturating_mul(SCORE_SCALE);
        Ok(numerator / ((successes + 1) * candidate.weight as u128))
    })
}

fn mature_latency(candidate: &RouteCandidate) -> Option<u64> {
    (candidate.adaptive.latency_samples >= LATENCY_MATURE_SAMPLES)
        .then_some(candidate.adaptive.latency_p50_micros)
        .flatten()
}

/// Lowest-is-better LowestUsage score: recent charged tokens plus the
/// provisional tokens of in-flight attempts, divided by route weight.
fn pick_lowest_usage(candidates: &[RouteCandidate], entropy: u128) -> Result<&RouteCandidate, PickerError> {
    pick_p2c(candidates, entropy, |candidate| {
        let usage = u128::from(candidate.adaptive.charged_tokens_60s) + u128::from(candidate.adaptive.in_flight_tokens);
        Ok(usage.checked_mul(SCORE_SCALE).ok_or(PickerError::ArithmeticOverflow)? / candidate.weight as u128)
    })
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
#[path = "../../../tests/unit/ai/routing/candidate_test.rs"]
mod tests;
