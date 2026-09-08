# B0160 — Stream TLS-route Runtime

## Scope

- Candidate analysis HEAD: `9e51b72f8b0a2009db4bc713fb04d8e98f95ad25`; refreshed at `f3c7bb80949ed2bc5f4251185f05c5dd104267df`; promoted at `687f058f88ca6c49efc4171f746cf06ecc25997f`
- Pipeline state: terminal; changed
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
- Approved plan: convert `TlsRouteContext` to borrowed metadata and remove its dead log surface/name API; update runtime and IP plugin; introduce an explicit terminated/passthrough mode in the shared dispatcher with scoped borrows and pinned compiled-generation Arc; adapt callers/tests; correct the directly affected precedent; run focused and full gates.
- Plan review: fresh `b0160_plan_review` independently approved the six-step plan with no Must Fix. It verified object safety, borrow lifetimes across await, exceptional fallback lifetime, outer-log ownership, terminated/passthrough metadata, Arc generation pinning, seven-file sufficiency and rollback boundaries.
- Integration coverage: no existing file may change; no new end-to-end gap was identified.
- Deferred: legacy Stage-1 fallback removal remains existing migration debt; no new deferred task.
- Freshness: refreshed at merge HEAD `f3c7bb80`; all seven proposed files and the target blob remained unchanged. External BackendTLS pool-isolation work is downstream of backend selection and does not intersect Stage-2 context construction. Corrected quantitative evidence: terminated mode avoids approximately five string allocations, passthrough approximately four; the discarded `LogBuffer` is large inline state, not a heap allocation. The value case, allow-list and plan-first classification remain unchanged.
- Implementation: `TlsRouteContext` now borrows SNI, optional TLS identity and matched `Rsc` through async trait-object dispatch. The shared dispatcher pins the compiled-generation Arc across await, scopes immutable borrows before canonical outer log writes, explicitly distinguishes terminated and passthrough metadata, and allocates the missing-resource placeholder only on the exceptional path. Dead inner log state/write, trait name surface and runtime `Clone` were removed. Lifecycle Rustdoc was corrected in the trait and IP implementation.
- Preserved behavior: empty allow, first deny, policy Block, Bypass/disabled entries, two-IP matching, missing-resource fail-close, legacy fallback, outer allow/deny diagnostics, terminated TLS identity/mTLS, passthrough `None`/`false`, backend ordering and routing termination are unchanged.
- Review corrections: the fresh final reviewer first required correcting universal post-handshake wording in the trait and then the IP implementation. Both exact-file documentation fixes were applied and re-reviewed. Final verdict: LGTM with no Must Fix, Should Fix or deferred finding.
- External gate support: external commits advanced through `0170db38`. The frozen manifest was mechanically refreshed to 3,107 files for two added namespace-watch paths and one changed cache-residue fingerprint. Separate analysis and implementation created the real AliDNS exact-record-selection/pagination task and replaced two nonexistent Controller task-artifact links with the authoritative lifecycle guide and existing namespace-watch contract. The validator was not weakened and no integration file was edited.
- Verification: runtime/IP focused tests 9/9; TLS fail-close tests 7/7; `cargo fmt --all`; locked workspace all-target check and Clippy; locked Gateway binary build; agent-doc, SSA, metrics, Pingora source, Gateway no-tracing, English-only, diff and 3,107-file frozen-path guards passed. Clippy retained only existing warnings.
- Commit: pending; never push.
