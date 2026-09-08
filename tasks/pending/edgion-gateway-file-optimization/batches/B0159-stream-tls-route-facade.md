# B0159 — Stream TLS-route Facade

## Scope

- Candidate analysis HEAD: `9e51b72f8b0a2009db4bc713fb04d8e98f95ad25`
- Pipeline state: analysis-ready
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
- Commit: pending; never push.
