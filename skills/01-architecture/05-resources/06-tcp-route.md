---
name: resource-tcp-route
description: TCPRoute resource — L4 TCP routing, per-listener route table, first-match semantics.
---

# TCPRoute resource

> **Common flow**: see [00-resource-flow.md](00-resource-flow.md)

TCPRoute defines L4 TCP-layer routing rules, forwarding TCP connections to backend Services. TCPRoute operates at the port level: each port maintains its own route table, with first-match semantics.

## Source code locations

- Controller Handler: `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/tcp_route.rs`
- Gateway ConfHandler: `src/core/gateway/routes/tcp/conf_handler_impl.rs`
- Route manager: `src/core/gateway/routes/tcp/routes_mgr.rs`
- Type definitions: `src/types/resources/tcp_route.rs`

## Controller-side processing

### validate

1. Calls `validate_tcp_route_if_enabled()` (currently a no-op; cross-namespace validation is handled via `ref_denied` in the parse stage)
2. Calls `validate_backend_refs()` to validate all backendRefs

### parse

1. Record cross-namespace references in CrossNsRefManager
2. Register Service references in ServiceRefManager
3. Mark rejected cross-namespace backendRefs (set `ref_denied`)
4. Clear and recompute `resolved_listeners`: resolve `Vec<ListenerKey>` from parentRef via `resolve_listener_keys()` (always look up the Gateway to obtain address and protocol; when sectionName+port are both present, validate consistency), subject to the allowedRoutes namespace policy
5. If resolved_listeners is None, log a debug message (the Gateway may not have arrived yet)

### on_change

Register into GatewayRouteIndex (Gateway changes trigger requeue), update AttachedRouteTracker, and requeue the parent Gateway when needed.

### on_delete

Clear registrations in CrossNsRefManager, ServiceRefManager, GatewayRouteIndex, and AttachedRouteTracker.

### update_status

Produce a RouteParentStatus for each parentRef and set the Accepted and ResolvedRefs Conditions. TCPRoute does not pass route_hostnames (TCP has no domain concept).

## Gateway-side processing

TCPRoute's ConfHandler compiles routes into `GlobalTcpRouteManagers`:

1. **Route initialization**: call `initialize_route()` to validate and prepare the route data
2. **Per-listener management**: based on resolved_listeners, register into the route manager of the corresponding `ListenerKey`
3. **Full / incremental updates**: full_set clears and rebuilds all route caches and listener managers; partial_update collects affected listeners (old + new) and rebuilds only those listeners' route tables
4. **First-match semantics**: when multiple TCPRoutes are on the same port, match the first one in registration order

## Cross-resource relationships

| Direction | Target resource | Mechanism | Notes |
|---------|---------|---------|------|
| TCPRoute → Gateway | Gateway | parentRef | Route attaches to the Gateway's TCP-protocol Listener |
| TCPRoute → Service | Service | backendRef | Backend target for TCP-connection forwarding |
| TCPRoute ← ReferenceGrant | ReferenceGrant | cross-namespace backendRef | Authorizes cross-namespace backend references |
