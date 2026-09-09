---
name: resource-tls-route
description: TLSRoute resource — TLS SNI routing, passthrough/terminate modes, per-listener route table.
---

# TLSRoute resource

> **Common flow**: see [00-resource-flow.md](00-resource-flow.md)

TLSRoute defines TLS-layer routing rules, supporting both TLS passthrough and terminate modes. Routing matches by SNI (Server Name Indication); each port maintains its own route table.

## Source code locations

- Controller Handler: `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/tls_route.rs`
- Gateway ConfHandler: `edgion-gateway/src/routes/tls/conf_handler_impl.rs`
- Route manager: `edgion-gateway/src/routes/tls/routes_mgr.rs` (GlobalTlsRouteManagers)
- Type definitions: `edgion-resources/src/resources/tls_route.rs`

## Controller-side processing

### validate

1. Calls `validate_tls_route_if_enabled()` (currently a no-op; cross-namespace validation is handled via `ref_denied` in the parse stage)
2. Calls `validate_backend_refs()` to validate all backendRefs

### parse

1. Record cross-namespace references in CrossNsRefManager
2. Register Service references in ServiceRefManager
3. Mark rejected cross-namespace backendRefs
4. Clear and recompute `resolved_listeners`: resolve `Vec<ListenerKey>` from parentRef via `resolve_listener_keys()`, following the Gateway API spec:
   - Always look up the Gateway to obtain address and protocol (the parentRef.port-only fast path no longer exists)
   - When sectionName + port are both present, validate consistency between them
   - If neither is specified → attach to all of the parent Gateway's Listeners (subject to the allowedRoutes namespace policy)
5. If resolved_listeners is None, log a debug message

### on_change

Register into GatewayRouteIndex, update AttachedRouteTracker, and requeue the parent Gateway when needed.

### on_delete

Clear registrations in CrossNsRefManager, ServiceRefManager, GatewayRouteIndex, and AttachedRouteTracker.

### update_status

Produce a RouteParentStatus for each parentRef. TLSRoute passes route_hostnames for hostname-intersection checks (required for SNI matching).

## Gateway-side processing

TLSRoute's ConfHandler compiles routes into `GlobalTlsRouteManagers`:

1. **Route initialization**: call `initialize_route()` to share the immutable incoming `Arc<TLSRoute>` with a compiled `TlsRouteUnit`; runtime selectors and resolved annotations live on the unit, not on a cloned or mutated schema
2. **Per-listener management**: based on resolved_listeners, register into the route manager of the corresponding `ListenerKey`
3. **SNI matching**: match TLS SNI against spec.hostnames, supporting wildcard domains
4. **Incremental updates**: partial_update collects affected listeners (old + new) and rebuilds only those listeners' route tables

## Cross-resource relationships

| Direction | Target resource | Mechanism | Notes |
|---------|---------|---------|------|
| TLSRoute → Gateway | Gateway | parentRef | Route attaches to the Gateway's TLS-protocol Listener |
| TLSRoute → Service | Service | backendRef | Backend target for TLS-connection forwarding |
| TLSRoute ← ReferenceGrant | ReferenceGrant | cross-namespace backendRef | Authorizes cross-namespace backend references |
