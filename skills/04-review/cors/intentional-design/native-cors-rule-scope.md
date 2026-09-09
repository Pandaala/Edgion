---
name: native-cors-rule-scope
description: Use when reviewing native CORS placement, backendRef preflight ownership, or CORS conformance declarations.
knowledge_type: intentional-design
---

# Native CORS belongs to the route rule

## Conclusion and scope

Edgion supports native Gateway API `type: CORS` only in `HTTPRouteRule.filters`.
Any CORS filter in `HTTPBackendRef.filters` invalidates the whole rule, including
zero-weight references, mixed rule/backend policies, and weighted backends with
different policies. No policies are merged, promoted, or silently ignored. Other
valid rules remain active. This policy covers authored and delegated effective rules.
It does not change the separate EdgionPlugins CORS extension's semantics.

The user selected this bounded support policy after reviewing the Gateway API
support level and the pinned conformance tests, and approved cross-layer enforcement.
This is an accepted product boundary, not a claim that the formerly partial behavior
was correct or that Gateway API forbids backendRef CORS.

## Rationale and evidence

D0084 identified a real defect: `pg_request_filter` detects native CORS only on
`rule.filters`, so backend-only preflights reached the generic headerless response
before `pg_upstream_peer` selected the backend whose CORS runtime could handle them.
Ordinary requests nevertheless used the backend CORS response filter.

- Shared gate: `edgion-resources/src/resources/http_route.rs`,
  `HTTPRouteRule::unsupported_backend_cors_reason` inspects the filter type regardless
  of weight or presence/validity of the `cors` payload.
- Controller: `handlers/http_route.rs::invalid_cors_reason` feeds
  `find_invalid_rule_details`, `invalid_rule_indices`, and `PartiallyInvalid`.
  An authored backend CORS violation with no surviving effective rule rejects the
  Gateway attachment with `Accepted=False/UnsupportedValue`; any stale
  `PartiallyInvalid=True` clears with an unsupported-value reason, not AllRulesValid.
  `handlers/route_delegation.rs` already uses that same rule validation for parents
  and children, so invalid child rules never enter `resolved_rules`. The parent's
  delegation diagnostic retains the child rule and backend CORS reason in
  `PartiallyInvalid`, or in `Accepted` when the whole attachment is rejected; child-side
  HTTPRoute parentRefs retain their separate delegation-relationship acceptance.
- Gateway: `routes/http/conf_handler_impl.rs::parse_http_routes_to_domain_rules`
  checks effective rules independently of status before building their runtimes.
  Surviving rules retain original IDs and share one runtime across hostnames/matches.
- A rejected rule no longer matches either preflight or actual requests. Other valid
  routes may match; without one the ordinary route-not-found response is 404.

The [Gateway API v1.5.0 definition](https://github.com/kubernetes-sigs/gateway-api/blob/v1.5.0/apis/v1/httproute_types.go)
marks CORS in both placements Extended, while the generic backend filter field has
an implementation-specific annotation. Neither implies that backendRef CORS is
invalid API syntax. The [CORS GEP](https://gateway-api.sigs.k8s.io/geps/gep-1767/)
requires supported preflights to terminate at the gateway, even for denied origins.

## Alternatives and limits

The chosen boundary avoids advancing backend selection into the preflight phase,
changing weighted selection/stickiness, or inventing a mixed-policy merge contract.
Its cost is that operators must move a common policy to rule scope or split routes;
per-backend native CORS is unavailable. Fully supporting backend CORS remains a
future product decision, not unfinished implementation of this selected policy.

Gateway API types are pinned to v1.5.0 and conformance to v1.5.1 in
`edgion-tests/conformance/go.mod`. The `HTTPRouteCORS` and
`HTTPRouteCORSAllowCredentialsBehavior` manifests contain seven rule-level CORS
rules and no backendRef CORS cases. Their feature gate remains enabled for that
coverage. No independent backend CORS feature exists in the pinned list. This
test coverage and declaration do not prove complete CORS placement conformance.
Do not remove rule-level tests or change generated historical reports to conceal
the unsupported scope.

## Verification and re-evaluation

- Added `backend_cors_scope` unit tests cover schema scope decisions, Controller
  status recovery, and Gateway enforcement without Controller status or on resolved
  rules, preserving surviving rule IDs.
- `edgion-tests/integration/scripts/integration/test_native_cors_scope.py` uses the
  existing shared topology and polls complete phases: Controller status, Gateway
  publication, preflight/actual responses, fallback, hot reload, and delegation.
- Verified on 2026-09-09: 70 focused CORS unit tests and 22 HTTP route compiler
  tests passed. The final Controller rerun passed both `backend_cors_scope` tests,
  including rejected-delegation diagnostics, surviving effective rules, and recovery.
- The standalone client passed all seven cross-process phases against freshly built
  Controller/Gateway binaries and one shared backend, including delegated rejection
  and recovery; all four temporary HTTPRoutes were removed and unpublished.
- Workspace check and Clippy, formatting, agent-doc consistency, SSA force, metrics
  inventory, Pingora source policy, and English-only guards passed. Clippy retains
  existing warnings. Full workspace tests and Kubernetes conformance were not rerun;
  historical conformance reports are not fresh verification.

Re-evaluate if backend CORS is requested, upstream changes support annotations or
feature granularity, conformance adds backend/mixed-policy coverage, or routing
changes preflight/selection ownership. Preserve local-response diagnostics, access-log
ownership, response streaming, and immutable publication in any successor design.

## Resolution references

- D0084 (B0124), originally `needs-discussion`: partial preflight activation was a
  real defect, resolved by the explicitly selected rule-only scope policy.
- Selected task: `define-backendref-native-cors-preflight-ownership.md`.
- [Existing plugin preflight ownership](../regression-guards/preflight-route-cors-single-owner.md)
  remains applicable to EdgionPlugins; it did not previously cover native backend CORS.
