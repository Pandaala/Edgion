# B0220 — Runtime Route Matching

## Scope

- Pipeline state: analysis-ready; queued, not the active mutation batch.
- Active anchor crate: `edgion-gateway`.
- Target: `edgion-gateway/src/runtime/matching/route.rs`.
- Baseline: `1ce924ef`.
- Analyst: `b0220_route_analysis`.
- Audit the complete matching module, real callers, attachment/identity ownership and relevant precedence against current code. B0217's supporting test adaptation did not provide independent file coverage.
- No implementation authorized. Integration and user-owned handoff/WAF paths remain protected; new integration needs are deferred.

## Analysis Outcome

- Decision: change; target blob `c62f6dc217f7690bf454f95582eac999e725b529` at `1ce924ef`. Analyst matcher tests 15/15 passed. Because the edit touches a request-path matching boundary, use the workflow's plan-first gate despite the small implementation.
- Named-listener matching hashes the same Gateway key through `index.get` and then `index.get_gateway`. Resolve one GatewayListeners borrow per parentRef and reuse its named lookup/specificity methods; do not cache across generations. HTTP/gRPC surviving route candidates consume this path.
- Exact proposed technical paths: target; `skills/04-review/routing/intentional-design/route-predicate-prefilter-not-justified.md`; only row 7 of `skills/04-review/routing/intentional-design/route-architecture-consistency.md`.
- Correct the prefilter page's obsolete per-candidate formatting claim: parentRef keys are already precomputed. Preserve the measured-fan-out threshold and prohibition on mutable secondary result caches; do not claim unmeasured speedups.
- Correct row 7's inaccurate current wildcard case-folding/build-time normalization claim. Current suffix matching is case-sensitive and PGIS preserves hostname bytes; canonical configured hostname grammar is lowercase and normal HTTP/SNI input paths lowercase. No supported-input runtime defect was established, so do not alter wildcard behavior or expand to broader documentation cleanup.
- Preserve first accepted parentRef, named misses continuing only to later parents, explicit missing sections without sibling fallback, specificity before AllowedRoutes, namespace/kind/selector semantics and returned Arc identity. Add focused missing-parent/listener, specificity and sharing assertions without reducing existing tests.
- At promotion refresh against HEAD and use separate implementation, plan review and final review roles. Verify matcher, full runtime matching, PGIS and HTTP/gRPC matching filters. No integration additions or new deferred finding.
