# B0041 — Backends Module

## Scope

- Baseline: `31d8bcdd099b323d6dccc32ec70f4c67e604a2a6`
- Files: `edgion-gateway/src/backends/mod.rs`
- Allowed support files: colocated unit tests in the target file only
- Forbidden files: every existing `edgion-tests/**` path, unsupported Gateway rustls code, unrelated modules, and protected user paths
- Pre-existing protected paths: `tasks/pending/request-body-capability-next/`, `tasks/waf-todo/`, `waf-architecture-and-multi-pattern-matching.md`
- Exact commit allow-list: the target file; `03-subtasks.md`; `07-changelog.md`; `edgion-gateway-file-optimization.md`; `deferred/INDEX.md`; any analyst-verified new deferred record; the B0040 and B0041 batch records

## Context and value gate

- Analysis subagent: `b0035_analysis`
- Coverage summary: complete 741-line target, all module declarations/re-exports/helpers, exported peer functions, HTTP/gRPC/sticky/external callers, Service/ClusterIP/ExternalName/EndpointSlice construction, DNS/SSRF/TLS/protocol/LB/error behavior, target tests, and review precedent
- Key responsibility/call chain: the root backend façade coordinates typed backend selection through Service/EndpointSlice/DNS resolution, health/outlier-aware load balancing, port/SSRF validation, TLS peer construction, upstream accounting, and established error mapping
- Concrete opportunity and decisive evidence: every ExternalName request deep-clones the complete Service annotation map before deciding whether controller-published EndpointSlices can serve the request; only borrowed lookup of DNS mode and resolver annotations is required
- Expected benefit: remove hot-path allocation and copying proportional to all Service annotations without changing routing or DNS policy
- Regression surface: limited to ExternalName mode selection and Service→Gateway→EGC→system resolver precedence; controller/cached/per-request behavior, TTL, SSRF, TLS, and errors must remain identical
- Decision: change

## Implementation

- Implementation subagent: `b0035_defer_review`
- Complexity: low
- Plan summary: borrow optional Service annotations for DNS mode and resolver selection; do not add caches, empty-map statics, or unrelated peer refactors
- Actual change: replaced the complete annotation-map clone with `as_ref()` borrowing, parsed DNS mode through the optional borrow, and made resolver selection accept the same optional borrowed map
- Behavior intentionally preserved: ExternalName controller/cached/per-request routing, resolver precedence, DNS TTL, dangling-httpdns behavior, SSRF, TLS, upstream accounting, and 500/503 mapping
- Tests added or updated: none; existing mode and resolver-precedence tests exercise the unchanged decision paths

## Independent review

- Reviewer: `b0034_defer_review`
- Verdict: LGTM
- Must Fix resolution: none
- Deferred observations: none; D0001 remains unchanged and is not reopened
- Adjudication: none

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths before delegation | passed | 2,681 existing paths matched the manifest |
| Focused backend/DNS tests | passed | 14 passed, 0 failed; one non-matching name filter was excluded from the count |
| Format | passed | `cargo fmt --all`; `cargo fmt --all -- --check` |
| Workspace compile | passed | `cargo check --workspace --all-targets` |
| Workspace lint | passed | `cargo clippy --workspace --all-targets`; existing warnings only |
| Repository guards | partial | SSA, metrics inventory, Pingora source, and Gateway no-tracing passed; baseline diagnosis and one retry confirmed the exact 13 pre-existing missing external Pingora links |
| Frozen integration paths after review and verification | passed | 2,681 existing paths matched the manifest |

## Handoff

- Ledger/changelog updated: yes
- Commit: `c3ff57b5`
