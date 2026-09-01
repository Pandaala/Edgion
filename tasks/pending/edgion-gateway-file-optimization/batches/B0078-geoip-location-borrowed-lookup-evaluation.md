# B0078 — GeoIP Location Borrowed Lookup Evaluation

## Scope
- Baseline: `5ad0d07f`
- Pipeline state: independently approved and fully verified; pending local commit
- Active anchor crate: `edgion-gateway`
- Owned files: `edgion-gateway/src/plugins/http/geoip_location/{mod.rs,plugin.rs}`
- Exact implementation file: `edgion-gateway/src/plugins/http/geoip_location/plugin.rs`
- Expected no-change file: `edgion-gateway/src/plugins/http/geoip_location/mod.rs`
- Allowed tests: colocated tests in the implementation file only
- Forbidden files: every existing `edgion-tests/**` path, GeoIP provider/updater and stream consumer, resource schemas, access-log and metrics implementation, shared runtime APIs, and protected user paths
- Pre-existing protected paths: `tasks/pending/request-body-capability-next/`, `tasks/waf-todo/`, `waf-architecture-and-multi-pattern-matching.md`, and `tasks/handoff/`
- Exact future commit allow-list: `edgion-gateway/src/plugins/http/geoip_location/plugin.rs`; `tasks/pending/edgion-gateway-file-optimization/03-subtasks.md`; `tasks/pending/edgion-gateway-file-optimization/07-changelog.md`; `tasks/pending/edgion-gateway-file-optimization/edgion-gateway-file-optimization.md`; `tasks/pending/edgion-gateway-file-optimization/batches/B0077-forward-auth-bounded-response-ownership.md`; and `tasks/pending/edgion-gateway-file-optimization/batches/B0078-geoip-location-borrowed-lookup-evaluation.md`

## Context and value gate
- Analysis subagent: `b0078_geoip_analysis`, revalidated against `5ad0d07f5e1be3accc71309aa9bd96db8b24de0e`; target blobs are `ae6a05c746ae2e8f04a09dad1cb7084c2956adfe` for `mod.rs` and `b4d36f1e70fe62e19be009c50a2bf6cab9e4b168` for `plugin.rs`.
- Baseline focused tests: 5 Gateway and 14 resource GeoIP tests passed.
- Each plugin generation allocated a constant name, and each consulted request-scoped `GeoLookup` was cloned before matching. Found lookups hold `Arc<GeoInfo>`, so the clone added avoidable atomic reference-count traffic to the request path.
- The fixed denial response rebuilt and serialized a JSON value for every rejected request.
- Initialization and database-unavailable outcomes used ordinary business log entries even though they are abnormal dependency/configuration failures.
- Decision: borrow the lazy lookup slots only through rule evaluation, let only a copyable decision leave that scope, use static plugin and response constants, and classify abnormal outcomes with fixed `push_err` entries.
- Expected no-change file: `mod.rs` remains the accurate minimal facade.

## Hard-rule boundary
- Parse-only mode must still force both direct-peer and remote lazy GeoIP slots so the canonical access log owns request-level GeoIP observability.
- CIDR-only rule sides must not touch the GeoIP database, and RemoteIp parsing remains conditional on configured rule needs.
- Preserve rule ordering, `defaultAction`, direct/remote identity separation, fail-open/fail-close behavior, 403 status, Content-Type, and exact response bytes.
- Initialization and database errors use fixed non-sensitive error classifications. Parse, allow, and deny remain ordinary bounded business diagnostics.
- Add no tracing, metrics, access-log fields, cache, schema, failure-policy shape, dependencies, integration-test edits, or shared mechanism.

## Deferred candidates
- D0025 already owns the provider-level collapse of decode errors and caught panics into `NotFound`, plus the required HTTP/stream/access-log/metrics design. This batch does not partially change that outcome model.
- Existing frozen integration coverage already exercises parse-only dual-slot access-log population, database fail-open/fail-close, CIDR-only behavior, and GeoIP decisions. No new integration-test issue is required.

## Implementation
- Complexity: local; no plan-first review required
- Implementer: `b0055_runtime_analysis`, distinct from the analyst and final reviewer
- Exact change: removed the instance name, borrowed direct and remote lookup slots inside a limited decision scope, retained lazy source selection, reused one static deny-body literal, and converted initialization/database failure entries to fixed `push_err` classifications
- Test contract: static plugin identity; exact parse, allow, deny, initialization-failure, database fail-open, and database fail-close log shapes; unchanged response status and body

## Independent review
- Code reviewer: `b0078_final_review`, distinct from the analyst and implementer
- Code verdict: approved with no correction; confirmed lazy-slot and CIDR-only behavior, borrow lifetime and owned decision boundary, exact response bytes, diagnostic classification, and D0025 isolation
- Deferred adjudication: existing D0025 only; no new deferred or integration issue

## Verification
| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths before delegation | passed | protected external paths unchanged |
| Baseline focused tests | passed | 5 Gateway and 14 resource GeoIP tests |
| Implementation and focused tests | passed | 6 Gateway, 14 resource GeoIP, and 34 PluginRuntime tests |
| Gateway all-target compile | passed | implementation run passed |
| Independent code review | passed | reviewer reran focused tests, Gateway Clippy, no-tracing, diff, and frozen checks |
| Full campaign gates | passed | format, workspace check, Clippy, agent docs, SSA, metrics, Pingora source, and no-tracing |
| Final frozen integration paths | passed | immutable integration-test baseline unchanged |

## Handoff
- Ledger/changelog updated: yes; 1 changed and 1 examined-no-change row
- Commit: pending
