# B0162 — Shared Route Helpers

## Scope

- Candidate analysis HEAD: `17cd833b261161565fa9c25a8a5a3f1151f52e86`
- Pipeline state: analysis-ready
- Active anchor crate: `edgion-gateway`
- Owned file: `edgion-gateway/src/routes/common.rs`
- Baseline blob: `e574f1a202f6697cf39efa0b07daf8ab5fbcc1de`
- Exact implementation allow-list:
  - `edgion-gateway/src/routes/common.rs`
  - deferred task records for live collision coverage and Controller parent-status identity, exact names to be frozen at promotion

## Handoff

- Analysis: `b0135_final_review` read the complete target, HTTP/gRPC publication, delegation flattening/status construction, all protocol consumers, schemas, Controller status ownership, tests and precedent. The target remained stable at external HEAD `d8d9d1d0`. Decision: change; simple high-value correctness fix.
- Defect: the accepted-parent filter retains HTTPRoute/GRPCRoute delegation reverse-lock refs, then listener resolution discards group/kind and uses only namespace/name. A same-coordinate Gateway can make a delegation-only child directly attach. Status matching also omits normalized group/kind, allowing Gateway and delegation entries to cross-supply `Accepted`.
- Implementation brief: recognize only normalized Gateway API group + Gateway kind before both status/no-status paths; match status with normalized group/kind plus existing namespace/name/section/port; preserve surviving order. Add deterministic unit coverage for delegation exclusion, status collision, explicit/default Gateway equivalence and mixed refs.
- Integration gap: create a deferred task for HTTP and gRPC live same-name Gateway/delegation collisions; do not edit existing integrations.
- Deferred: create a separate Controller task for canonical parent-status identity across update, retention and semantic comparison; no duplicate was found and Controller code is outside this batch.
- Commit: pending; never push.
