# B0159 — Stream TLS-route Facade

## Scope

- Candidate analysis HEAD: `9e51b72f8b0a2009db4bc713fb04d8e98f95ad25`; refreshed and promoted at `f3c7bb80949ed2bc5f4251185f05c5dd104267df`
- Pipeline state: terminal; changed
- Active anchor crate: `edgion-gateway`
- Owned file: `edgion-gateway/src/plugins/stream/tls_route/mod.rs`
- Baseline blob: `4f51bb1e92f6c5e7a92fe4f9953519adb41ea5a7`
- Exact implementation allow-list: owned file only

## Handoff

- Analysis: `b0133_implementation` read the complete TLS-route module, terminated and passthrough dispatchers, runtime/trait/IP plugin, store construction, schemas, tests and lifecycle precedents. The target matched its frozen baseline. Decision: change; simple documentation-only correction.
- Value: the facade incorrectly describes dispatch as universally post-handshake and its connection-context runtime as a universal pre-TLS Stage 1. Passthrough dispatch actually occurs after bounded ClientHello/SNI matching and before the end-to-end handshake is forwarded; connection-context execution is only a legacy TLSRoute fallback here.
- Implementation brief: replace only the stale module Rustdoc with the accurate terminated-versus-passthrough lifecycle and separate legacy compatibility fallback. Preserve all declarations, exports, visibility, APIs and runtime behavior.
- Deferred: equivalent stale language in independently owned trait, plugin, dispatcher, schema and older documentation remains for its own coverage; no scope expansion.
- Verification: formatting, exact diff containment and the complete campaign gate after promotion.
- Freshness: after remote `1ca0ad0e` merged, the analyst re-read its upstream BackendTLS code, pool-isolation coverage and changed knowledge. TLSRoute uses raw `BasicPeer` connections and does not enter the changed upstream peer builders; the target blob and lifecycle conclusion remain unchanged. The refreshed 3,105-file frozen manifest passed.
- Implementation: changed only the module Rustdoc to distinguish terminated post-handshake TLS/mTLS, passthrough post-ClientHello/SNI route context before forwarding the end-to-end handshake, and the separate legacy connection-context fallback. Runtime declarations and bytes are unchanged.
- External gate support: external commit `65b1da92` introduced one missing AliDNS cleanup-response task link. After explicit user authorization, separate analysis verified that describe/delete responses can still be misclassified as cleanup success. The exact one-file task record was created without changing ACME runtime code, the regression guard, the validator or any test.
- Review: `b0135_final_review` independently returned LGTM for the facade and the AliDNS task, with no Must Fix, Should Fix or deferred finding. It verified downstream/upstream TLS separation, AliDNS success-shape evidence, Controller-owned nonfatal cleanup warnings and exact scope containment.
- Verification: `cargo fmt --all`; locked workspace all-target check and Clippy; locked Gateway binary build; agent-doc, SSA, metrics, Pingora source, Gateway no-tracing, English-only, diff and 3,105-file frozen-path guards passed. Clippy retained only existing warnings.
- Commit: pending; never push.
