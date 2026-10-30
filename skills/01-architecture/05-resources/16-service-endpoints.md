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

## Controller DNS references and generated slices

DNS annotation references borrow the actual Service or Gateway. Omitted source
namespace uses default for permission, while dependency owners keep raw metadata.
The explicit namespace/name string grammar trims each half and rejects missing
or additional slashes without imposing a new DNS-label restriction. Valid denied
references retain cross-namespace recovery edges; invalid identities create none.
The independent raw Kubernetes Service DNS loop checks the same current Grant
permission before LinkSys GET. Denial drops only the tenant reference candidate:
inline servers and administrative defaults retain their existing precedence.

A successful content fingerprint includes the effective family, canonical addresses
and projected ports. Skip requires the complete expected current-UID family set,
with no obsolete owned sibling. The Controller patches desired slices first and
then deletes obsolete proven-owned siblings; any failure keeps convergence pending.
Policy narrowing withdraws forbidden families before DNS, so a timeout cannot
preserve now-forbidden last-good output. The Service's explicit family also gates
canonical Gateway ExternalName membership, including L4; ordinary Services are
unchanged. Gateway/EGC family defaults do not filter shared Controller publications.

Deletion proves generated name/namespace, managed-by and synthetic labels, Service
label, and a v1/Service owner name with nonempty UID. A previous incarnation's owner
UID is allowed for obsolete siblings. The observed slice UID/resourceVersion are
delete preconditions; 404 completes absence, conflicts remain retryable. Foreign or
unproven slices are preserved and prevent claiming convergence. See the
[family convergence guard](../../04-review/controller/regression-guards/dns-owned-family-slice-convergence.md).
