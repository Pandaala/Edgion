# B0160 — Stream TLS-route Runtime

## Scope

- Candidate analysis HEAD: `9e51b72f8b0a2009db4bc713fb04d8e98f95ad25`
- Pipeline state: analysis-ready
- Active anchor crate: `edgion-gateway`
- Owned file: `edgion-gateway/src/plugins/stream/tls_route/tls_route_plugin_runtime.rs`
- Baseline blob: `5f79ee405e0938a5c40814a5d307279827c12b8e`
- Exact implementation allow-list:
  - `edgion-gateway/src/plugins/stream/tls_route/tls_route_plugin_runtime.rs`
  - `edgion-gateway/src/plugins/stream/tls_route/tls_route_plugin_trait.rs`
  - `edgion-gateway/src/plugins/stream/tls_route/ip_restriction.rs`
  - `edgion-gateway/src/routes/tls/mod.rs`
  - `edgion-gateway/src/routes/tls/proxy.rs`
  - `edgion-gateway/src/routes/tls/passthrough_proxy.rs`
  - `skills/04-review/plugin-runtime/intentional-design/plugin-runtime-god-match.md`

## Handoff

- Analysis: `b0135_final_review` read the complete TLS-route module, both terminated and passthrough lifecycles, construction/publication, Controller validation, every implementation/caller/export/test, Stage-1 sibling and precedent. The target matched its frozen baseline. Decision: change; moderate and plan-first.
- Value: every matching Stage-2 connection unconditionally clones SNI, optional TLS identity and matched resource strings, creates a large temporary log buffer, and writes an entry that no caller ever reads or transfers. The outer TLS context already owns the emitted allow/deny record. Borrowing connection metadata removes the ordinary allocations and dead buffer/write without changing observable behavior.
- Implementation brief: borrow SNI, TLS identity and matched resource; remove the dead inner log surface, unused trait `name` vtable entry and unused runtime `Clone`; preserve exceptional missing-resource fallback, terminated/passthrough context semantics, policy/order/failure behavior and request-stable Arc generations. Correct the directly applicable review precedent sentence.
- Plan-first: required before implementation because the change crosses an async object-safe trait and both TLS lifecycles.
- Integration coverage: no existing file may change; no new end-to-end gap was identified.
- Deferred: legacy Stage-1 fallback removal remains existing migration debt; no new deferred task.
- Freshness: refreshed at merge HEAD `f3c7bb80`; all seven proposed files and the target blob remained unchanged. External BackendTLS pool-isolation work is downstream of backend selection and does not intersect Stage-2 context construction. Corrected quantitative evidence: terminated mode avoids approximately five string allocations, passthrough approximately four; the discarded `LogBuffer` is large inline state, not a heap allocation. The value case, allow-list and plan-first classification remain unchanged.
- Commit: pending; never push.
