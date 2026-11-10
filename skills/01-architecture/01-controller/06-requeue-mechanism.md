---
name: controller-requeue
description: Cross-resource requeue mechanism — runtime-owned dispatch, dependency managers/indexes, post-init revalidation, intentional cycles, and TriggerChain safety.
---

# Cross-resource Requeue Mechanism

## Overview

The Controller uses a requeue-based reconciliation pattern to handle dependencies between resources. When resource A changes, resource B that depends on A is re-enqueued (re-processed) to pick up the latest state. This loosely coupled design supports any resource arrival order — it works correctly during both the init phase and runtime.

Core components:

- **Workqueue** (one per resource type) — ready queue + delay heap, supports dedup (dirty requeue pattern)
- **ControllerRuntime.processor_registry** — per-ConfCenter processor ownership and cross-resource dispatch
- **TriggerChain** — causality trace chain that records the requeue cascade path to prevent infinite loops

## Core Trigger Categories

The categories below are navigation aids, not an exhaustive fixed count. Handler
hooks also cover route delegation, policy-sibling precedence, EndpointSlice,
ConfigMap, plugin/config-data references, and other runtime-owned indexes.

### 1. GatewayRouteIndex

**Purpose**: resolve Route ↔ Gateway dependencies (hostname, port, sectionName)

| Direction | Trigger | Target resources | Mechanism |
|---|---|---|---|
| Gateway → Routes | Published listener attachment inputs changed, or Gateway was unpublished | Registered Route kinds | `after_publish` atomically compares associated listener records and collects dependents; `after_unpublish` clears the record and requeues dependents |
| Gateway → EdgionTls | Every publication/unpublication | EdgionTls | Refresh the controller-owned attachment proof after the cache transition |
| Route → Gateway | Route reconciliation or dropped parentRef | Gateway | Requeue current and dropped parents so attachment status converges; Gateway routing-input comparison prevents feedback loops |
| Post-init | `trigger_gateway_route_revalidation()` | All routes registered in the index | One-shot run that covers the case where Gateway arrives before Route |

**Registration rule**: a Handler that resolves Gateway dependencies must maintain the GatewayRouteIndex on apply and remove its edges on delete. Ordinary Route handlers finalize their edges in `on_change`; EdgionTls installs its exact edges in `parse` before proof lookup.

TLSRoute additionally pre-registers new parent edges before reading Gateway
snapshots. Its normal `on_change` still replaces the complete edge set and
requeues dropped parents after attachment counts change. EdgionTls uses its
existing dependency-before-proof-read barrier for the same publication race.

**Key files**:
- `gateway_route_index.rs` — forward/reverse index + associated listener routing-input records
- `handlers/gateway.rs` — after_publish/after_unpublish: update comparison records and requeue routes after cache publication/removal
- `handlers/{http_route,grpc_route,tls_route}.rs` — on_change: finalize index edges; TLSRoute also pre-registers in parse
- `handlers/edgion_tls.rs` — parse: register before proof lookup; on_change has no index work

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

### 6. EdgionGatewayConfig → GatewayClass (params revalidation)

**Purpose**: keep the GatewayClass `ResolvedRefs` / `Accepted` conditions in sync with whether the `EdgionGatewayConfig` named by `parametersRef` exists (controller-07) and with this deployment's view of that EGC's references (ctrl-proc2-02). Secret/ConfigMap, EdgionConfigData (IpList) and EdgionPlugins changes requeue the registered EGC through their own reference managers, which then requeues the classes here.

| Direction | Trigger | Target resources | Mechanism |
|---|---|---|---|
| EdgionGatewayConfig → GatewayClass | EGC create/update/delete, or any EGC reprocess (incl. dependency-driven requeues) | GatewayClass | `EdgionGatewayConfigHandler::{after_publish,after_unpublish}` requeue **all** GatewayClasses (brute-force; cardinality ~1–3) so each re-resolves `parametersRef` |
| Post-init | `trigger_gatewayclass_params_revalidation()` | All GatewayClasses | One-shot run covering the case where a GatewayClass is processed before its EGC during init |

**Key files**:
- `handlers/edgion_gateway_config.rs` — after_publish/after_unpublish: `requeue_all_gateway_classes()`
- `handlers/gateway_class.rs` — `update_status()` resolves `parametersRef` existence via the processor registry
- `ref_grant/revalidation_listener.rs` — `trigger_gatewayclass_params_revalidation()`

> This is **EGC → GatewayClass**, distinct from the still-rejected **EGC → Gateway** requeue (see the
> note below). The GatewayClass handler now derives status from the EGC, so the requeue is no longer
> a no-op — exactly the re-evaluation trigger anticipated by the controller-06 finding.

