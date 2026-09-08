# B0170 — HTTP Route Conf Handler Sharing

## Scope

- Candidate analysis and promotion HEAD: `837bb26de18fc03ea97d4f9650a1756a79577c08`
- Pipeline state: terminal; changed
- Active anchor crate: `edgion-gateway`
- Owned file: `edgion-gateway/src/routes/http/conf_handler_impl.rs`
- Baseline blob: `7c0ecc79835a568fe3359f27ef924a713ea31a05`
- Exact technical implementation allow-list: owned file only

## Handoff

- Analysis: `b0164_final_review` read the complete HTTP compilation/publication path, plugins/backends/metrics, matcher/manager/request consumers and tests.
- Value: rule and metric-handle data are currently rebuilt and retained for every hostname × match unit although their inputs are route/rule invariant. Hoisting them removes duplicated retained metric matrices and config-build allocation work.
- Required isolation: compute attached Gateways once per route and share one `Arc<HTTPRouteRule>` plus one `Arc<RouteMetricHandles>` per valid rule, but retain a distinct BackendSelector per `(rule, hostname)` so WRR state is not merged.
- Preserved behavior: invalid/resolved rule indexing, regex compilation, backend/plugin generation, route order/publication, metric labels and request semantics remain unchanged.
- Integration coverage: focused pointer-identity and backend-dimension tests are sufficient; no integration gap or edit.
- Implementation: attached Gateway identities are now computed once per route, while each valid rule builds one shared schema Arc and metric-handle matrix before hostname/match expansion. BackendSelector construction stays inside the `(rule, hostname)` path and match siblings alone share that selector.
- Tests: the conf-handler suite passed 21/21. A real two-host, two-rule, two-match regression proves same-rule schema/metric sharing across hosts and matches, distinct rule metric matrices and backend slot dimensions, same-host selector sharing, and cross-host/cross-rule selector isolation without relying on map iteration order.
- Review: fresh independent reviewer `b0170_final_review` re-read the target and HTTP metrics/LB path, verified all pointer and isolation boundaries, and returned LGTM with no Must Fix, Should Fix or deferred finding.
- Verification: `cargo fmt --all`; focused tests 21/21; locked workspace all-target check and Clippy; locked Gateway binary build; agent-doc, SSA, metrics, Pingora source, Gateway no-tracing, English-only, diff and 3,113-file frozen-path guards passed. Clippy retained only existing warnings.
- Commit: `bf9e618e`; never pushed by the campaign.
