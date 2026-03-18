# Requeue Mechanism — Cross-Resource Dependency Resolution

## Overview

The Controller uses a requeue-based reconciliation pattern to handle resource
dependencies. When Resource A changes, dependent Resources B are requeued
(re-processed) to pick up the new state. This avoids tight coupling and supports
arbitrary resource arrival order during init and runtime.

## Core Components

### Workqueue

- Each `Processor<K>` has its own workqueue
- Ready queue (channel) + delay heap (priority queue by fire time)
- `enqueue(key)`: immediate processing
- `enqueue_after(key, delay, chain)`: delayed with cycle detection
- Workers spawned only after `InitDone`; items queued during init are buffered

### PROCESSOR_REGISTRY

- Static global registry of all resource processors
- `requeue(kind, key)` → immediate enqueue
- `requeue_with_chain(kind, key, delay, chain)` → delayed with cycle tracking

### TriggerChain

- Tracks requeue causality to prevent infinite loops
- `max_trigger_cycles = 5` by default
- Chain is passed through `HandlerContext.requeue()` calls

## Requeue Trigger Paths

### 1. gateway_route_index

**Purpose**: Resolves Route ↔ Gateway dependencies (hostnames, ports, sectionName)

| Direction | Trigger | Target Kinds | Condition |
|---|---|---|---|
| Gateway → Routes | Listener hostname/port changed | HTTPRoute, GRPCRoute, TLSRoute, EdgionTls | `hostnames_changed \|\| ports_changed` |
| Route → Gateway | parentRef attachment changed | Gateway | Only if attachment list actually changed |
| Post-init | `trigger_gateway_route_revalidation()` | All registered routes | One-time; covers Gateway-before-Route ordering |

**Files**:
- `gateway_route_index.rs` — forward/reverse index, change detection caches
- `handlers/gateway.rs` — on_change: update caches, requeue routes
- `handlers/{http_route,grpc_route,tls_route,edgion_tls}.rs` — on_change: register
- `handlers/mod.rs` — `update_gateway_route_index()`, `remove_from_gateway_route_index()`

**Registration rule**: Any handler that calls `lookup_gateway()` in `parse()` MUST
implement `on_change()` to call `update_gateway_route_index()` and `on_delete()` to
call `remove_from_gateway_route_index()`.

### 2. SecretRefManager

**Purpose**: Resolves Secret dependencies (TLS certificates, auth credentials)

| Direction | Trigger | Target Kinds | Mechanism |
|---|---|---|---|
| Secret → dependents | Secret created/updated/deleted | Gateway, EdgionTls, EdgionPlugins, EdgionAcme | `trigger_cascading_requeue()` |
| Post-init | `trigger_gateway_secret_revalidation()` | All Gateways | One-time; covers Secret-after-Gateway ordering |

**Files**:
- `ref_manager.rs` — generic key→value ref manager
- `handlers/secret.rs` — on_change/on_delete: cascading requeue
- `secret_utils/secret_ref.rs` — SecretRefManager alias

### 3. ServiceRefManager

**Purpose**: Resolves Service backend dependencies for all route types

| Direction | Trigger | Target Kinds | Mechanism |
|---|---|---|---|
| Service → Routes | Service created/updated/deleted | HTTPRoute, GRPCRoute, TLSRoute, TCPRoute, UDPRoute | `requeue_dependent_routes()` |

**Files**:
- `service_ref.rs` — ServiceRefManager (alias of RefManager)
- `handlers/service.rs` — on_change/on_delete: requeue dependent routes
- `route_utils.rs` — `register_service_backend_refs()` called by all route parsers

### 4. CrossNamespaceRefManager

**Purpose**: Revalidates cross-namespace references when ReferenceGrant changes

