---
name: controller-requeue
description: Cross-resource requeue mechanism — 6 trigger paths, cycle detection, post-init revalidation, handler on_change/on_delete inventory.
---

# Cross-resource Requeue Mechanism

## Overview

The Controller uses a requeue-based reconciliation pattern to handle dependencies between resources. When resource A changes, resource B that depends on A is re-enqueued (re-processed) to pick up the latest state. This loosely coupled design supports any resource arrival order — it works correctly during both the init phase and runtime.

Core components:

- **Workqueue** (one per resource type) — ready queue + delay heap, supports dedup (dirty requeue pattern)
- **PROCESSOR_REGISTRY** (global singleton) — registers all Processors and provides cross-resource dispatch via `requeue(kind, key)` and `requeue_with_chain(kind, key, chain)`
- **TriggerChain** — causality trace chain that records the requeue cascade path to prevent infinite loops

## 5 Trigger Paths

### 1. GatewayRouteIndex

**Purpose**: resolve Route ↔ Gateway dependencies (hostname, port, sectionName)

| Direction | Trigger | Target resources | Mechanism |
|---|---|---|---|
| Gateway → Routes | Listener hostname or port changed | HTTPRoute, GRPCRoute, TLSRoute, EdgionTls | When `gateway_route_index.update_gateway_hostnames/ports()` returns true, requeue all routes associated in the index |
| Route → Gateway | parentRef attachment list changed | Gateway | Only requeues when the attachment list actually changes (decided by AttachedRouteTracker) |
| Post-init | `trigger_gateway_route_revalidation()` | All routes registered in the index | One-shot run that covers the case where Gateway arrives before Route |

**Registration rule**: any Handler that calls `lookup_gateway()` in `parse()` must also call `update_gateway_route_index()` in `on_change()` and `remove_from_gateway_route_index()` in `on_delete()`.

**Key files**:
- `gateway_route_index.rs` — forward/reverse index + hostname/port change-detection cache
- `handlers/gateway.rs` — on_change: update the cache and requeue routes
- `handlers/{http_route,grpc_route,tls_route,edgion_tls}.rs` — on_change: register into the index

### 2. SecretRefManager

**Purpose**: resolve Secret dependencies (TLS certificates, authentication credentials)

| Direction | Trigger | Target resources | Mechanism |
|---|---|---|---|
| Secret → dependents | Secret create/update/delete | Gateway, EdgionTls, EdgionPlugins, EdgionAcme | `trigger_cascading_requeue()` — query the forward index for all resources referencing this Secret and requeue each one |
| Post-init | `trigger_gateway_secret_revalidation()` | All Gateways | One-shot run that covers the case where Secret arrives later than Gateway |

**Key files**:
- `ref_manager.rs` — generic `BidirectionalRefManager<ResourceRef>`
- `handlers/secret.rs` — on_change/on_delete: cascading requeue
- `secret_utils/secret_ref.rs` — SecretRefManager type alias

### 3. ServiceRefManager

**Purpose**: resolve Service backend dependencies, covering all route types

| Direction | Trigger | Target resources | Mechanism |
|---|---|---|---|
| Service → Routes | Service create/update/delete | HTTPRoute, GRPCRoute, TLSRoute, TCPRoute, UDPRoute | `requeue_dependent_routes()` — query the forward index for all routes referencing this Service and requeue them |

**Key files**:
- `service_ref.rs` — ServiceRefManager (alias for `BidirectionalRefManager<ResourceRef>`)
- `handlers/service.rs` — on_change/on_delete: requeue dependent routes
- `route_utils.rs` — `register_service_backend_refs()`, called from every route Handler's parse()

### 4. CrossNamespaceRefManager

**Purpose**: when a ReferenceGrant changes, revalidate all affected cross-namespace references

| Direction | Trigger | Target resources | Mechanism |
|---|---|---|---|
| ReferenceGrant → referencing resources | ReferenceGrant create/update/delete | Routes and resources holding cross-namespace references | `CrossNsRevalidationListener` listens for change events and requeues affected resources |
| Post-init | `trigger_full_cross_ns_revalidation()` | All resources that registered cross-namespace references | One-shot run that handles ReferenceGrant arrival-order issues |

