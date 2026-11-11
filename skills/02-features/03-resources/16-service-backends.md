---
name: backends-features-detail
description: Full reference for backend Service/EndpointSlice discovery.
---

# Backend Features

## Service Discovery

Edgion discovers backend endpoints via the Kubernetes EndpointSlice API. The Controller continuously syncs EndpointSlice resources via K8s watch, and pushes endpoint information to the Gateway via gRPC ConfigSync.

### Service Resource

Routes reference Services via `backendRefs`:

```yaml
backendRefs:
  - name: my-service          # Service name
    port: 8080                # Service port
    weight: 100               # Traffic weight
```

Service changes (e.g., port updates) automatically trigger reprocessing of dependent routes via ServiceRefManager.

The Gateway publishes Service and EndpointSlice input as one immutable generation. A route port
selects a canonical Service section by exact number or name; a missing port is invalid rather than
falling back to another Service port. Equivalent EndpointSlice origins for one socket are merged,
and all supported load-balancer selectors are compiled before the generation becomes visible.
Invalid or unidentified input has no legacy raw Service/EndpointSlice projection, so other gateway
readers cannot route around the unavailable state.

Source and endpoint identities are transaction-scoped. Kubernetes UID recreation, authoritative
delete/recreate, any membership-affecting Service-port change, and EndpointSlice proof changes
publish a fresh membership. Health probes and passive-outlier attempt results are accepted only
when both that membership and the captured policy generation are still current.


## ExternalName resolution modes

An `ExternalName` Service is resolved in one of three modes, selected by the
Service annotation `edgion.io/dns-resolution-mode`, else by the global
`EdgionGatewayConfig.spec.dnsResolver.endpointTable` switch (`true`, the default,
selects `gateway`; `false` selects `system`). `controller` is only selected per
Service. An unknown annotation value fails closed (`BackendDnsResolutionFailed`).
No mode falls back to another.

| Mode | Who resolves | Endpoints | Protocols |
|---|---|---|---|
| `gateway` (default) | Gateway endpoint table, in the background | Gateway-local synthetic EndpointSlices: ordinary LB, health check, outlier ejection, sticky sessions | HTTP, gRPC, TCP, UDP, TLS |
| `system` | OS resolver on every request attempt / connection / UDP session | One address per attempt: OS order, rotated start; no pool, no health state | HTTP, gRPC, TCP, UDP, TLS |
| `controller` | Controller, fixed `dns_resolver.refresh_interval` | Controller-published in-memory EndpointSlices relayed by conf_sync | HTTP, gRPC, TCP, UDP, TLS |

**Gateway endpoint table** (`edgion-gateway/src/backends/discovery/dns_table/`).
One entry per distinct hostname + resolver identity + address family; Services
sharing a target share one query. Entries are created and released only by
Service / EGC / LinkSys events, never by requests, and an entry is deleted with
its last referring Service. A Service relist is a mark-and-sweep, and a periodic
pass withdraws any Gateway-local slice the table does not own. `maxEntries`
(default 4096) bounds the table; new targets beyond it fail closed and live ones
are never evicted. Answers are normalized (IPv4-mapped folded, link-local and
unspecified dropped, deduplicated, IPv4 then IPv6 ascending; nothing left counts as
an empty answer); an unchanged answer touches nothing, so health and outlier state
survive refreshes. Refresh is
`clamp(ttl * ttlFactor, minRefreshSeconds, maxRefreshSeconds)` (TTL capped at the
RFC 2181 maximum first), `fallbackRefreshSeconds` without a TTL, or a fixed
`refreshIntervalSeconds` (exempt from the bounds, 2 s floor), each jittered ±10 %
and re-clamped. A transient failure keeps the last good set up to
`maxStaleSeconds` from the first failure, enforced on time even while a retry is
queued (`0` keeps nothing); an authoritative empty answer withdraws at once. A
resolver or family switch keeps serving the previous answer (new family only) until
the new target's first result, unless no address of the new family remains. All
publications of one table mutation reach the coordinator in one transaction.
HTTP requests and TCP/TLS connections wait at most 2 s, once each, for a pending
target's first answer (lock-free pending map; admission is checked before an L4
wait), then select from whatever exists (none: 503; an expired request deadline:
504); the UDP datagram path never waits. `system` mode drops unspecified answers
and bounds its lookup by the L4 connect timeout or the remaining request / gRPC
deadline. Resolver precedence is Service annotation -> EGC default ->
OS resolver; family precedence is Service annotation -> EGC -> `ipv4Only`. The
Gateway resource annotations do not apply to `ExternalName`. Admin view:
`GET /api/v1/dns/endpoint-table`; counts also appear in `store-stats`.

**Coordinator partition.** Gateway-local slices live under their own key prefix:
a conf_sync relist can neither remove nor resurrect them. An `ExternalName`
Service in `controller` mode reads conf_sync slices only; any other
`ExternalName` Service reads Gateway-local slices only; every other Service reads
conf_sync slices only.

