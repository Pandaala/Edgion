---
name: conditional-request-wrapper-direct-forward
description: Use when reviewing ConditionalRequestFilter dispatch, request-stage plugin wrapping, or per-request future allocation in the HTTP plugin runtime.
knowledge_type: regression-guards
---

# Conditional request wrapper forwards unconditioned entries directly

## Conclusion and scope

Every HTTP request-stage plugin entry built by `plugin_runtime.rs` is wrapped in
`ConditionalRequestFilter` (`edgion-gateway/src/plugins/runtime/conditional_filter.rs`),
which owns the entry's alias, dye plan, and rate-limit condition binding. Its
`run_request` is implemented by hand with the exact `#[async_trait]` signature:

- When `conditions` is `None` or `!should_evaluate()` (neither `skip` nor `run`),
  it returns `self.inner.run_request(session, log)` directly, so an unconditioned
  entry costs only the inner filter's own boxed future.
- Otherwise it returns one `Box::pin(async move { ... })` that evaluates the
  conditions, records `set_cond_skip` on Skip, propagates evaluation errors, and
  then awaits the inner filter.

The wrapper object itself must stay: the runner reads its `name`, `alias`,
`dye_plan`, and `suppress_automatic_log`. Response-stage wrappers are outside
this rule.

## Rationale and evidence

Previously the method was an `async fn` under `#[async_trait]`, so every call
allocated a forwarding future even when there was nothing to evaluate. This ran
once per plugin per request on every route with request plugins, which can be a
noticeable fraction (not benchmarked) of cheap plugins (header modifiers, IP restriction, simple
auth checks).

On the direct path the inner `run_request` is called synchronously. Every inner
`RequestFilter` impl (plugins, body gate, Wasm/ExtProc/guardrail adapters) is an
`#[async_trait]` method that only constructs a lazy boxed future, so no inner code
runs before the first poll and side-effect timing is unchanged.

## Alternatives and limits

Precomputing a `needs_eval` field was not adopted: `should_evaluate()` is two
`Option` discriminant reads and `bind_rate_limit_dependencies` never changes
`skip`/`run`. Eliding the wrapper for unconditioned entries was rejected because
the runner reads wrapper-owned metadata. The wrapper still does not forward
`requires_materialized_request_body`; that predates this change and is unchanged.

## Verification and re-evaluation

Verified on 2026-11-06: `cargo test -p edgion-gateway --lib conditional_filter`
(19 passed, including `test_conditional_filter_empty_conditions_forward_lazily_to_inner`,
which asserts the inner filter has not run before the first poll and runs exactly
once, with no condition inputs read) and `cargo test -p edgion-gateway --lib body_gate`
(35 passed).

Re-evaluate if `RequestFilter` stops using `#[async_trait]` (for example native
async-fn-in-trait with `dyn` support), if the wrapper gains per-entry work that
must run for unconditioned entries, if the trait signature changes, or if any inner
`RequestFilter` gets a hand-written `run_request` that does eager work before
returning its future.

## Resolution references

- `request-01` — "Forward unconditioned request plugins without an extra boxed
  future" (P3 performance task), fixed on 2026-11-06.
