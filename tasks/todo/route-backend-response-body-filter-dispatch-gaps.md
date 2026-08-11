# Dispatch response-body filters for GRPCRoute and selected backends

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-08-11 |
| Status | pending |
| Profile | single-file |
| Priority | P2 |
| Source | Adjacent finding from `response-body-filter-no-work-gate` investigation |

## AI guide summary

Fix the response-body stage so it dispatches every runtime scope that can own
an `UpstreamResponseBodyFilter`. The current hook runs only the HTTPRoute rule,
Gateway, and GatewayClass-global scopes. It silently skips the GRPCRoute rule
and both selected BackendRef scopes even though their `ExtensionRefFilter`
entries can resolve to an EdgionPlugins bundle containing `BandwidthLimit`.

## Evidence

`pg_upstream_response_body_filter.rs` checks only `ctx.route_unit` before the
Gateway and global loops. It never reads:

- `ctx.grpc_route_unit`;
- `ctx.selected_backend`;
- `ctx.selected_grpc_backend`.

`PluginRuntime::from_httproute_filters_for_backend`,
`from_grpcroute_filters_for_rule`, and
`from_grpcroute_filters_for_backend` all add `ExtensionRefFilter` to
`upstream_response_body_plugins`. `ExtensionRefFilter` implements
`UpstreamResponseBodyFilter` and forwards to the referenced runtime's body
filters. The neighboring synchronous and asynchronous response-header stages
already dispatch GRPCRoute and selected-backend runtimes, which confirms the
body-stage asymmetry.

## Impact

A `BandwidthLimit` reached through a GRPCRoute rule ExtensionRef or an HTTP or
gRPC BackendRef ExtensionRef can be accepted and run at other stages while its
response-body callback is never invoked. The configured response bandwidth
limit therefore has no effect for that scope.

## Required fix

- Dispatch the matched rule runtime as GRPCRoute-else-HTTPRoute.
- Dispatch the selected backend runtime as gRPC-backend-else-HTTP-backend.
- Preserve response-onion ordering relative to Gateway and global runtimes.
- Combine delay results with the existing maximum-delay rule.
- Add focused coverage for all four route/backend variants and an integration
  case proving a referenced `BandwidthLimit` affects a GRPCRoute or BackendRef
  response.

## Out of scope

- The async response-body no-work predicate.
- Refactoring response-stage dispatch beyond the minimum shared helper needed
  to prevent another scope mismatch.
- Changes to BandwidthLimit semantics.
