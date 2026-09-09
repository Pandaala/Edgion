# B0217 — Runtime Gateway Information

## Scope

- Pipeline state: terminal changed; committed locally as `1ce924ef`.
- Active anchor crate: `edgion-gateway`.
- Target: `edgion-gateway/src/runtime/gateway_info.rs`.
- Baseline: `2f51deff`.
- Analyst: `b0211_final_review`, acting only as the B0217 analyst.
- Investigate immutable/runtime Gateway information, publication and actual
  listener/request/stream consumers against canonical configuration boundaries.
- No implementation authorized; existing integration and user-owned paths remain protected.

## Analysis Outcome

- Decision: change; complex/plan-first. Target blob `e27ab264bfde7a4825f2af067208d6211b5d0a87` at `2f51deff`; analyst freshly passed PGIS listener_gateway_info tests 9/9.
- `gateway_key()` allocates an owned String for each ordinary Service peer attempt, DirectEndpoint peer and AllEndpointStatus path. Identity is immutable per shared GatewayInfo generation; constructor-time canonical key storage and borrowed reads remove these repeated allocations, at the cost of one retained key per info object. No measured latency/RSS claim.
- Freeze identity against stale caches: private namespace/name, read-only getters preserving None versus Some(empty), canonical `format_gateway_key` constructor and Default semantics. Key can return borrowed str; cold PGIS String indexing owns a copy. Preserve current Debug content or explicitly review its change without adding logs.
- Exact proposed six source paths: target; `runtime/store/listener_gateway_info.rs`; `runtime/matching/route.rs`; `backends/mod.rs`; `routes/http/proxy_http/pg_upstream_peer.rs`; `plugins/http/all_endpoint_status/plugin.rs`, all under edgion-gateway/src. Exact knowledge: new `skills/04-review/cpu-memory/regression-guards/gateway-identity-key-precomputation.md` and cpu-memory index.
- Matching tests contain the only identity literal and direct name assertion. Ordinary peer selection may clone the existing GatewayInfo Arc to borrow its key while mutating context; never retain an extra PGIS guard or deep-copy info. Support paths do not gain independent coverage credit.
- Preserve dynamic backend TLS store lookup and policy-client-certificate priority, PGIS flat/index Arc identity, old-request generation, metrics getters, endpoint/DNS safety and config/body/client-IP ownership. Do not confuse the separate precomputed parentRef key with this allocation.
- Required plan review covers all identity callers, borrow boundaries, debug and empty/default cases. Verification includes new identity cases, PGIS sharing/generations, matching, peer-builder/DirectEndpoint/AllEndpointStatus filters. No integration changes required for the proposed ownership proof.

## Promotion

- Original analyst revalidated at `ede9c526`: all six source files unchanged, four key consumers and sole test literal unchanged; fresh PGIS tests 9/9 passed.
- Implementer: `b0217_implementation`, phase 1 plan only. Eight technical paths above are the frozen allow-list; supporting files receive no independent coverage credit.
- Existing integrations remain immutable, and new integration requirements become deferred tasks. User-owned handoff/WAF paths remain untouched.
- After the verified handoff, the user explicitly reaffirmed automatic local commits for every campaign batch, never push; no further per-batch confirmation is required.

## Implementation Plan

1. Encapsulate namespace/name and compute the canonical String key in the constructor; borrow it through the getter, preserve Default/metric getters and original eight-field Debug, and expose raw optional namespace read-only.
2. Adapt the cold PGIS index ownership and sole matching test literal/assertion, preserving flat/index Arc identity and atomic publication.
3. Borrow keys in all three request consumers; use only an existing GatewayInfo Arc clone where mutable context needs independent ownership. Keep dynamic TLS lookup, policy priority and endpoint safety unchanged.
4. Add inline identity/default/Debug/backing-byte assertions and PGIS sharing/old-new identity generation assertions; retain existing tests and integrations unchanged.
5. Add one ownership regression guard and its cpu-memory index link, documenting retained storage cost and excluding latency/whole-path allocation claims.
6. Run identity, PGIS, matching, peer-builder, backend, DirectEndpoint and AllEndpointStatus test filters, scoped formatting, compile and doc checks; then independent final review and coordinator workspace gates.

- Plan reviewer: `b0217_plan_review`; Approved at `ede9c526`, no Must Fix. Independently confirmed the mutable/async ordinary peer path needs a local existing Arc owner, other key borrows end before mutation/awaits, and dynamic TLS semantics remain uncached. Phase 2 authorized to the original implementer.
- Stop for hidden consumers outside the eight-path scope, semantic borrowing changes, overlapping external source edits or broader defects; revise the plan before any scope/strategy deviation.

## Implementation and Verification

- Implementer completed exactly the eight technical paths. Private immutable identity owns one canonical key; three request consumers borrow it, with only an existing Arc handle retained across the ordinary peer path. Cold PGIS indexing owns its key copy. Default, raw namespace, original Debug and dynamic TLS semantics are preserved.
- Fresh implementer filters passed: identity 3/3, PGIS 10/10, matching 41/41, peer builder 15/15, backends 253/253, upstream peer 92/92 and AllEndpointStatus 18/18. The planned `backends::tests::` filter selected zero tests; the broader `backends::` run covers the actual inline test module.
- Gateway library check and scoped formatting passed. Coordinator workspace format/check/Clippy, Gateway binary build, docs (1,266 files), SSA, metrics, Pingora source, Gateway tracing, English-only, diff and 3,120-file freeze passed. Clippy retains the recorded 983 Gateway lib-test warnings; no warning-free claim.
- Final reviewer: `b0217_final_review`, separate from analyst, implementer and plan reviewer; LGTM with no Must Fix, Should Fix or new deferred finding. Independently verified complete consumers, dynamic TLS and the ownership knowledge claim; fresh identity 3/3, PGIS 10/10 and peer builder 15/15 passed. The broader backend filter covers the actual selected-service tests.
- No integration or protected-path changes. No whole-workspace unit/integration run or measured latency/whole-request allocation claim. D0119's pre-existing broad context layout failures are not claimed fixed by these targeted checks.
- Verification logs: `/tmp/edgion-b0217-{check,clippy,build}.log`, implementer `edgion-b0217-impl-*.log` and independent `edgion-b0217-final-review-*.log`. Resume formatting/workspace check/Clippy and all guards passed again without source changes. Committed locally as `1ce924ef`; no push.