| Direction | Trigger | Target Kinds | Mechanism |
|---|---|---|---|
| ReferenceGrant → Routes | ReferenceGrant created/updated/deleted | Any route with cross-ns refs | `CrossNsRevalidationListener` |
| Post-init | `trigger_full_cross_ns_revalidation()` | All cross-ns referencing resources | One-time; covers ReferenceGrant ordering |

**Files**:
- `ref_grant/cross_ns_ref_manager.rs` — namespace→resource ref manager
- `ref_grant/revalidation_listener.rs` — listener + post-init revalidation functions

### 5. ListenerPortManager

**Purpose**: Resolves Gateway port conflicts (multiple Gateways on same port)

| Direction | Trigger | Target Kinds | Condition |
|---|---|---|---|
| Gateway → Gateway | Port conflict detected | Other Gateways | `get_conflicting_gateways()` |
| Gateway on_delete | Port freed | Previously conflicting Gateways | Requeue to clear Conflicted status |

**Files**:
- `handlers/gateway.rs` — on_change/on_delete: conflict detection + requeue

### 6. AttachedRouteTracker

**Purpose**: Tracks which routes are attached to which Gateways for status updates

| Direction | Trigger | Target Kinds | Mechanism |
|---|---|---|---|
| Route → Gateway | Attachment changed | Parent Gateway(s) | `requeue_parent_gateways()` |

**Files**:
- `handlers/mod.rs` — `update_attached_route_tracker()`, `requeue_parent_gateways()`

## Post-Init Revalidation (CachesReady)

After all processors complete `on_init_done()`, three revalidation functions run:

1. `trigger_full_cross_ns_revalidation()` — requeue all resources with cross-ns refs
2. `trigger_gateway_secret_revalidation()` — requeue all Gateways
3. `trigger_gateway_route_revalidation()` — requeue all routes in gateway_route_index

This handles any resource ordering issues during init.

**Called in**:
- `kubernetes/center.rs` (primary init + HA all-serve reload)
- `file_system/center.rs`

## Handler on_change/on_delete Checklist

| Handler | on_change | on_delete | Registers |
|---|---|---|---|
| HTTPRoute | Yes | Yes | gateway_route_index, attached_route_tracker, cross_ns_ref |
| GRPCRoute | Yes | Yes | gateway_route_index, attached_route_tracker, cross_ns_ref |
| TLSRoute | Yes | Yes | gateway_route_index, attached_route_tracker, service_ref |
| EdgionTls | Yes | Yes | gateway_route_index, secret_ref |
| TCPRoute | Yes | Yes | attached_route_tracker, service_ref |
| UDPRoute | Yes | Yes | attached_route_tracker, service_ref |
| Gateway | Yes | Yes | listener_port_manager, gateway_route_index (consumer) |
| Secret | Yes | Yes | Triggers cascading requeue |
| Service | Yes | Yes | Triggers dependent route requeue |
| ReferenceGrant | Yes | Yes | CrossNsRevalidationListener |
| ConfigMap | Yes | Yes | No requeue deps |
| EndpointSlice | — | — | Consumed directly by Gateway at request time |
| Endpoints | — | — | Consumed directly by Gateway at request time |
| GatewayClass | — | — | Referenced by Gateway, no requeue |
| EdgionGatewayConfig | — | — | Referenced by Gateway, no requeue |

## Cycle Safety

- `TriggerChain` tracks causality and prevents infinite loops
- `max_trigger_cycles = 5` (configurable)
- Route on_change → requeue Gateway only when attachments change (NOT when
  hostname/port resolution changes)
- Gateway on_change → requeue routes only when hostnames/ports actually change
- This breaks the Route ↔ Gateway cycle

## Debugging

- Enable `RUST_LOG=debug` to see requeue events in controller logs
- Look for `"Listener config changed, requeue referencing routes"` in Gateway handler
- Look for `"Triggering cascading requeue"` in Secret handler
- `"Requeuing all gateway-referencing routes for post-init revalidation"` indicates
  the post-init route revalidation is firing
