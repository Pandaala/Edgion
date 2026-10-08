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

The Gateway discovery coordinator combines Service port/protocol metadata with EndpointSlice
sources and publishes one immutable generation of canonical Service sections.
Native HTTP/gRPC selects ExternalName behavior from the actual Service type, using
that same generation for its hostname, annotations, checked Service port, protocol
and appProtocol across DNS resolution. The reference remains standard `Service`;
pseudo kinds are unsupported. Subsequent attempts recapture geometry while
request-captured policy and live health retain their existing boundaries. L4
uses published slices and has no Gateway self-resolving DNS branch.

## EndpointSlice

The Controller passes EndpointSlice objects through and requeues dependent resources where the
handler's indexes require it. On the Gateway, `EpSliceHandler` publishes slices through the
discovery coordinator, grouped by the `kubernetes.io/service-name` label. Section serving ports,
numeric direct-endpoint authorization indexes, health-check targets, and backend runtime state
are derived from that publication. See the [backend architecture](../02-gateway/07-backends.md).

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