**Key files**:
- `ref_grant/cross_ns_ref_manager.rs` — namespace-level reference management
- `ref_grant/revalidation_listener.rs` — listener + post-init revalidation functions

### 5. AttachedRouteTracker

**Purpose**: track which Routes are attached to which Gateways, used to update the `attachedRoutes` count in Gateway status

| Direction | Trigger | Target resources | Mechanism |
|---|---|---|---|
| Route → Gateway | Attachment state changed | Parent Gateway | `requeue_parent_gateways()` — only triggered when the attachment list actually changes |

**Key files**:
- `attached_route_tracker.rs` — attachment-state storage
- `handlers/mod.rs` — `update_attached_route_tracker()`, `requeue_parent_gateways()`

## Post-Init Revalidation (CachesReady)

After all Processors complete `on_init_done()`, three revalidation functions run to resolve resource arrival-order issues from the init phase:

| # | Function | Effect |
|---|---|---|
| 1 | `trigger_full_cross_ns_revalidation()` | Requeue all resources with cross-namespace references |
| 2 | `trigger_gateway_secret_revalidation()` | Requeue all Gateways |
| 3 | `trigger_gateway_route_revalidation()` | Requeue all routes registered in gateway_route_index |

Call sites:
- `kubernetes/center.rs` (main initialization + HA all-serve reload)
- `file_system/center.rs`

## Handler on_change/on_delete Inventory

| Handler | on_change | on_delete | Registered managers |
|---|---|---|---|
| HTTPRoute | yes | yes | gateway_route_index, attached_route_tracker, cross_ns_ref, service_ref |
| GRPCRoute | yes | yes | gateway_route_index, attached_route_tracker, cross_ns_ref, service_ref |
| TLSRoute | yes | yes | gateway_route_index, attached_route_tracker, service_ref |
| EdgionTls | yes | yes | gateway_route_index, secret_ref |
| TCPRoute | yes | yes | attached_route_tracker, service_ref |
| UDPRoute | yes | yes | attached_route_tracker, service_ref |
| Gateway | yes | yes | gateway_route_index (consumer) |
| Secret | yes | yes | triggers cascading requeue |
| Service | yes | yes | triggers requeue of dependent routes |
| ReferenceGrant | yes | yes | CrossNsRevalidationListener |
| ConfigMap | yes | yes | no requeue dependency |
| EndpointSlice | — | — | stored directly; consumed by Gateway on demand |
| Endpoints | — | — | stored directly; consumed by Gateway on demand |
| GatewayClass | — | — | referenced by Gateway; no requeue (intentional — see note) |
| EdgionGatewayConfig | — | — | referenced by Gateway; no requeue (intentional — see note) |

> **Note (GatewayClass / EdgionGatewayConfig "no requeue" is intentional):** both resources are
> independently synced to the data plane and consumed directly at listener-build time
> (`src/core/gateway/runtime/server/base.rs:110-129`), and the controller-side `GatewayHandler`
> derives no status from them, so requeuing a Gateway on their change would be a no-op. See
> `skills/04-review/handler-requeue/controller-06-gatewayclass-egc-no-gateway-requeue.md`.

## Cycle Safety

### TriggerChain

`TriggerChain` records the full requeue cascade path (e.g. `HTTPRoute/ns/r1 → Gateway/ns/gw1 → HTTPRoute/ns/r2`). Every call to `ctx.requeue(kind, key)` checks whether the target (kind, key) appears in the chain more times than `max_trigger_cycles` (default 5). If so, the requeue is dropped and an error is logged.

### Breaking the Route ↔ Gateway Cycle

There is a potential bidirectional requeue between Route and Gateway:

- **Gateway on_change → requeue Routes**: triggered only when hostname/port **actually changed** (compared via the `gateway_route_index` cache)
- **Route on_change → requeue Gateway**: triggered only when the **attachment list actually changed** (decided by `AttachedRouteTracker`)

These two conditions ensure that, in the vast majority of cases, no cycle forms. Even when one does, TriggerChain's `max_trigger_cycles=5` will force termination.

### DAG Constraint

The requeue relations between resource types are essentially a DAG (directed acyclic graph) — there are no cycles other than the Route ↔ Gateway pair. The breaking mechanisms above ensure that this single cycle does not result in unbounded cascading.
