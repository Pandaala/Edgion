# B0037 — Backend EndpointSlice Discovery

## Scope

- Baseline: `1b0e8c11c5981b281745d84f014ca18bf9fb68bc`
- Files: `edgion-gateway/src/backends/discovery/endpoint_slice/conf_handler_impl.rs`, `edgion-gateway/src/backends/discovery/endpoint_slice/discovery_impl.rs`, `edgion-gateway/src/backends/discovery/endpoint_slice/ep_slice_store.rs`, `edgion-gateway/src/backends/discovery/endpoint_slice/mod.rs`
- Allowed support files: colocated unit tests within the four targets only
- Forbidden files: every existing `edgion-tests/**` path, unsupported Gateway rustls code, unrelated modules, and protected user paths
- Pre-existing protected paths: `tasks/pending/request-body-capability-next/`, `tasks/waf-todo/`, `waf-architecture-and-multi-pattern-matching.md`
- Exact commit allow-list: the four target files; `03-subtasks.md`; `07-changelog.md`; `edgion-gateway-file-optimization.md`; `deferred/INDEX.md`; any analyst-verified new deferred record; the B0036 and B0037 batch records

## Context and value gate

- Analysis subagent: `b0035_analysis`
- Coverage summary: all four targets, EndpointSlice handler registration and event compression, selector publication and runtime cleanup, request-time lazy hydration, health/port/direct-endpoint consumers, 14 colocated tests, frozen integration inventory, and relevant backend/concurrency precedents
- Key responsibility/call chain: compressed EndpointSlice events update a coupled slice-to-Service and Service-to-slices store, then refresh selector cells and clean removed runtime state; request-time empty cells can lazily hydrate from the same store
- Concrete opportunity and decisive evidence: same-key `add` overwrites the forward entry without removing the old Service reverse-index membership, while update-only Service-label migration refreshes the old Service to empty but cleanup is gated on explicit removes; the two defects can preserve stale or misdirected backend state
- Expected benefit: one coherent ownership-transition contract would prevent a recreated or relabeled EndpointSlice from remaining visible through its previous Service and would retire stale selector/runtime state
- Regression surface: spans compressed-event semantics, store upsert behavior, affected-Service reporting, selector publication, runtime draining/removal, and focused delete-to-recreate and label-migration tests
- Decision: mixed — defer the ownership lifecycle, remove the independent request-path tracing, and retain the minimal module façade

## Implementation

- Implementation subagent: `b0035_defer_review`
- Complexity: simple local tracing removal; complex lifecycle work deferred
- Plan summary: remove only the three production tracing blocks from `EndpointSliceExt::build_backends`; do not alter parsing, readiness, deduplication, return values, errors, tests, or lifecycle code
- Actual change: removed per-endpoint DEBUG, empty-result WARN, and final-result DEBUG events from the request-reachable backend builder
- Behavior intentionally preserved: EndpointSlice event, store, selector, lazy-hydration, backend parsing, readiness, deduplication, return, and failure behavior remain unchanged; D0002 receives no partial implementation
- Tests added or updated: none

## Independent review

- Reviewer: `b0034_defer_review`
- Verdict: LGTM
- Must Fix resolution: none
- Deferred observations: D0002 is evidence-backed and correctly scoped; the three removed tracing blocks were the complete request-reachable tracing surface in `build_backends`
- Adjudication: `b0035_defer_review` resolved the analyst/reviewer disagreement in favor of splitting the batch: the tracing removal is local and mandatory, while D0002 remains deferred

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths before delegation | passed | 2,681 existing paths matched the manifest |
| Focused EndpointSlice tests | passed | 15 passed, 0 failed |
| Request-path tracing search | passed | no production `tracing::*` remains in `discovery_impl.rs` |
| Format | passed | `cargo fmt --all`; `cargo fmt --all -- --check` |
| Workspace compile | passed | `cargo check --workspace --all-targets` |
| Workspace lint | passed | `cargo clippy --workspace --all-targets`; existing warnings only |
| Repository guards | partial | SSA, metrics inventory, Pingora source, and Gateway no-tracing passed; baseline diagnosis and one retry confirmed the exact 13 pre-existing missing external Pingora links |
| Frozen integration paths after review and verification | passed | 2,681 existing paths matched the manifest |

## Handoff

- Ledger/changelog updated: yes
- Deferred finding: [`D0002`](../deferred/D0002-endpoint-slice-ownership-transitions.md)
- Commit: `353268ec`
