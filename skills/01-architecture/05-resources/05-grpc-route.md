---
name: resource-grpc-route
description: GRPCRoute resource — gRPC Service/Method matching, gRPC-Web, and the relationship to HTTPRoute.
---

# GRPCRoute resource

> **Common flow**: see [00-resource-flow.md](00-resource-flow.md)

GRPCRoute defines gRPC-layer routing rules. Its Controller-side processing is highly aligned with HTTPRoute (sharing route_utils helper functions); the main difference is the matching method on the Gateway side: it matches by gRPC service name and method name rather than by HTTP path.

## Source code locations

- Controller Handler: `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/grpc_route.rs`
- Gateway ConfHandler: `src/core/gateway/routes/grpc/conf_handler_impl.rs`
- Route manager: `src/core/gateway/routes/grpc/routes_mgr.rs`
- Match unit: `src/core/gateway/routes/grpc/match_unit.rs`
- Type definitions: `edgion-resources/src/resources/grpc_route.rs`

## Controller-side processing

### validate

1. Calls `validate_grpc_route_if_enabled()` (currently a no-op; cross-namespace validation is handled via `ref_denied` in the parse stage)
2. Calls `validate_backend_refs()` to validate all backendRefs

### parse

Identical pattern to HTTPRoute:
1. Record cross-namespace references in CrossNsRefManager
2. Register Service references in ServiceRefManager
3. Clear and recompute `resolved_listeners` (resolve `Vec<ListenerKey>` from parentRef via `resolve_listener_keys()`)
4. Resolve `resolved_hostnames` (intersection of route hostnames with Gateway Listener hostnames)
5. Mark rejected cross-namespace backendRefs (set `ref_denied`)

### on_change

Same as HTTPRoute: register in GatewayRouteIndex, update AttachedRouteTracker, requeue the parent Gateway when needed.

### on_delete

Same as HTTPRoute: clear all reference and index registrations.

### update_status

Same as HTTPRoute: produce a RouteParentStatus for each parentRef and set the Accepted and ResolvedRefs Conditions.

## Gateway-side processing

GRPCRoute's ConfHandler compiles routes into `GlobalGrpcRouteManagers`:

1. **preparse**: in the route cache, call `route.preparse()` to build runtime structures
2. **Route units**: each rule is compiled into a `GrpcRouteRuleUnit` containing `GrpcMatchInfo` (service, method, header matching)
3. **Domain matching**: use resolved_hostnames (preferred) or spec.hostnames as the match domain; with no hostname, use `*` (wildcard)
4. **Per-listener management**: based on resolved_listeners, register into the route manager of the corresponding `ListenerKey`; on change, only rebuild the route table for the affected listeners
5. **gRPC-Web support**: automatically supports the gRPC-Web protocol via content-type detection

## Cross-resource relationships

| Direction | Target resource | Mechanism | Notes |
|---------|---------|---------|------|
| GRPCRoute → Gateway | Gateway | parentRef | Route attaches to the Gateway's Listener |
| GRPCRoute → Service | Service | backendRef | Route's backend target service |
| GRPCRoute → EdgionPlugins | EdgionPlugins | ExtensionRef filter | Plugin configuration referenced via ExtensionRef |
| GRPCRoute ← ReferenceGrant | ReferenceGrant | cross-namespace backendRef | Authorizes cross-namespace backend references |
