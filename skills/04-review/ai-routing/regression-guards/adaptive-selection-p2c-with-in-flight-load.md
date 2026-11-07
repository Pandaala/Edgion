---
name: adaptive-selection-p2c-with-in-flight-load
description: Use when reviewing AI LowestLatency / LowestUsage backend selection, adaptive telemetry in-flight leases, or proposals to select the global minimum of published adaptive metrics.
knowledge_type: regression-guards
---

# AI adaptive selection is P2C over in-flight-aware scores

## Conclusion and scope

`LowestLatency` and `LowestUsage` (`edgion-gateway/src/ai/routing/candidate.rs`) select with
Power-of-Two-Choices: two **distinct** selectable candidates are drawn from request entropy
(bits 80..128; the credential pick uses the low 64 bits) and the lower score wins;
a tie keeps the first draw. Distinct sampling reuses `lb::selection::p2c::skip_excluded_index`.
Neither mode may return to a deterministic argmin over the published snapshot.

Scores (lower wins, route weight is a capacity divisor):

- `LowestLatency`: `p50 × (in_flight + 1) × (successes + failures + 1) / ((successes + 1) × weight)`.
  When some candidate is mature, a candidate without a mature p50 (< 8 successes in 30 s) uses
  its own immature p50 when it has one. With no sample it scores zero (an idle probe) only while
  nothing is in flight and no attempt entered admission for it in 30 s (`last_attempt_second`,
  live, stamped by `AiAttemptRuntimeState::admit` before circuit, outlier, and quota checks, so a
  refused admission also closes the probe); otherwise it
  is scored at `UNKNOWN_LATENCY_PENALTY` (2) times the worst mature p50. Stream cuts after
  headers, client-class statuses, in-body provider failures, client aborts and quota cuts record
  neither a sample nor a failure. Gating the probe on admission (not outcome) and scoring the
  non-probe case pessimistically together keep such a backend from taking most low-concurrency
  traffic: an outcome-gated probe would hold it at score zero, and an optimistic borrowed p50
  would tie with (or, at higher weight, beat) the idle best backend. With no mature candidate every
  latency term is 1 (immature p50s are ignored): weighted least-request scaled by the success ratio. There is no periodic exploration sequence.
- `LowestUsage`: `(charged_tokens_60s + in_flight_tokens) / weight`.

In-flight load is live, not published: `AiAttemptRuntimeState::admit` creates an
`AdaptiveInFlightLease` (`routing/adaptive.rs`) that adds one attempt and its provisional
`estimated_input + max_output` tokens (capped per lease at `u32::MAX`). The lease is an RAII
field of `AiAttemptRuntime`, so every completion, failure, pre-dispatch release, and request
abort drops it exactly once. `AdaptiveTelemetryState::snapshot()` overlays these live counters
on the one-second published windows. Backend-failure verdicts (`record_failure_once`, the same
verdict that feeds outlier detection) are counted in the 30 s latency window.

## Rationale and evidence

The replaced pickers took the global minimum of metrics updated only when attempts finished
and republished once per second. Every request admitted between completions saw the same
snapshot, so bursty long-lived LLM traffic herded onto one backend and oscillated, and a
hanging backend kept its stale fast p50 and all traffic. Instant in-flight terms plus a random
pair is the herd-resistant shape already used by Edgion's `Ewma`/`LeastConn` balancer
([P2C choice count](../../lb/intentional-design/p2c-choice-count-fixed-at-two.md)) and by
agentgateway's LLM balancer (P2C, `latency × (1 + pending × 0.1)`, health EWMA; per
<https://agentgateway.dev/docs/kubernetes/latest/documentation/llm/load-balancing/>, read
2026-11-06). LiteLLM's `latency-based-routing` (`router_strategy/lowest_latency.py`, main branch
read 2026-11-06) keeps the completion-only argmin and is the counter-example. These external
descriptions are context, not the authority for this rule.

## Alternatives and limits

- Route weight is a score divisor (capacity), not a sampling weight: with two candidates the
  pair is fixed, so sampling weights alone could not express weight. Consequence: at low
  concurrency an idle higher-weight peer wins every pair. Without the idle probe, a lower-weight
  unknown backend would never collect samples. The probe bounds exploration to about one
  attempt per 30 s per unknown candidate and replica. Outside the probe a sample-less candidate
  still wins pairs once the alternatives carry more load than the penalty, or when its route
  weight exceeds the penalty ratio (weight is capacity). A known-slower backend
  whose samples age out is re-probed once, then scored on its own immature p50. The bound is
  approximate: picks made between the selection and the synchronous `admit` stamp also see it
  as idle. Stamping before the checks and awaits matters: a stamp in the lease would leave an
  unknown backend that keeps refusing admission (open circuit, ejected credential, exhausted
  quota) at score zero forever, costing a failed admission, possibly a Redis round trip, on
  about 2/n of selections.
- The success-ratio factor is bounded: a high-weight backend failing fast can still win some
  pairs until about `weight ratio` failures accumulate. Outlier ejection and retries remain the
  primary failure guard.
- Failure verdicts follow outlier semantics: an attempt cut after headers without EOS records no
  verdict, so it penalizes only while in flight.
- Sample-less endings (client-class statuses, cuts after headers, in-body provider failures)
  add no penalty, so a backend that mostly ends that way but records occasional fast successes
  is scored on that fast immature p50. Counting sample-less dispatched endings is out of scope.
- In-flight and usage telemetry are scoped per route rule, backend, and logical model; load
  from other routes sharing a backend is not visible.
- An unset output bound makes the provisional usage term input-only.

- Peak-EWMA TTFT (`lb/ewma/score.rs`) was not reused: its `DEFAULT_RTT_US` (30 ms) neutral seed
  is tuned for HTTP RTT and would make every new AI backend look far faster than real TTFTs. The
  bounded windowed p50 stays; the in-flight factor provides the reactive term.
- Settled usage enters the bucket of the current second and is published by the next tick,
  while the lease releases immediately: one attempt's charge can be invisible for up to one
  second. Accepted as negligible against the 60 s window.
- Replicas still decide independently; random pairs and local in-flight counts decorrelate them.
- `gw-ai3-05` (bucket rotation drops samples) affects the same telemetry and is tracked separately.

## Verification and re-evaluation

Unit tests in `edgion-gateway/tests/unit/ai/routing/candidate_test.rs` cover distinct-pair
coverage, faster-wins with a larger-weight slower peer, in-flight push-away, weight-as-capacity,
failure penalty, the admission-gated idle probe, own immature p50, saturation, and usage
in-flight tokens; `adaptive_test.rs` covers lease visibility/release and the failure window;
`attempt_runtime_test.rs::adaptive_in_flight_lease_spans_admission_to_every_attempt_end` covers
the refused-admission probe stamp, admission-to-release wiring, and one failure verdict per
attempt. Verified 2026-11-07 with
`cargo test -p edgion-gateway --lib -- ai:: lb::selection` (650 passed). No integration
scenario exercises adaptive selection end to end. Re-evaluate if production shows unknown
backends absorbing bursts during admission, or the 30 s probe interval proving too slow or
too frequent for low-traffic routes. Re-evaluate if a benchmark shows the success-ratio or
unknown-latency penalty misallocates traffic, or if adaptive
telemetry moves to Peak-EWMA with an AI-appropriate neutral seed.

## Resolution references

- `gw-ai-02` — adaptive selection argmin herding, fixed 2026-11-06 (feature-10-28).
