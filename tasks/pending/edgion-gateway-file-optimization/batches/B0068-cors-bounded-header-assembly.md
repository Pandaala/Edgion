# B0068 — CORS Bounded Header Assembly

## Scope
- Baseline: `d697aba1`
- Pipeline state: independently reviewed and fully verified; pending local commit
- Active anchor crate: `edgion-gateway`
- Owned files: `edgion-gateway/src/plugins/http/cors/{mod.rs,plugin.rs}`
- Exact implementation file: `edgion-gateway/src/plugins/http/cors/plugin.rs`
- Expected no-change file: `edgion-gateway/src/plugins/http/cors/mod.rs`
- Allowed tests: colocated tests in the implementation file only
- Forbidden files: every existing `edgion-tests/**` path, `CorsConfig` and all resource/Controller code, the Gateway API `HTTPCORSFilter`, plugin runtime/framework code, metrics/access-log code, docs, Cargo manifests, other plugins, and protected user paths
- Pre-existing protected paths: `tasks/pending/request-body-capability-next/`, `tasks/waf-todo/`, `waf-architecture-and-multi-pattern-matching.md`, and `tasks/handoff/`
- Exact future commit allow-list: `edgion-gateway/src/plugins/http/cors/plugin.rs`; campaign ledger/changelog/main record; B0067 and B0068 batch records; `deferred/D0040-cors-validation-header-precomputation.md`; `tasks/todo/unify-cors-validation-and-header-precomputation.md`; and `deferred/INDEX.md`

## Context and value gate
- Analysis subagent: `b0056_implementation`, revalidated against `d697aba1`; both target blobs and relevant construction/response call chains remain unchanged from the queued report.
- Baseline: 11 Gateway CORS tests, 30 resource CORS tests, 4 preflight-handler tests, and 34 PluginRuntime tests pass.
- `Cors` allocates a dynamic String for the static plugin name at every construction.
- Every allowed actual or preflight response allocates the container for `Vec<(&'static str, String)>`, although the builder has a fixed maximum of eight header pairs.
- Decision: return the static plugin name directly and use the existing `smallvec` dependency with inline capacity eight. Preserve all individual header values and their exact order.
- Expected no-change file: `mod.rs` remains the minimal facade.

## Hard-rule boundary
- Preserve `Cors::new` validation/init and its config-path-only tracing warning.
- Preserve wildcard/credentials and regex validation, deny-safe construction, preflight-only headers, route/plugin single ownership, `Vary` append behavior, status/body ownership, PluginLog entries, and hot-reload Arc semantics.
- Do not change the separate Gateway API CORS filter contract.
- Add no tracing, metrics, access-log fields, response headers, public APIs, dependencies, or integration-test edits.

## Implementation
- Complexity: simple; one private representation substitution and one static identity removal with no control-flow or public-contract change
- Implementer: `b0055_runtime_analysis`, distinct from the analyst and final reviewer
- Exact change: removed the per-instance name String, returned the static plugin identity directly, and used `SmallVec<[(&'static str, String); 8]>` for the proven eight-pair maximum without changing values, order, or callers.
- Test contract: static-name assertion across separate instances; maximum eight-header fixture asserting exact length, inline storage, and existing name/order/value sequence

## Independent review
- Code reviewer: `b0063_fresh_code_review`, distinct from the analyst and implementer
- Code verdict: approved with no findings after checking the complete eight-pair bound, static identity, ordering, values, lifetimes, hot reload, logging, validation, and CORS ownership/security precedents.
- Deferred adjudication: D0040 records the distinct resource-layer validation and precomputation boundary and links the nonduplicate Kubernetes CEL work. B0068 itself needs no new integration issue because inline storage is a unit-level representation invariant and frozen CORS integration coverage already exercises wire behavior.

## Verification
| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths before delegation | passed | user-authorized concurrent baseline unchanged |
| Baseline focused tests | passed | 11 Gateway CORS, 30 resource CORS, 4 preflight-handler, and 34 PluginRuntime tests |
| Implementation and focused tests | passed | 13 Gateway CORS, 30 resource CORS, 4 preflight-handler, and 34 PluginRuntime tests; maximum-inline test passed ten repeated runs |
| Gateway all-target compile | passed | implementation and independent review verification |
| Independent code review | passed | fresh reviewer distinct from analyst and implementer; no findings |
| Full campaign gates | passed | format, workspace check, Clippy, agent docs, SSA, metrics, Pingora source, and gateway no-tracing |
| Final frozen integration paths | passed | manifest unchanged; no existing integration test edited |

## Handoff
- Ledger/changelog updated: yes; one changed and one examined-no-change
- Commit: `c3c05d16`
