---
name: scoped-response-body-filter-dispatch
description: Use when reviewing response-body plugin dispatch across HTTPRoute, GRPCRoute, selected BackendRef, Gateway, and global scopes.
knowledge_type: regression-guards
---

# Scoped response-body filter dispatch

## Conclusion and scope

`run_read_only_body_plugins` must dispatch the request's matched rule runtime,
selected backend runtime, resolved Gateway runtimes, and resolved global runtimes
in that order. The rule slot selects `grpc_route_unit` before the HTTPRoute
fallback; the backend slot selects `selected_grpc_backend` before the selected
HTTP backend fallback. Every returned `Option<Duration>` contributes to one
maximum delay.

This applies to `UpstreamResponseBodyFilter` entries reached directly or through
an EdgionPlugins `ExtensionRef`, including `BandwidthLimit`. Only the matched
route type and selected backend type execute for one request.

## Rationale and evidence

Route and backend filters compile into separate `PluginRuntime` instances in
`HttpRouteRuleUnit::compile_http_rule` and
`GrpcRouteRuleUnit::compile_grpc_rule`. The selected backend handle carries the
backend runtime into `EdgionHttpContext`; response-body dispatch must therefore
read both the matched route slot and selected backend slot. Gateway and global
runtimes follow them to preserve response onion ordering.

Historically, `run_read_only_body_plugins` read only `ctx.route_unit`, so
GRPCRoute rule filters and both selected BackendRef variants were skipped. Commit
`cbd88c5d` added all four route/backend variants and common maximum-delay
merging. The BandwidthLimit integration suite now proves the production HTTP
BackendRef path: `backendRefs[].filters.ExtensionRef` resolves an EdgionPlugins
resource whose `BandwidthLimit` delays a 100 KiB response at 100 KiB/s.

## Alternatives and limits

Dispatching every populated HTTP and gRPC slot would run two mutually exclusive
chains if an invalid context contained both. The response header stages already
use gRPC-first fallback semantics, so the body stage follows the same selection.

Focused Gateway tests cover HTTP/GRPC rule selection, HTTP/gRPC backend
selection, and maximum-delay merging across route, backend, Gateway, and global
scopes. The integration scenario covers a selected HTTP BackendRef. It does not
duplicate the same wire assertion for GRPCRoute or a gRPC BackendRef.

## Verification and re-evaluation

Verified on 2026-09-20:

- Gateway unit tests
  `grpc_route_preempts_http_route_and_http_route_is_the_fallback`,
  `grpc_backend_preempts_http_backend_and_http_backend_is_the_fallback`, and
  `route_backend_gateway_and_global_delays_merge_to_the_maximum` passed.
- `run_integration.sh --no-prepare -r EdgionPlugins -i BandwidthLimit` passed,
  including configuration loading, resource convergence, three client cases,
  unknown-field checks, requeue detection, and cache residue detection.

Re-evaluate when response scope ordering changes, route/backend context slots are
unified, ExtensionRef runtime capture changes, or another response-body return
value must be combined differently from the current maximum delay.

## Resolution references

- `route-backend-response-body-filter-dispatch-gaps` — runtime defect fixed by
  `cbd88c5d`; selected BackendRef integration coverage completed on 2026-09-20.
