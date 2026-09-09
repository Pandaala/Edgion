# B0217 — Runtime Gateway Information

## Scope

- Pipeline state: analysis-ready; queued, not the active mutation batch.
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
