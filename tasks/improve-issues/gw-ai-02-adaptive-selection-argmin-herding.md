# gw-ai-02: LowestLatency / LowestUsage are deterministic argmin over lagging telemetry (herding), diverging from the canonical P2C/Peak-EWMA balancer

- **Category**: design-error
- **Severity**: P2 (worth doing)
- **Scope**: local
- **Location**:
  - `edgion-gateway/src/ai/routing/candidate.rs:185-232` (`pick_lowest_latency`, `pick_lowest_usage`)
  - `edgion-gateway/src/ai/routing/adaptive.rs:221-290, 479-492` (`record_*`, 1 s published snapshot, 30 s / 60 s windows)
  - `edgion-gateway/src/ai/runtime/attempt_runtime.rs:570-627` (`commit_ledger`: usage and TTFT recorded only when an attempt finishes)
  - `edgion-gateway/src/ai/runtime/dispatch.rs:735-790` (`select_target`)
  - Canonical comparison: `edgion-gateway/src/lb/selection/p2c.rs`, `skills/12-algorithms/lb-peak-ewma.md`

## Summary

Both adaptive AI selection modes pick the single global minimum of a per-instance, per-second
published metric that is only updated when attempts *finish*. With LLM requests lasting seconds to
minutes, every request admitted between two completions sees the same snapshot and goes to the
same backend. This is the textbook "least-loaded with stale information" herd that Edgion's own
general load balancer already avoids with Power-of-Two-Choices over an in-flight-aware Peak-EWMA.
The AI path re-implements latency-aware selection with a weaker algorithm instead of reusing it.

## Current behavior (evidence)

- `pick_lowest_usage` (`candidate.rs:217`) returns the candidate with minimal
  `charged_tokens_60s / weight` — a strict argmin, ties to declaration order (not random).
- `charged_tokens_60s` grows only in `commit_ledger` (`attempt_runtime.rs:615-618`), i.e. after an
  attempt ends, and is republished at most once per second by `run_adaptive_telemetry_ticker`
  (`adaptive.rs:479`). In-flight work is invisible. A burst of N concurrent streaming requests,
  each lasting 20–60 s, all land on the same backend until the first of them completes; that
  backend then gets a large step of usage and the whole stream of new traffic flips to the next
  backend (oscillation), which also pushes one credential pool into provider 429s.
- `pick_lowest_latency` (`candidate.rs:185`) routes **all** mature traffic to candidates whose
  p50 TTFT equals the minimum (histogram-bucketed); others get traffic only through 1-in-20
  exploration and only while they are *immature* (< 8 samples in 30 s). Once a backend is mature
  but slightly slower, it receives zero traffic until its samples age out of the 30 s window, then
  bounces through exploration. Because samples come only from successful completions, a backend
  that suddenly starts hanging keeps its old fast p50 (and therefore 100 % of the traffic) for up
  to 30 s unless outlier detection ejects the credential first.
- Each gateway replica runs this independently, so N replicas herd in lockstep (same inputs, same
  argmin), magnifying the burst seen by the chosen provider.

## Industry comparison

- Envoy `LEAST_REQUEST` uses P2C over *active request counts* (instant, not lagged); Envoy AI
  Gateway relies on the same Envoy LB primitives.
- Linkerd/Finagle Peak-EWMA (adopted by Edgion itself, `skills/12-algorithms/lb-peak-ewma.md`,
  `skills/04-review/lb/intentional-design/p2c-choice-count-fixed-at-two.md`: "Sampling two random
  candidates and keeping the lower metric is O(1), resists herding") combines latency EWMA with
  in-flight penalty, and samples two candidates.
- Kong `ai-proxy-advanced` offers similar "lowest-latency" / "lowest-usage" algorithms; whatever
  their scoring, Edgion's own general balancer already demonstrates the herd-resistant shape
  (randomized two-choice over in-flight-aware scores) that the AI path should reuse.

## Impact

Under exactly the conditions these modes are selected for (bursty, long-lived LLM traffic across
several providers), load concentrates on one provider/credential pool at a time, causing avoidable
provider 429s, credential cooldowns and higher tail latency, and it can mask a degrading backend
for up to the latency window.

## Suggested direction

Keep the user-facing enum but change the algorithm:

1. Account in-flight work at **dispatch** (increment an in-flight counter and, for LowestUsage, a
   provisional `estimated_input + bound_output` term that is replaced by the real charge at
   settlement), so admission-time decisions see concurrent load.
2. Select with the canonical P2C sampler (`lb/selection/p2c.rs`) weighted by route weight instead
   of argmin; for LowestLatency, score with Peak-EWMA (TTFT EWMA × (in-flight + 1)) as in the
   general balancer, and let failures/timeouts feed a penalty instead of being invisible.
3. Drop the bespoke "exploration every 20th request for immature candidates" rule — P2C provides
   continuous exploration.

This also satisfies the CLAUDE.md unification principle (one canonical latency-aware selection).

## Notes

- Searched `skills/04-review/ai-routing/*` (precompiled keys, store accessors, cost-weighted
  output bound, etc.) and `skills/04-review/lb/*`; no page accepts argmin/herding for AI selection.
  `docs/en/ai-gateway/routing.md:77-78` only describes "Chooses the lowest rolling p50 / lowest
  recent usage", not a herding tradeoff.
- Out of scope: cross-replica shared telemetry (not needed once P2C + in-flight accounting are used).

## Verification (independent)

- **Verdict**: CONFIRMED — Severity P2 unchanged.
- `candidate.rs:217-232` `pick_lowest_usage` is a strict argmin over `charged_tokens_60s/weight`, and ties go to declaration order. `pick_lowest_latency` (`:185-215`) sends mature traffic only to candidates whose p50 equals the minimum, and explores immature candidates only every `LATENCY_EXPLORATION_INTERVAL`.
- Usage and TTFT are recorded only in `commit_ledger` (`attempt_runtime.rs:615-627`), i.e. at attempt completion. No in-flight term exists, so the stale-information herd is real.
- The only related review page (`routing/regression-guards/lowest-latency-tests-must-distinguish-minimum.md`) covers test discrimination, not the algorithm choice. `docs/en/ai-gateway/routing.md:77-78` describes argmin without a herding tradeoff.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID — Severity P2 unchanged.
- `edgion-gateway/src/ai/routing/` and `runtime/dispatch.rs` are unchanged between feature-08-30 and HEAD.
- `routing/candidate.rs:185-215` (`pick_lowest_latency`, min-p50 + every-20th exploration) and `:217-232` (`pick_lowest_usage`, strict argmin with declaration-order ties) are byte-identical.
- Usage/TTFT are still recorded only at attempt completion in `commit_ledger` (`attempt_runtime.rs:614-626`); no in-flight term exists. No new review page accepts argmin selection.
