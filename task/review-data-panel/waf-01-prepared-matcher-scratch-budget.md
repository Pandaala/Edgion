# Evaluate generation-prepared WAF matcher scratch budgets

Status: deferred for user decision. Priority: P2. Type: performance investigation.
Inspected baseline: `6e41d58f9`.

## Evidence

The Gateway WAF request hook calls Transaction::new_shared_with_policy. In
`edgion-waf/src/seclang/transaction.rs`, new_with_handle builds RequestState,
computes matcher_scratch_bytes, checks scratch and aggregate retained limits,
allocates three vectors of empty Option slots, then calls matcher_scratch_capacity
again with actual vector capacities. Both calculations traverse all_matchers,
boolean_regexes and capture_regexes. They sum each program's conservative scratch
envelope minus its inline scratch struct, then add Option slot storage.

The compiled program inputs are immutable during request execution. However,
`CompiledGeneration::publish_with_work` in compiler.rs installs optional capture-hit
matchers and regex hints after initial generation assembly. all_matchers in
capture_hit_plan.rs includes both ordinary groups and optional capture-hit matchers.
A cold cached envelope must reflect the final published generation, including
optional engines and any hint-dependent scratch requirements.

Engine scratch is already lazy: lazy_group_scratch, lazy_boolean_scratch and
lazy_capture_scratch initialize their individual request-owned slots on first use.
The repeated work here is budget computation, not duplicate engine allocation.
The full generation envelope is conservative admission, not measured heap or RSS.
See `skills/04-review/waf/regression-guards/fused-json-inspection.md`, especially
Demand-driven scratch and static activation and optional capture-hit ownership.

## Decision and boundaries

Evaluate storing the checked program-envelope subtotal in the immutable generation
at its final publication point. Each request would still account for requested
and actual Option capacities and perform its existing runtime-limit checks.
Compare this with constructor-local subtotal reuse, which removes the second
program walk without adding generation metadata, and retaining the current code
if representative measurements do not justify either change. The local alternative
does not inherently require a new authorization boundary; this task records the
broader cold-preparation choice, not a technical prohibition on local reuse.

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

This record changes no runtime code and supplies no fresh performance measurement.
