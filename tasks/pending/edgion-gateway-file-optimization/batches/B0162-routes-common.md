# B0162 — Shared Route Helpers

## Scope

- Candidate analysis HEAD: `17cd833b261161565fa9c25a8a5a3f1151f52e86`; promoted unchanged at `68670e9fa7549c96bb69571898604c293f7274df`
- Pipeline state: terminal; changed
- Active anchor crate: `edgion-gateway`
- Owned file: `edgion-gateway/src/routes/common.rs`
- Baseline blob: `e574f1a202f6697cf39efa0b07daf8ab5fbcc1de`
- Exact implementation allow-list:
  - `edgion-gateway/src/routes/common.rs`
  - `tasks/todo/delegation-parentref-gateway-name-collision.md` (historical batch artifact; resolved externally and deleted, with evidence retained in `skills/04-review/routing/regression-guards/delegation-parentref-gateway-name-collision.md`)
  - `tasks/todo/route-parent-status-canonical-identity.md`

## Handoff

- Analysis: `b0135_final_review` read the complete target, HTTP/gRPC publication, delegation flattening/status construction, all protocol consumers, schemas, Controller status ownership, tests and precedent. The target remained stable through promotion at `68670e9f`. Decision: change; simple high-value correctness fix.
- Defect: the accepted-parent filter retains HTTPRoute/GRPCRoute delegation reverse-lock refs, then listener resolution discards group/kind and uses only namespace/name. A same-coordinate Gateway can make a delegation-only child directly attach. Status matching also omits normalized group/kind, allowing Gateway and delegation entries to cross-supply `Accepted`.
- Implementation brief: recognize only normalized Gateway API group + Gateway kind before both status/no-status paths; match status with normalized group/kind plus existing namespace/name/section/port; preserve surviving order. Add deterministic unit coverage for delegation exclusion, status collision, explicit/default Gateway equivalence and mixed refs.
- Integration gap: create a deferred task for HTTP and gRPC live same-name Gateway/delegation collisions; do not edit existing integrations.
- Deferred: create a separate Controller task for canonical parent-status identity across update, retention and semantic comparison; no duplicate was found and Controller code is outside this batch.
- Implementation: added one canonical Gateway parentRef classifier with Gateway API defaults, used it before both status-backed and optimistic filtering, matched status by normalized group/kind plus the existing namespace/name/section/port identity, and reused the classifier for metric Gateway extraction. Surviving order and valid Gateway behavior remain unchanged.
- Tests: deterministic unit coverage excludes HTTPRoute/GRPCRoute reverse-lock refs with and without status, prevents a same-coordinate delegation status from supplying Gateway acceptance, equates omitted and explicit Gateway defaults, and preserves accepted Gateway order among mixed refs. Focused route-common tests passed 34/34.
- Deferred tasks: the live HTTP/gRPC collision coverage is completed and maintained in `skills/04-review/routing/regression-guards/delegation-parentref-gateway-name-collision.md`; `tasks/todo/route-parent-status-canonical-identity.md` records the complete Controller writer/storage/comparison to Gateway publication identity fix, including configured controller ownership and foreign-controller precedence.
- Review: `b0160_final_review` independently re-read the three-file diff and relevant route/status rules. After requiring the canonical-identity task to cover Gateway publication and configured controller ownership plus correcting lifecycle/defaulting docs, it returned LGTM with no remaining Must Fix or Should Fix.
- Verification: `cargo fmt --all`; focused tests 34/34; locked workspace all-target check and Clippy; locked Gateway binary build; agent-doc, SSA, metrics, Pingora source, Gateway no-tracing, English-only, diff and 3,107-file frozen-path guards passed. Clippy retained only existing warnings.
- Commit: `78cf23d9`; never pushed by the campaign.
