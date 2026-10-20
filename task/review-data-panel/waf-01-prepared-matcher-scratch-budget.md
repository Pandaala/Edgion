# Evaluate generation-prepared WAF matcher scratch budgets

Status: deferred for user decision. Priority: P2. Type: performance investigation.
Inspected baseline: `6e41d58f9`.

## Historical evidence and current scope

At the inspected baseline, the Gateway WAF request hook called
Transaction::new_shared_with_policy. In
`edgion-waf/src/seclang/transaction.rs`, new_with_handle built RequestState,
computed matcher_scratch_bytes, checked scratch and aggregate retained limits,
allocated three vectors of empty Option slots, then called matcher_scratch_capacity
again with actual vector capacities. Both calculations traversed all_matchers,
boolean_regexes and capture_regexes. They summed each program's conservative scratch
envelope minus its inline scratch struct, then added Option slot storage.

The compiled program inputs are immutable during request execution. However,
`CompiledGeneration::publish_with_work` in compiler.rs installs optional capture-hit
matchers and regex hints after initial generation assembly. all_matchers in
capture_hit_plan.rs includes both ordinary groups and optional capture-hit matchers.
A cold cached envelope must reflect the final published generation, including
optional engines and any hint-dependent scratch requirements.

Engine scratch is already lazy: lazy_group_scratch, lazy_boolean_scratch and
lazy_capture_scratch initialize their individual request-owned slots on first use.
The historical repeated work was budget computation, not duplicate engine allocation.
The full generation envelope is conservative admission, not measured heap or RSS.
See `skills/04-review/waf/regression-guards/fused-json-inspection.md`, especially
Demand-driven scratch and static activation and optional capture-hit ownership.

## Decision and boundaries

Evaluate storing the checked program-envelope subtotal in the immutable generation
at its final publication point. Each request would still account for requested
and actual Option capacities and perform its existing runtime-limit checks.
Constructor-local subtotal reuse is now implemented: one program walk supplies
both requested-slot admission and actual-capacity reconciliation. No generation
metadata was added. Evaluate further cold preparation against this updated
baseline, or retain local reuse if measurements do not justify added compiled
state. The remaining decision concerns eliminating the first per-request walk.

Preserve RequestState validation before scratch admission. Moving arithmetic to
compilation must not silently replace a runtime Arithmetic outcome with compiler
rejection or reorder request errors. This is a preservation obligation, not a
claim that a practical overflow has been demonstrated. Account for any new
compiled field in the existing compiler memory contract and initialize it only
after all optional engines and hints are final. Do not infer that an early empty
capture-hit plan is the published shape.

Keep full-generation admission, actual-capacity reconciliation, aggregate retained
bounds and lazy first-use engine initialization. Do not charge only reached rules,
pool mutable scratch across requests, change engine layouts, or reinterpret the
retained-byte metric as actual heap. No body cache, schema, tracing or per-request
profiling metric is needed.

## Acceptance for later implementation

- Compare runtime results and budget errors for empty and mixed generations,
  optional capture-hit engines, regex hints, shared matchers, boolean selectors
  and capture extraction. Check zero-use and first/repeated-use paths.
- Preserve exact and one-below scratch and aggregate retained limits, malformed
  request priority, checked arithmetic, and actual slot capacities. Retain
  lazy_program_slots_initialize_once_and_preserve_generation_wide_admission and
  the existing literal request-head retained-budget tests.
- Verify cold metadata after optional compilation fallbacks and publication;
  preserve compiler retained/peak accounting if generation storage changes.
- Measure program traversal counts and constructor CPU cost separately from
  request-head parsing and first-use engine allocation on representative small
  and large rule sets. Report preparation cost and per-generation storage too.
- Run the leaf unit suite, Gateway WAF tests, workspace checks and relevant guards.

Generation-prepared budgets remain unimplemented. The subsequent constructor-local
reuse preserves runtime admission and is recorded in INDEX.md; it does not resolve
this broader proposal or supply a performance measurement.
