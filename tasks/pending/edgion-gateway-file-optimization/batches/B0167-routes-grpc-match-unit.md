# B0167 — Compiled gRPC Backend Policy Identity

## Scope

- Candidate analysis and promotion HEAD: `bc47034c9d355a3510e5fe1d15784dd4763aed4d`
- Pipeline state: terminal; changed
- Active anchor crate: `edgion-gateway`
- Owned file: `edgion-gateway/src/routes/grpc/match_unit.rs`
- Supporting call site: `edgion-gateway/src/routes/grpc/integration.rs`
- Baseline blobs: `17c2f89f2dabf8e0953edbc964a7af74e1a0b34f`, `4e2f786cdaed959f99d97fa76e2c36bc992c4553`
- Exact technical implementation allow-list:
  - `edgion-gateway/src/routes/grpc/match_unit.rs`
  - `edgion-gateway/src/routes/grpc/integration.rs`

## Handoff

- Analysis: `b0163_final_review` read the complete unit, conf-handler compilation, matcher/manager publication, integration/backend/TLS/session-persistence path, schema, selection invariants and tests.
- Value: `policy_target_key(route_namespace)` currently clones namespace and name on every first backend selection. The authorized identity depends only on immutable backendRef data and route namespace, so it can be compiled once into the route generation and borrowed by the request path.
- Required semantics: only core Service refs produce a key; explicit namespace wins, otherwise route namespace; denied, delegation and unsupported refs remain absent. Keep explicit denial precedence, BackendNotFound behavior, session-persistence/WRR ordering, TLS capture and retry generation pinning unchanged.
- Integration coverage: focused ownership/allocation tests plus existing frozen gRPC suites are sufficient; no new gap.
- Plan: `b0164_final_review` proposed compiling the authorized canonical target into each immutable backend generation and borrowing it through the handle while preserving denial/TLS/retry ordering. Independent reviewer `b0166_final_review` approved it without Must Fix and constrained the zero-allocation claim to handle/accessor projection, exact case-sensitive group/kind defaults, no borrow across await, and old-generation retention coverage.
- Implementation: `CompiledGrpcBackend` now owns an optional canonical Service policy target built once during rule compilation. `GrpcBackendHandle` borrows it without allocation; the integration path preserves explicit `RefDenied` priority, maps absent unsupported targets to the existing `BackendNotFound`, performs the same-port TLS lookup before plugin awaits, and never copies the key into request state.
- Tests: backend-handle tests passed 7/7 and the full gRPC route subset passed 38/38. Coverage includes every supported/unsupported group-kind and namespace form, denial/delegation, repeated pointer identity, allocation-free projection, and retained old-generation identity after replacement.
- Review: fresh independent final reviewer `b0167_final_review` re-read the two-file diff and policy/TLS/session/retry rules, confirmed borrow lifetime and exact behavioral equivalence, and returned LGTM with no Must Fix, Should Fix or deferred finding.
- Verification: `cargo fmt --all`; focused tests 7/7 and gRPC tests 38/38; locked workspace all-target check and Clippy; locked Gateway binary build; agent-doc, SSA, metrics, Pingora source, Gateway no-tracing, English-only, diff and 3,113-file frozen-path guards passed. Clippy retained only existing warnings.
- Commit: `11843ea1`; never pushed by the campaign.
