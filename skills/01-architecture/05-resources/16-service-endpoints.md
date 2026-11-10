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

Route dependency owners retain their actual metadata namespace, including `None`
for namespace-less local inputs. Service and EndpointSlice events enqueue that
exact raw worker key. Backend target defaults and GatewayRouteIndex identities
still use the effective namespace (`default` when omitted). Registration,
replacement, policy notification and deletion must use the same raw owner;
normalizing it would enqueue `default/name` for a source stored under `name`.
The R4a worker matrix covers GRPC/TCP/TLS/UDP with both namespace forms, extending
the HTTP R3b precedent, including ordinary same-UID Service updates with changed
resource version and port mapping. It proves in-process queue/raw-store reconciliation, not
live configuration delivery or every other event source's identity handling.

The Gateway discovery coordinator combines Service port/protocol metadata with EndpointSlice
sources and publishes one immutable generation of canonical Service sections.
Native HTTP/gRPC selects ExternalName behavior from the actual Service type, using
that same generation for its hostname, annotations, checked Service port, protocol
and appProtocol across DNS resolution. The reference remains standard `Service`;
pseudo kinds are unsupported. Subsequent attempts recapture geometry while
request-captured policy and live health retain their existing boundaries. An
`ExternalName` Service in `gateway` mode is fed by Gateway-local slices from the
DNS endpoint table, in `controller` mode by Controller-published slices; L4 uses
the same sources plus the per-connection `system` mode. See
[ExternalName resolution modes](../../02-features/03-resources/16-service-backends.md#externalname-resolution-modes).

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

## Controller DNS references and generated slices

DNS annotation references borrow the actual Service or Gateway. Omitted source
namespace uses default for permission, while dependency owners keep raw metadata.
The explicit namespace/name string grammar trims each half and rejects missing
or additional slashes without imposing a new DNS-label restriction. Valid denied
references retain cross-namespace recovery edges; invalid identities create none.
The Controller DNS resolver checks the same current Grant permission before using
a Service's LinkSys reference. Denial drops only the tenant reference candidate:
inline servers and administrative defaults retain their existing precedence.

Only Services annotated `edgion.io/dns-resolution-mode: controller` are resolved.
Answers become in-memory synthetic EndpointSlices (`edgion.io/endpoint-origin:
controller-dns`, one per family, named `<service>-edgion-ctl-dns[-ipv6]`) held in
`ControllerRuntime::dns_synthetic_slices`. The resolver enqueues changed keys into
the EndpointSlice processor, whose `synthetic_object` hook supplies them when the
real source has no object for the key; they then publish through conf_sync like
any watched slice. No apiserver write, owner reference or garbage collection is
involved, and an operator `label_selector` cannot filter them out. An unchanged
answer enqueues nothing; a removed or switched Service withdraws its slices on the
next pass, and `ControllerRuntime::reset` aborts the resolver and clears them.
