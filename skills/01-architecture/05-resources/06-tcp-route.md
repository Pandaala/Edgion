---
name: resource-tcp-route
description: TCPRoute resource — L4 TCP routing, per-listener route table, first-match semantics.
---

# TCPRoute resource

> **Common flow**: see [00-resource-flow.md](00-resource-flow.md)

TCPRoute defines L4 TCP-layer routing rules, forwarding TCP connections to backend Services. The Gateway maintains route tables by Controller-resolved `ListenerKey` (listening socket address and protocol), not port alone: listeners on different addresses can use the same port without sharing a table.

## Source code locations

- Controller Handler: `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/tcp_route.rs`
- Gateway ConfHandler: `edgion-gateway/src/routes/tcp/conf_handler_impl.rs`
- Route manager: `edgion-gateway/src/routes/tcp/routes_mgr.rs`
- Type definitions: `edgion-resources/src/resources/tcp_route.rs`

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

1. **Route compilation**: call `initialize_route()` to compile the shared `Arc<TCPRoute>` schema into a `TcpRouteUnit`; this initializer builds runtime state, not a route-validation gate
2. **Per-listener management**: based on resolved_listeners, register into the route manager of the corresponding `ListenerKey`
3. **Full / incremental updates**: full_set clears and rebuilds all route caches and listener managers; partial_update collects affected listeners (old + new) and rebuilds only those listeners' route tables
4. **First-match semantics**: sort compiled units by resource key (`namespace/name`) at table-build time and select the first unit. This is a deterministic fallback for an over-bound listener, not registration order or full Gateway API conflict arbitration

See [Gateway TCP route details](../02-gateway/03-routes/03-tcp-route.md) for the canonical table, compilation, and snapshot ownership description.

## Cross-resource relationships

| Direction | Target resource | Mechanism | Notes |
|---------|---------|---------|------|
| TCPRoute → Gateway | Gateway | parentRef | Route attaches to the Gateway's TCP-protocol Listener |
| TCPRoute → Service | Service | backendRef | Backend target for TCP-connection forwarding |
| TCPRoute ← ReferenceGrant | ReferenceGrant | cross-namespace backendRef | Authorizes cross-namespace backend references |