**Controller mode** (`edgion-controller/src/services/dns_resolver/`). Only
explicitly annotated Services are resolved, through Service annotation ->
Controller `dns_resolver` default -> OS resolver. Answers become in-memory
synthetic EndpointSlices that enter the EndpointSlice processor through
`ProcessorHandler::synthetic_object` (a sourced object with the same key wins)
and reach Gateways over conf_sync. Nothing is written to the apiserver, so it
works in Kubernetes, file-system and etcd modes, in every process that runs the
resource pipeline. The task belongs to `ControllerRuntime`: started with the
first target (restarted after a panic), aborted and cleared by `reset()`; an
epoch fences writes of an aborting task. Its first pass waits until every
processor is registered and ready, so httpdns LinkSys and ReferenceGrant state
are known. A hostname or resolver change never inherits the old answer;
transient failures keep the last good answer for at most 300 s (the OS resolver
reports NXDOMAIN as transient). Endpoints exist only in Controller memory: after a
Controller restart or leadership change they return with the first answer.

## Native ExternalName attempts

HTTP/gRPC use an ordinary permitted `Service` reference (omitted/empty group,
omitted/exact Service kind, required declared port). The captured actual Service
`spec.type: ExternalName` selects DNS behavior; pseudo backend kinds and literal
`core` do not grant a native target. `CapturedServiceSelection` keeps the Service
source Arc, checked section and membership-matched runtime snapshot from one
joint discovery generation. Type, hostname, annotations, Service port, protocol
and appProtocol therefore remain coherent through a `system`-mode DNS await. A
later peer attempt recaptures geometry; request-captured EBTP/TLS policy,
including absence, keeps its existing lifetime. Endpoint health/outlier atomics
remain live.

`gateway` and `controller` modes use ordinary section selection (ready slots,
health, outlier, LB policy, sticky revalidation). `system` mode does not consume
a sticky EndpointSlice target; a retry prefers an address no earlier attempt
dialed. L4 runs the same decision in `backends::prepare_l4_service` (one
projection load, reused for selection). On TCP/TLS a `system`-mode lookup is
uncached; a DNS failure ends a TLS connection instead of retrying. On UDP the
receive loop must not block, so `system` mode resolves through the shared
upstream answer cache (hostname-keyed, single-flight) waiting at most 250 ms; a
session retires when its Service stops being a `system`-mode ExternalName of the
same hostname, its port disappears, or it idles out.

An unavailable Service returns `BackendServiceNotFound` (500); a missing live
section returns `BackendPortResolutionFailed` (503), and an absent/empty external
hostname returns `BackendExternalNameNotFound` (503) without querying DNS. DNS
results pass canonical address safety, including mapped IPv6, before the shared
TLS peer builder. Without BackendTLSPolicy, even port 443 remains plaintext.
Custom outbound reference modes and annotation reference authority are separate
contracts.

## Health Checks, Circuit Breaking, and Load Balancing

These are no longer Service annotations or route-level `ExtensionRef` filters —
they are configured via the `EdgionBackendTrafficPolicy` CRD, attached to one
or more Services via `spec.targetRefs`. See
[20-edgion-backend-traffic-policy.md](20-edgion-backend-traffic-policy.md) for
the full schema, defaults, and conflict semantics.

## BackendTLSPolicy

For gateway-to-backend TLS configuration, see [12-backend-tls-policy.md](12-backend-tls-policy.md).

## DNS address families

DNS-derived backend addresses default to `ipv4Only`. Set `ipv6Only` for IPv6 only
or `all` for both families. These are the only accepted values; an empty or unknown
annotation fails DNS dispatch rather than inheriting or widening the policy.
`all` does not enable Happy Eyeballs or connection racing. IPv6 requires a working
upstream IPv6 network.

`controller` mode uses the Service annotation `edgion.io/dns-lookup-family`, then
Controller `dns_resolver.lookup_family` (read at Controller startup), then
`ipv4Only`. `gateway` and `system` modes use the Service annotation, then
`EdgionGatewayConfig.spec.dnsResolver.lookupFamily`, then `ipv4Only`; the family is
part of the endpoint-table key, so a policy change re-keys the target. AI
external hostnames and external jumps use Gateway annotation then EGC policy.
Resolver selection and family inheritance are independent: setting only the
family does not replace a chosen DNS server or HTTPDNS provider.

```yaml
# ExternalName Service metadata
metadata:
  annotations:
    edgion.io/dns-lookup-family: ipv6Only
```

In `controller` mode a family change republishes the last good answer filtered
by the new policy at once, then re-resolves. In `gateway` mode it re-keys the
target: the old slices are withdrawn and the new target's first answer publishes.
Ordinary Service slices, literal non-DNS
backends, listeners, DNS-server transport and HTTPDNS bootstrap are outside this
policy.
