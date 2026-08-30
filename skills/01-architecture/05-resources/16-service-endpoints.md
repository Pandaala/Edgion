---
name: resource-service-endpoint-slice
description: Service and EndpointSlice resources — Controller dependency requeue and Gateway backend discovery.
---

# Service and EndpointSlice resources

> Common flow: [00-resource-flow.md](00-resource-flow.md)
> External behavior: [../../02-features/03-resources/16-service-backends.md](../../02-features/03-resources/16-service-backends.md)

Edgion backend discovery uses Kubernetes Service metadata plus EndpointSlice address/port data.
The legacy Endpoints resource is no longer in `ResourceKind`, Controller handlers, or Gateway
ConfigClient.

## Service

The Controller stores Service objects and maintains Service dependencies. Routes,
BackendTLSPolicy, and EdgionBackendTrafficPolicy register target Service keys through the
runtime-owned reference manager. A Service create/update/delete requeues those owners so
`ResolvedRefs`, resolved ports, and policy target status are recomputed.

The Gateway `ServiceStore` supplies Service port/protocol metadata used with EndpointSlice data.

## EndpointSlice

The Controller passes EndpointSlice objects through and requeues dependent resources where the
handler's indexes require it. On the Gateway, `EpSliceHandler` groups slices by the
`kubernetes.io/service-name` label and updates each Service's `ServiceSelectorCell`, health-check
targets, and removed-backend runtime state.

## Relationships

| Direction | Mechanism |
|-----------|-----------|
| Route → Service | backendRef registration; Service change requeues route |
| BackendTLSPolicy → Service | targetRef registration and status recomputation |
| EdgionBackendTrafficPolicy → Service | targetRefs, status, conflict arbitration |
| EndpointSlice → Service | `kubernetes.io/service-name` label and Gateway backend pool |

## Key sources

- `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/service.rs`
- `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/endpoint_slice.rs`
- `edgion-gateway/src/backends/discovery/services/conf_handler_impl.rs`
- `edgion-gateway/src/backends/discovery/endpoint_slice/conf_handler_impl.rs`
