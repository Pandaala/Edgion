---
name: route-predicate-prefilter-not-justified
description: Use when reviewing a build-time header/query predicate prefilter in front of HTTP route deep matching; covers the unmeasured candidate fan-out, existing direct listener-lookup optimizations, and rejected secondary-index designs.
status: wont-fix
finding_id: task-add-route-predicate-prefilter
closed: 2026-09-03
knowledge_type: intentional-design
---

# A build-time route predicate prefilter is not justified — WONT-FIX

## Conclusion

`RadixRouteMatchEngine::match_route` narrows a request **by radix path only**, then runs
`HttpRouteRuleUnit::deep_match` over the surviving candidates in build-time precedence
order; the listener/hostname check is step 0 *inside* `deep_match_common`, paid per
candidate. Adding a second, predicate-keyed index in front of that loop — a build-time
inverted table from a header or query value to a candidate set, intersected with the radix
candidates at request time — is **not accepted on present evidence**: the fan-out it
targets has not been measured in the evidence supporting this decision. Direct listener
lookup optimizations are already present; their existence does not establish that the
remaining deep-match cost justifies a second index.

Fix suggestions of the form "index routes by their header/query predicates so `match_route`
skips candidates that cannot match", "port Kong's `atc-router` optional-field prefilter", or
"add a request-time bitmap intersection before deep matching" are **not accepted** as review
findings.

This is a *no-evidence* verdict, not a proof of no value. The re-evaluation triggers below
are concrete; do not cite this record as though the question were settled on measurement.

## Core Rationale

**1. What a prefilter can and cannot remove**

A conservative prefilter may drop a candidate only when that candidate carries a predicate on
the indexed dimension **and** its required value mismatches the request. A candidate carrying
no predicate on that dimension matches every value of it and must be retained
unconditionally — otherwise the prefilter produces a false negative and the route becomes
unreachable.

This is *not* limited to same-path siblings. `FrozenRadixTree::match_all_ext`
(`edgion-algo/src/matcher/radix_path/frozen.rs`) returns every valued node along the
request path, so `/api/v1/checkout` yields the `/api`, `/api/v1` and `/api/v1/checkout`
buckets together, and a prefix ancestor may well carry a header predicate of its own. The
saving is therefore bounded by the number of returned candidates carrying a mismatching
predicate on the indexed dimension — across all buckets, not within one.

**2. The fan-out is unmeasured, in either shape**

The originating task named one specific shape: routes sharing the same listener, host and
radix path, separated only by predicates. For *that* shape the saving is bounded by
`tree_value_paths` bucket depth, and the 2026-09-02 candidate-ordering review observed that
buckets "are almost always length 1" (recorded in
`tasks/todo/radix-route-matching-benchmarks.md`). That is a design note carrying **no stated
empirical basis** — it is not backed by the throwaway microbenchmark mentioned elsewhere in
that task, which measured a different bullet — and it says nothing about the cross-bucket case
in point 1.

So neither shape has a number. That is the whole of the evidentiary position.

**3. Account for the current listener lookup, not its historical allocation cost**

`deep_match_common` (`edgion-gateway/src/routes/http/match_unit.rs`) rejects a non-matching
candidate through: step 0 listener/hostname, step 1 optional method comparison, step 2
headers, step 3 query.

Steps 1–3 are close to minimal and allocation-free. A header matcher is one
`req_header.headers.get(name)` plus either `==` or a build-time-compiled `Regex::is_match`.
Query matching is gated on `has_query_matchers` and shares a single `ctx.cached_query()`
parse across every candidate of the request, so a route without query predicates never
touches the query string.

Step 0 is not free, but the previously recorded per-candidate key allocation is no longer
current. Route snapshot construction precomputes `ResolvedParentRef.gateway_key` with
`format_gateway_key`; `check_gateway_listener_match`
(`edgion-gateway/src/runtime/matching/route.rs`) borrows those bytes. It resolves one
`GatewayListeners` borrow per parentRef and reuses that borrow for the named listener lookup
and specificity check, or for unnamed listener iteration. This removes the named path's
duplicate outer Gateway-key lookup without introducing another index.

Only immutable parent-reference data is precomputed. The Gateway index changes independently
of route snapshots, so listener slots, borrowed entries and matching results must not be
cached across generations. The remaining hostname, specificity and AllowedRoutes work still
runs per candidate. **A renewed prefilter proposal must measure that current path and show
that candidate fan-out still dominates; no throughput improvement is inferred here.**

