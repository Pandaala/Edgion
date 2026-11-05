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


## Native ExternalName attempts

HTTP/gRPC use an ordinary permitted `Service` reference (omitted/empty group,
omitted/exact Service kind, required declared port). The captured actual Service
`spec.type: ExternalName` selects DNS behavior; pseudo backend kinds and literal
`core` do not grant a native target. `CapturedServiceSelection` keeps the Service
source Arc, checked section and membership-matched runtime snapshot from one
joint discovery generation. Type, hostname, annotations, Service port, protocol
and appProtocol therefore remain coherent through a DNS await. A later peer
attempt recaptures geometry; request-captured EBTP/TLS policy, including absence,
keeps its existing lifetime. Endpoint health/outlier atomics remain live.

Controller mode uses the captured section when it has ready slots. Health or
outlier rejection does not turn readiness into DNS fallback. Without ready slots
it bridges through cached DNS; Gateway mode always uses cached DNS and per-request
mode resolves without that cache. Resolver precedence remains Service -> Gateway
-> EGC -> system through Common `ResolverSpec`, including prepared HTTPDNS. The
cache key remains Service key + hostname + resolver fingerprint. Self-resolving
DNS does not consume a sticky EndpointSlice target. Ordinary selection and sticky
revalidation use the same captured section/runtime.

An unavailable Service returns `BackendServiceNotFound` (500); a missing live
section returns `BackendPortResolutionFailed` (503), and an absent/empty external
hostname returns `BackendExternalNameNotFound` (503) without querying DNS. DNS
results pass canonical address safety, including mapped IPv6, before the shared
TLS peer builder. Without BackendTLSPolicy, even port 443 remains plaintext.
TCP/UDP/TLS can consume controller-published slices but gain no asynchronous
Gateway DNS fallback. Custom outbound reference modes and annotation reference
authority are separate contracts.

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

Controller publication uses the ExternalName Service annotation
`edgion.io/dns-lookup-family`, then Controller `dns_resolver.lookup_family`, then
`ipv4Only`. The Controller setting takes effect after restarting the Controller.
Gateway self-resolution uses Service annotation, Gateway annotation, selected global
`EdgionGatewayConfig.spec.dnsResolver.lookupFamily`, then `ipv4Only`, in that order.
Native AI external hostnames use Gateway then EGC policy. Resource updates are live.
Resolver selection and family inheritance are independent: setting only the family
does not replace a chosen DNS server or HTTPDNS provider.

Controller EndpointSlices are shared. Gateway/EGC defaults affect self-resolution
and the empty-slice bridge, not already-published Controller endpoints. Set the
Service annotation to pin both planes; its explicit family also gates stale
ExternalName slice membership, including L4. Ordinary Service slices, literal
non-DNS backends, listeners, DNS-server transport and HTTPDNS bootstrap are outside
this policy. TCP/UDP/TLS still require published EndpointSlices.

```yaml
# ExternalName Service or Gateway metadata
metadata:
  annotations:
    edgion.io/dns-lookup-family: ipv6Only
```

Successful complete nonempty Controller answers replace the owned family set,
including deleting an obsolete IPv4/IPv6 sibling. Same-policy resolution errors
retain allowed last-good addresses; narrowing the policy withdraws forbidden
families even if the next query fails. Gateway address-cache keys include the
family, so live policy changes cannot read the old policy's cached answer.
