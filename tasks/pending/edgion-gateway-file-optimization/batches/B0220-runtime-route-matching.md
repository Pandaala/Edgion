# B0220 — Runtime Route Matching

## Scope

- Pipeline state: terminal; local commit `4db7b1eb`.
- Active anchor crate: `edgion-gateway`.
- Target: `edgion-gateway/src/runtime/matching/route.rs`.
- Baseline: `1ce924ef`.
- Analyst: `b0220_route_analysis`.
- Audit the complete matching module, real callers, attachment/identity ownership and relevant precedence against current code. B0217's supporting test adaptation did not provide independent file coverage.
- Implementation is plan-only until independent plan approval. Integration and user-owned handoff/WAF paths remain protected; new integration needs are deferred.

## Analysis Outcome

- Decision: change; target blob `c62f6dc217f7690bf454f95582eac999e725b529` at `1ce924ef`. Analyst matcher tests 15/15 passed. Because the edit touches a request-path matching boundary, use the workflow's plan-first gate despite the small implementation.
- Named-listener matching hashes the same Gateway key through `index.get` and then `index.get_gateway`. Resolve one GatewayListeners borrow per parentRef and reuse its named lookup/specificity methods; do not cache across generations. HTTP/gRPC surviving route candidates consume this path.
- Exact proposed technical paths: target; `skills/04-review/routing/intentional-design/route-predicate-prefilter-not-justified.md`; only row 7 of `skills/04-review/routing/intentional-design/route-architecture-consistency.md`.
- Correct the prefilter page's obsolete per-candidate formatting claim: parentRef keys are already precomputed. Preserve the measured-fan-out threshold and prohibition on mutable secondary result caches; do not claim unmeasured speedups.
- Correct row 7's inaccurate current wildcard case-folding/build-time normalization claim. Current suffix matching is case-sensitive and PGIS preserves hostname bytes; canonical configured hostname grammar is lowercase and normal HTTP/SNI input paths lowercase. No supported-input runtime defect was established, so do not alter wildcard behavior or expand to broader documentation cleanup.
- Preserve first accepted parentRef, named misses continuing only to later parents, explicit missing sections without sibling fallback, specificity before AllowedRoutes, namespace/kind/selector semantics and returned Arc identity. Add focused missing-parent/listener, specificity and sharing assertions without reducing existing tests.
- At promotion refresh against HEAD and use separate implementation, plan review and final review roles. Verify matcher, full runtime matching, PGIS and HTTP/gRPC matching filters. No integration additions or new deferred finding.

## Current-HEAD Promotion

- Fresh analyst `b0220_refresh_analysis` reconfirmed change at `00f896a2`; target blob and matching/PGIS/consumer/knowledge paths unchanged. Duplicate Gateway-key lookup remains demonstrable, without a throughput claim.
- Technical allow-list remains the target and two knowledge files above; architecture-consistency edits are limited to row 7. Coordinator records only: main task, ledger, changelog, B0219 SHA backfill, B0220–B0222 queue records.
- Protected worktree: untracked `tasks/handoff/`, `tasks/waf-todo/`, `waf-architecture-and-multi-pattern-matching.md`. No external tracked modifications; 547-file inventory reconciled and 3,121-file integration freeze passed.
- Original knowledge paths: route architecture `03-routes/SKILL.md`, coding index/Rust rules, testing index/unit rules, design/review-routing indexes and both scoped routing knowledge pages. Every technical role reads original rules independently.

## Implementation Plan

- Implementer: `b0220_implementation`; complex. Independent `b0220_plan_review` Approved at `00f896a2`; implementation authorized for the unchanged six-step plan.
1. Resolve one GatewayListeners borrow per parentRef; named lookup/specificity and unnamed iteration reuse it.
2. Preserve missing-parent/section continuation, first accepted parent, hostname/specificity/AllowedRoutes ordering, namespace/kind/selector behavior and Arc identity without cross-generation retention.
3. Retain 15 existing tests; add missing-parent/section continuation, explicit-section no-sibling fallback, denied-specificity precedence and named/unnamed pointer-identity assertions.
4. Correct prefilter knowledge to precomputed keys and borrowed lookup while retaining measurement prerequisites and no secondary mutable cache.
5. Correct only architecture row 7's hostname implementation claims; preserve historical finding identity and runtime behavior.
6. Independently review, then run the five matcher/PGIS/HTTP/gRPC filters and full required gates. Stop for scope/semantic deviations; recover only campaign-owned hunks.

## Implementation Result

- Implementer completed the exact three technical paths. One parent-local borrow replaces duplicate Gateway-key lookup; existing 15 tests remain with four added behavioral/Arc-identity tests.
- Both knowledge corrections remain scoped; row 7 explicitly distinguishes normalized HTTP/ClientHello input from verbatim configured `fallback_sni`. No hostname behavior, integration test or shared task-index changes.
- Mechanical containment and integration freeze passed. Fresh final reviewer `b0220_final_review` is read-only and distinct from all earlier roles; verification is running in the coordinator's serial Cargo lane.

## Independent Review and Verification

- Final reviewer LGTM; no Must Fix, Should Fix or new deferred issue. Reviewed route blob `5efe644dd4c540c87b016462b69d26a934d8e0a0`; lookup equivalence, all consumers/invariants, 15 retained tests plus four substantive additions and both knowledge conclusions independently checked.
- Fresh tests: route 19/19, full runtime matching 45/45, PGIS 10/10, HTTP matching 49/49 and gRPC matching 19/19. Logs: `/tmp/edgion-b0220-{route,matching,pgis,http,grpc}.log`.
- Workspace format/check/Clippy passed (existing lint warnings); docs (1,268 files), SSA, metrics, Pingora source, Gateway no-tracing, English-only, diff and integration freeze passed. No integration or protected user files changed.
- Ledger/changelog updated. Commit `4db7b1eb`; local only, never push.
