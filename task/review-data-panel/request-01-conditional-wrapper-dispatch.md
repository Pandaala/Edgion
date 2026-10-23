# Request conditional-wrapper dispatch

Status: deferred optimization decision for the user. Baseline: `6532f2cb0`.
No runtime change or measured allocation saving.

## Evidence

The ordinary request-entry construction in
`edgion-gateway/src/plugins/runtime/plugin_runtime.rs` installs a
ConditionalRequestFilter outside an optional BodyGatedRequestFilter even when
entry conditions are absent. Specialized WAF/ExtProc construction paths also
use conditional wrappers; enumerate all constructors before implementation.

`conditional_filter.rs::ConditionalRequestFilter::run_request` awaits detailed
condition evaluation when should_evaluate is true, then awaits the inner request
filter. Absent conditions or a container with neither skip nor run still pass
through this wrapper. Locked async-trait 0.1.92 emits Box::pin for async trait
implementations. This is evidence of nested future construction in source, not
a measurement of optimized allocator traffic, latency or total callback cost.

The wrapper also owns alias and compiled dye metadata, delegates name and
automatic-log suppression, and binds rate-limit condition dependencies at
construction. The request runner consumes that metadata before invocation:
entry identity, anti-spoof dye stripping and log setup precede the awaited call.
Trait methods are not uniformly forwarded: requires_materialized_request_body
uses the trait default on the conditional wrapper. The current body gate is
inside it and consults its own inner plugin, so this fact alone is not a defect.

## Decision

Retain the uniform wrapper, elide only a proven metadata-free inactive wrapper
at publication, or retain metadata ownership while forwarding a no-evaluation
future directly. None is selected. Elision changes the exposed trait object;
direct forwarding can move inner-future creation before the wrapper's first
poll and change allocation/drop timing. A manual async-trait-compatible method
also adds signature and maintenance complexity. Do not assume an absent condition
means that every other wrapper responsibility is absent.

This is per-request HTTP plugin dispatch, distinct from
[stream-01](stream-01-synchronous-plugin-dispatch.md)'s connection/session trait
proposal and response-body per-chunk dispatch. It authorizes no general trait
rewrite or synchronous fast path across all plugin stages.

## Acceptance for a selected implementation

- Measure the actual mounted dispatch and full callback with plain, body-gated,
  conditioned, alias-bearing and dye-bearing entries. Compare absent conditions
  with a valid container having neither skip nor run. Record first/repeated-call
  allocations, latency, future sizes and publication storage, not just source
  Box::pin counts. Retaining the current design remains a valid outcome.
- Preserve every trait method as observed by real callers, including default
  methods, alias/name identity, dye and automatic-log suppression. Keep immutable
  generation ownership and captured old requests during reload. Freeze an exact
  constructor/runner/test allowlist before changing representation.
- Preserve dependency binding, live condition evaluation, skip cause/action logs,
  all error/deny returns and request-entry ordering. Do not fold live predicates
  at publication or weaken validation of explicit empty allOf/anyOf sets.
- Preserve the outer condition versus inner body-gate order, secondary body
  conditions, capture-only versus materialized access, limits and failure policy.
  No new body cache or change to shared request storage ownership.
- Verify future construction, first poll, repeated Pending, completion and dropped
  unpolled/pending futures with an observable test implementation. Preserve identity
  and dye cleanup, cancellation, side-effect/error timing and no invocation after
  an earlier terminal result. Do not claim an allocation shift is an elimination.
- Keep policy gates, fail-closed construction, RequestId early/late phase behavior,
  logging and metrics unchanged. Add no request tracing, instrumentation or new
  dependency to compensate for an unmeasured optimization. Require independent
  IMPL plan/final review and repository checks before runtime changes.

Fresh baseline verification passed 18 conditional-wrapper and 31 body-gate tests.
No optimized dispatch prototype or benchmark was run. Original dye ownership is
recorded in the [stage/producer guard](../../skills/04-review/plugin-runtime/regression-guards/dye-stage-producer-parity.md);
the source trait and runner remain the authority for actual forwarding behavior.
