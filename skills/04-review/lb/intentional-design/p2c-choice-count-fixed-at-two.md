---
name: p2c-choice-count-fixed-at-two
description: Use when reviewing findings that LeastConn/EWMA should expose a configurable P2C choice count like Envoy least_request.choice_count — K=2 with distinct sampling is the accepted default; Envoy's knob is not copied.
status: accepted-tradeoff
finding_id: configurable-p2c-choice-count
closed: 2026-09-03
knowledge_type: intentional-design
---

# P2C choice count stays fixed at two — DESIGN-TRADEOFF

## Conclusion

LeastConn and EWMA keep a shared, allocation-free Power-of-Two-Choices selector that samples
**two distinct** backends (`edgion-gateway/src/lb/selection/p2c.rs`). K=2 is the product default,
not an unfinished Envoy-parity gap. Shipping `loadBalancer.choiceCount`, generalizing the hot-path
sampler to K in 3..=8, copying Envoy's with-replacement loop, or adding `FULL_SCAN` as the normal
path are **not accepted**.

## Core Rationale

**1. Mitzenmacher P2C already supplies the large gain at K=2.**

Sampling two random candidates and keeping the lower metric is O(1), resists herding, and is the
industry default. Raising K has diminishing returns and extra RNG plus atomic loads on every
LeastConn/EWMA request. There is no production evidence that Edgion's current selector is
imbalanced because K is two.

**2. Linkerd/Tower match Edgion; Envoy's knob is the outlier.**

Linkerd2-proxy uses Tower `balance::p2c::Balance`: `sample_floyd2` always draws two distinct ready
indices. The control-plane `BalanceP2c` message configures the load estimator (Peak EWMA /
PenaltyPeakEwma), not K. Tower even specialized the sampler to a two-draw helper rather than a
generic K-sampler.

Envoy Least Request exposes `choice_count` (default 2, proto `gte: 2`, no upper bound) and an
optional `FULL_SCAN` method. Its N-choices loop draws **with replacement** (`random() % N` K
times). Envoy issue #20420 records the consequence: two hosts and K=2 still send about 25% of
picks to the busier host; K=N does not become true least-request. Edgion already avoids that by
forcing distinct indices (including the n=2 `1 - i` fast path). Copying Envoy's knob would regress
that property unless we also keep distinct sampling, at which point we are not matching Envoy
anyway.

**3. Linkerd-shaped follow-up is metric work, not a wider sampler.**

Linkerd's recent LB investment is Peak EWMA decay and 429/`Retry-After` penalties, not more
choices. Those evaluations already live in `tasks/todo/time-decayed-peak-ewma-load-balancing.md`
and `tasks/todo/retry-after-backend-selection-penalty.md`. Do not reopen this decision to smuggle
those features into a choice-count change.

## Fix Suggestions Not Accepted

- Expose `EdgionBackendTrafficPolicy.spec.loadBalancer.choiceCount` (or an equivalent annotation)
  because Envoy has `choice_count` — public API and CRD coverage without a proven K>2 benefit.
- Generalize `select_p2c` to sample K in 3..=8 on the request path "just in case" — hot-path cost
  for an unused knob; keep K=2 until a dedicated investigation produces repeatable benches.
- Copy Envoy's with-replacement N-choices loop — reintroduces duplicate samples and the two-host
  25% busy-host pick.
- Treat `FULL_SCAN` or an O(log N) min-heap as the default LeastConn/EWMA path — out of scope here
  (the in-source P2C heap TODO is a separate, still-unproven evaluation).

## Re-evaluation Triggers

Re-open this decision only if:

- Benchmarks on representative backend counts (including 8, 32, 256, 1000), equal/skewed/changing
  metrics, and unhealthy ratios 10%/50%/90% show a repeatable material improvement for some K>2
  with bounded extra CPU, and the implementation stays allocation-free with distinct sampling.
- Production data shows P2C underperforming on highly skewed services *and* the cause is sampling
  breadth rather than the EWMA/LeastConn metric itself.

## Reference Cases

- Closed task `tasks/todo/configurable-p2c-choice-count.md` (2026-09-03)
- `edgion-gateway/src/lb/selection/p2c.rs`, callers in `lb/compiled_selector.rs`
- Envoy `LeastRequest.choice_count` / `unweightedHostPickNChoices`
- Tower `p2c_ready_index` + `sample_floyd2`; Linkerd `BalanceP2c`
- Sibling evaluations: `tasks/todo/time-decayed-peak-ewma-load-balancing.md`,
  `tasks/todo/retry-after-backend-selection-penalty.md`

## Resolution references

- `configurable-p2c-choice-count` — 2026-09-03 (accepted tradeoff).