## Post-Init Revalidation (CachesReady)

After all processors are cache-ready, `run_post_cache_ready_revalidations()` runs the authoritative sequence:

| # | Function | Effect |
|---|---|---|
| 1 | `trigger_full_cross_ns_revalidation()` | Requeue all resources with cross-namespace references |
| 2 | `trigger_gateway_secret_revalidation()` | Requeue all Gateways |
| 3 | `trigger_gateway_route_revalidation()` | Requeue all routes registered in gateway_route_index |
| 4 | `trigger_gatewayclass_params_revalidation()` | Requeue all GatewayClasses to re-resolve `parametersRef` |
| 5 | `trigger_delegation_parent_revalidation()` | Requeue every delegating parent HTTPRoute so edges, keys and levels settle after init |
| 6 | `trigger_grpc_delegation_parent_revalidation()` | Same for delegating parent GRPCRoutes |

Call sites:
- `kubernetes/center.rs` (main initialization + HA all-serve reload)
- shared non-Kubernetes lifecycle (FileSystem and Etcd)

## Handler on_change/on_delete Inventory

| Handler | on_change | on_delete | Registered managers |
|---|---|---|---|
| HTTPRoute | yes | yes | gateway_route_index, attached_route_tracker, cross_ns_ref, service_ref |
| GRPCRoute | yes | yes | gateway_route_index, attached_route_tracker, cross_ns_ref, service_ref |
| TLSRoute | yes | yes | gateway_route_index, attached_route_tracker, service_ref |
| EdgionTls | no index work; registered in parse | yes | gateway_route_index, secret_ref |
| TCPRoute | yes | yes | attached_route_tracker, service_ref |
| UDPRoute | yes | yes | attached_route_tracker, service_ref |
| Gateway | after_publish | after_unpublish + on_delete | gateway_route_index (consumer) |
| Secret | yes | yes | triggers cascading requeue |
| Service | yes | yes | triggers requeue of dependent routes |
| ReferenceGrant | yes | yes | CrossNsRevalidationListener |
| ConfigMap | yes | yes | requeues resources that depend on referenced ConfigMap data |
| EndpointSlice | yes | yes | requeues affected backend/route consumers |
| GatewayClass | after_publish | after_unpublish | requeues matching raw Gateways for ownership admission/withdrawal; also target of EdgionGatewayConfig (path 6) |
| EdgionGatewayConfig | yes | yes | requeues all GatewayClasses (path 6); still no requeue toward Gateway (see note) |

> **Ownership is a real GatewayClass → Gateway dependency:** Kubernetes Gateway eligibility
> reads raw class ownership and the matching published class UID. Class publication and
> unpublication therefore requeue matching **raw** Gateway keys after the cache mutation.
> This is not parametersRef-status propagation or listener reconstruction. EGC still has
> no direct Gateway requeue; parameter consumers on the data plane retain their own lifecycle.
> See `skills/04-review/handler-requeue/intentional-design/controller-06-gatewayclass-egc-no-gateway-requeue.md`.

## Cycle Safety

### TriggerChain

`TriggerChain` records the full requeue cascade path (e.g. `HTTPRoute/ns/r1 → Gateway/ns/gw1 → HTTPRoute/ns/r2`). Every call to `ctx.requeue(kind, key)` checks whether the target (kind, key) appears in the chain more times than `max_trigger_cycles` (default 5). If so, the requeue is dropped and an error is logged.

### Common Cycles and Change Guards

Route ↔ Gateway is one important bidirectional relationship:

- **Gateway after_publish → requeue Routes**: triggered only when associated Listener routing inputs change. Records include logical name, optional hostname, physical key, raw port/protocol and allowedRoutes; metadata, status, proof epochs and TLS credentials are excluded. Unpublish clears the record and requeues dependents.
- **Route on_change → requeue Gateway**: current parent status is refreshed on reconciliation; dropped parents are also requeued. A status-only Gateway publication does not change the routing-input record and cannot restart the loop.
- **Gateway → EdgionTls**: the existing proof-consumer path refreshes every publication epoch independently of the ordinary Route change gate.

Route delegation can also requeue parents and children of the same kind (target
children, declared parents, and level dependents, at most four hops; a cycle
terminates because levels above five become "none"), and
policy sibling-precedence changes can requeue peers. Change-detection guards
reduce churn; TriggerChain remains the final bound for every cycle.

Do not model the full requeue graph as a DAG. Before adding an edge, identify its
reverse or sibling paths, ensure stable no-change behavior, and preserve the
TriggerChain through delayed-queue dedup/merge.
