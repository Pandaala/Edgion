---
name: resource-udp-route
description: UDPRoute resource — L4 UDP routing, per-listener route table, client-session response path.
---

# UDPRoute resource

> **Common flow**: see [00-resource-flow.md](00-resource-flow.md)

UDPRoute defines L4 UDP-layer routing rules, forwarding UDP datagrams to backend
Services. UDP itself is connectionless, but the Gateway creates a per-client
session: the first packet looks up the current route table and selects an upstream,
while later packets reuse that pinned session in both directions.

## Source code locations

- Controller Handler: `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/udp_route.rs`
- Gateway ConfHandler: `edgion-gateway/src/routes/udp/conf_handler_impl.rs`
- Route manager: `edgion-gateway/src/routes/udp/routes_mgr.rs` (GlobalUdpRouteManagers)
- Type definitions: `edgion-resources/src/resources/udp_route.rs`

## Controller-side processing

### validate

1. Calls `validate_udp_route_if_enabled()` (currently a no-op; cross-namespace validation is handled via `ref_denied` in the parse stage)
2. Calls `validate_backend_refs()` to validate all backendRefs

### parse

1. Record cross-namespace references in CrossNsRefManager
2. Register Service references in ServiceRefManager
3. Mark rejected cross-namespace backendRefs
4. Clear and recompute `resolved_listeners`: resolve `Vec<ListenerKey>` from parentRef via `resolve_listener_keys()` (logic is identical to TCPRoute / TLSRoute)
5. If resolved_listeners is None, log a debug message

### on_change

Register into GatewayRouteIndex (Gateway changes trigger requeue), update AttachedRouteTracker, and requeue the parent Gateway when needed.

### on_delete

Clear registrations in CrossNsRefManager, ServiceRefManager, GatewayRouteIndex, and AttachedRouteTracker.

### update_status

Produce a RouteParentStatus for each parentRef and set the Accepted and ResolvedRefs Conditions. UDPRoute does not pass route_hostnames (UDP has no domain concept).

## Gateway-side processing

UDPRoute's ConfHandler compiles routes into `GlobalUdpRouteManagers`:

1. **Route compilation**: call `initialize_route()` to build the companion
   `UdpRouteUnit`, including its backend selector
2. **Per-listener management**: based on resolved_listeners, register into the route manager of the corresponding `ListenerKey`
3. **Full / incremental updates**: full_set clears and rebuilds; partial_update collects affected listeners and rebuilds only those listeners
4. **Data-plane model**: a packet without an existing client session loads the
   current route table, runs route/plugin/backend selection, and creates a session.
   Established sessions bypass route lookup and keep using their pinned upstream;
   the same session maps backend responses back to the client

### Data-plane sessions

- The session key is the client address; on the first client packet a session is created and bound to a dedicated upstream UDP socket
- Session limit `MAX_UDP_SESSIONS = 10000`; once exceeded, a new client packet logs `SessionLimitReached` and is dropped
- `SESSION_TIMEOUT = 60s`, refreshed by traffic in either direction; the cleanup loop runs every 10s, removing expired sessions and writing UDP access logs
- When the main listener exits, the cleanup loop and all per-session upstream receive tasks are cancelled

Operationally, fd, task, and memory usage must be sized against the expected concurrent client count; sessions are not pure stateless forwarding — backend responses must be associated back to the client through their corresponding upstream socket.

## Cross-resource relationships

| Direction | Target resource | Mechanism | Notes |
|---------|---------|---------|------|
| UDPRoute → Gateway | Gateway | parentRef | Route attaches to the Gateway's UDP-protocol Listener |
| UDPRoute → Service | Service | backendRef | Backend target for UDP-datagram forwarding |
| UDPRoute ← ReferenceGrant | ReferenceGrant | cross-namespace backendRef | Authorizes cross-namespace backend references |
