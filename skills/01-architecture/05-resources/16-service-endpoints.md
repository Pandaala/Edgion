---
name: resource-service-endpoints
description: Service + EndpointSlice + Endpoint resources — backend discovery, cascading requeue, two endpoint modes.
---

> **Status: partially removed**. The `Endpoints` resource handler was removed in commit `43acc97` (2026-05-06); backend discovery now uses EndpointSlice/Service exclusively. The Endpoints-related content below is preserved for historical context.

# Service + EndpointSlice + Endpoint resources

> **Common flow**: see [00-resource-flow.md](00-resource-flow.md)

These three Kubernetes core resources together form the backend service-discovery mechanism. Service defines the logical service; EndpointSlice (preferred) and Endpoint (legacy) provide the actual backend Pod IPs and ports.

## Source code locations

- Controller Handlers:
  - `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/service.rs`
  - `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/endpoint_slice.rs`
  - Endpoints handler — formerly at `controller/conf_mgr/sync_runtime/resource_processor/handlers/endpoints.rs`; removed in commit `43acc97`, 2026-05-06
- Gateway ConfHandlers:
  - `edgion-gateway/src/backends/discovery/services/conf_handler_impl.rs`
  - `edgion-gateway/src/backends/discovery/endpoint_slice/conf_handler_impl.rs`
  - Endpoint ConfHandler — formerly at `gateway/backends/discovery/endpoint/conf_handler_impl.rs`; removed in commit `43acc97`, 2026-05-06

## Service

### Controller-side processing

#### parse

No special handling; passes through directly.

#### on_change / on_delete

Triggers cascading requeue via `ServiceRefManager`: looks up all route resources (HTTPRoute, GRPCRoute, TCPRoute, TLSRoute, UDPRoute) using this Service as a backendRef and requeues them one by one. This ensures:
- On Service creation: routes that previously had ResolvedRefs=False are revalidated to True
- On Service deletion: route ResolvedRefs goes from True to False
- On Service update: routes re-resolve their backend configuration

### Gateway-side processing

After Service is synced to the Gateway side, it is used as backend-discovery metadata (port mapping, protocol, etc.).

## EndpointSlice (preferred mode)

### Controller-side processing

#### parse

No special handling; passes through directly. EndpointSlice data is consumed on the Gateway side.

### Gateway-side processing

EndpointSlice is the Kubernetes-recommended endpoint-discovery mechanism, providing the backend Pod address list for a Service. The Gateway side builds the load-balancing target pool from EndpointSlice for backend selection during request forwarding and for the health-check target list.

## Endpoint (legacy mode)

### Controller-side processing

#### parse

No special handling; passes through directly.

### Gateway-side processing

Endpoint is the legacy Kubernetes endpoint-discovery mechanism. Used as a fallback when EndpointSlice is unavailable. Functionally equivalent to EndpointSlice but with worse performance and scalability (no sharding support).

## ServiceRefManager mechanism

During parse, route resources (e.g., HTTPRoute) register Service references from backendRefs into ServiceRefManager via `register_service_backend_refs()`. When a Service changes, ServiceHandler obtains the dependency list via `get_service_ref_manager().get_refs()` and requeues them.

Registration lifecycle:
- Routes register during parse
- Routes clear via `clear_service_backend_refs()` on delete
- On route updates, old registrations are cleared and new references are registered

## Cross-resource relationships

| Direction | Target resource | Mechanism | Notes |
|---------|---------|---------|------|
| Service ← HTTPRoute | HTTPRoute | backendRef + ServiceRefManager | Service changes requeue routes |
| Service ← GRPCRoute | GRPCRoute | backendRef + ServiceRefManager | Same as above |
| Service ← TCPRoute | TCPRoute | backendRef + ServiceRefManager | Same as above |
| Service ← TLSRoute | TLSRoute | backendRef + ServiceRefManager | Same as above |
| Service ← UDPRoute | UDPRoute | backendRef + ServiceRefManager | Same as above |
| Service ← BackendTLSPolicy | BackendTLSPolicy | targetRef | Policy binding target |
| EndpointSlice → Service | Service | kubernetes.io/service-name label | Associated to a Service |
| Endpoint → Service | Service | same name | Associated to the Service with the same name |
