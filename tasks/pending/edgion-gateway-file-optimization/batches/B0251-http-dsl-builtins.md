# B0251 — HTTP DSL Builtins

## Scope

- Pipeline state: analysis-ready; complex change candidate awaiting serial promotion and current-HEAD refresh.
- Active anchor: `edgion-gateway`.
- Baseline HEAD: `f326f2800c03c34fce39f9fbedf73f18cacc2dc1`.
- Target: `edgion-gateway/src/plugins/http/dsl/lang/builtins.rs`.
- Target blob: `0e8c4dd00f2e48b20d51325c7d578e3bc8c58806`.
- Read scope may expand across the complete DSL compiler/VM/runtime, request context, outbound calls, shared mechanisms and tests.
- Frozen write scope is not yet authorized; analysis is read-only.
- Existing `edgion-tests/**`, `tasks/handoff/`, `tasks/waf-todo/`, and `waf-architecture-and-multi-pattern-matching.md` remain protected.

## Analysis Outcome

- Decision: change; complex/plan-first.
- DSL `http.call` currently stops at `httpMaxResponseBodyBytes`/`maxStringLen` and returns the partial prefix with the upstream status. This violates the root invariant that a bounded plugin-initiated outbound response must fail on overflow and never silently truncate. Upstream header names/values also enter DSL strings without `maxStringLen` enforcement.
- Reuse canonical `stream_body_capped`, compute the effective minimum body cap, preflight declared length, require EOF for exact-cap success, and return the existing non-throwing `{status: 0, body: "", headers: {}}` failure map for body/header overflow, stream failure or lossy-UTF8 expansion beyond the cap. Keep the accepted header-count cap and all outbound/SSRF/failure-shape ownership.
- Exact proposed scope: target `lang/builtins.rs`, focused `tests/http_call_tests.rs`, and the specifically listed DSL architecture/coding/review/user docs whose historical truncation conclusions become stale. Existing integrations stay frozen.
- The analyst separately identified Redis error-string bounds/diagnostics, Redis alias/request-metadata allocation work, and a frozen DSL HMAC-auth example. These require independent-review confirmation before coordinator tasks are created; config range validation remains outside this batch.
- Baseline remains `f326f280`; target blob `0e8c4dd00f2e48b20d51325c7d578e3bc8c58806`.