**4. The task's own gate was never satisfied**

The originating task made retention conditional on "a benchmark corpus demonstrat[ing] a
material candidate fan-out after host/path matching". No such corpus exists: `edgion-gateway`
has no `benches/` directory and no `[[bench]]` target, and the harness task that would create
one (`tasks/todo/radix-route-matching-benchmarks.md`) is still pending. Landing the index now
would violate the acceptance criterion the task set for itself.

**5. Second-index designs conflict with the published-snapshot model**

The route snapshot is immutable and swapped wholesale. A prefilter that is anything other
than build-time data inside that snapshot — a request-time mutable index, a route-result
cache, or a general expression router — is rejected for the same reasons recorded in
[radix-candidate-precedence-rank-precomputed.md](../regression-guards/radix-candidate-precedence-rank-precomputed.md).
Unlike points 1–4, this rejection is unconditional and does not turn on measurement.

## Fix Suggestions Not Accepted

- "Skip candidates whose indexed predicate value differs from the request's" — a candidate
  with no predicate on that dimension matches every value; omitting the 'don't care' set makes
  an unpredicated route unreachable (false negative, 404). Any accepted design must carry that
  set explicitly.
- "Build an inverted `(header name, value) -> candidate set` index and intersect it with the
  radix candidates" — not accepted while the fan-out and remaining deep-match cost after
  the direct optimizations in point 3 are unmeasured.
- "Port the Kong `atc-router` optional-field prefilter wholesale" — Kong's router dispatches
  over a general expression language with no prior path narrowing; Edgion has already narrowed
  by radix path before this point, so the premise that motivates the prefilter does not
  transfer unchanged.
- "Add a route-result cache keyed by request path plus predicate values" — an explicit
  non-goal for this engine: unbounded key space and invalidation on every snapshot swap.
- "Add a second condition enum or a request-time mutable index for predicates" — breaks the
  single shared `Condition` layer and the immutable published-snapshot ownership model.

## Re-evaluation Triggers

**Measurement baseline.** The historical per-candidate `format_gateway_key` allocation has
already been removed, and the named path now reuses its Gateway lookup. Any measurement must
include these direct optimizations and establish that deep matching still dominates. Their
presence alone does not satisfy the prefilter's benchmark gate.

With that precondition satisfied, re-open this decision only if:

- The benchmark harness from `tasks/todo/radix-route-matching-benchmarks.md` exists and
  measures a material candidate count carrying mismatching predicates after radix narrowing —
  counting across buckets, per point 1, not bucket depth alone — with deep matching, rather
  than candidate collection or ordering, dominating match cost.
- A real deployment shape appears in which many routes share one listener, hostname and path
  string and are separated only by header or query predicates (large-scale header-based canary
  or tenant fan-out).
- `deep_match` gains a genuinely expensive per-candidate step (for example a predicate
  requiring body inspection or an external lookup), changing the cost balance this decision
  rests on.

## Reference Cases

- Task "Add a Benchmark-Gated Route Predicate Prefilter"
  (`tasks/todo/add-route-predicate-prefilter.md`), created 2026-09-01, closed 2026-09-03
  without production code — the benchmark gate it defined for itself was never satisfied.
- `edgion-gateway/src/routes/http/match_engine/radix_route_match.rs` — `ordered_candidates`
  and the deep-match loop
- `edgion-gateway/src/routes/http/match_unit.rs` — `deep_match_common`, `match_header`,
  `match_query_param`
- `edgion-gateway/src/runtime/matching/route.rs` and
  `edgion-gateway/src/runtime/store/listener_gateway_info.rs` — precomputed parent-reference
  keys and generation-local Gateway/listener lookup ownership
- `edgion-algo/src/matcher/radix_path/frozen.rs` — `match_all_ext` returns every valued
  node along the path
- `tasks/todo/radix-route-matching-benchmarks.md` — the prerequisite measurement surface and
  the bucket-depth observation
- External reference considered and rejected: Kong ATC optional field prefilter,
  <https://github.com/Kong/atc-router>
- Sibling ordering rule:
  [radix-candidate-precedence-rank-precomputed.md](../regression-guards/radix-candidate-precedence-rank-precomputed.md)

## Resolution references

- `task-add-route-predicate-prefilter` — 2026-09-03 (wont-fix on present evidence).
