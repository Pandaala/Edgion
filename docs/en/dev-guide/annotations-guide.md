# Annotations And `edgion.io/*` Extension Keys

This document explains which `edgion.io/*` keys in Edgion are real `metadata.annotations`, which belong to `options` or `labels`, and which are reserved or test-only.

If you are changing code, debugging behavior, or updating docs, start with the agent-facing entry:

- [05-annotations-reference.md](https://github.com/Pandaala/Edgion/blob/main/skills/02-features/05-annotations/00-annotations-overview.md)

The detailed key tables are split into these references:

- [annotations-metadata.md](https://github.com/Pandaala/Edgion/blob/main/skills/02-features/05-annotations/references/annotations-metadata.md)
- [annotations-options-and-labels.md](https://github.com/Pandaala/Edgion/blob/main/skills/02-features/05-annotations/references/annotations-options-and-labels.md)
- [annotations-system-and-test-keys.md](https://github.com/Pandaala/Edgion/blob/main/skills/02-features/05-annotations/references/annotations-system-and-test-keys.md)

## Start With Placement, Not Just The Key Name

The same `edgion.io/*` prefix appears in different config locations:

| Location | Example | Meaning |
|----------|---------|---------|
| `metadata.annotations` | `Gateway.metadata.annotations["edgion.io/enable-http2"]` | The most common extension entry point |
| `listener.tls.options` | `listener.tls.options["edgion.io/cert-provider"]` | Listener TLS extension, not an annotation |
| `BackendTLSPolicy.spec.options` | `spec.options["edgion.io/client-certificate-ref"]` | Backend TLS extension, not an annotation |
| `metadata.labels` | `edgion.io/leader` | Scheduling / ownership labels, not annotations |

If the placement is wrong, the code path and the documentation usually drift with it.

## High-Frequency `metadata.annotations`

### Gateway

| Key | Effect |
|-----|--------|
| `edgion.io/enable-http2` | Controls HTTP/2 support |
| `edgion.io/http-to-https-redirect` | Enables redirect on non-TLS listeners |
| `edgion.io/https-redirect-port` | Target redirect port |
| `edgion.io/metrics-test-key` | Integration-test metrics correlation key |
| `edgion.io/metrics-test-type` | Integration-test metrics mode |
| `edgion.io/edgion-stream-plugins` | Gateway-level connection-filter entry |
| `edgion.io/inbound-proxy-protocol` | Strict JSON policy for receiving PROXY protocol v1/v2 on selected Gateway listeners. See below. **Inbound only** — unrelated to `edgion.io/proxy-protocol`, which sends a header upstream. |

#### Inbound PROXY protocol

Use one of these exact JSON shapes:

```yaml
metadata:
  annotations:
    edgion.io/inbound-proxy-protocol: >-
      {"source":"inline","listeners":["https"],"config":{"mode":"trustedSources","trustedCidrs":["10.0.0.0/8","192.168.1.5"]}}
```

```json
{"source":"inline","listeners":["tcp-edge"],"config":{"mode":"required"}}
```

```json
{"source":"resourceRef","listeners":["https"],"resourceRef":{"name":"edge-load-balancers","namespace":"networking"}}
```

`listeners` is optional. When omitted, the policy selects every TCP-based listener
(HTTP, HTTPS, TCP, and TLS) and ignores UDP listeners on a mixed Gateway. A present
list must contain 1–64 unique existing TCP-based listener names. Selecting UDP,
using an unknown name, or applying the annotation to a UDP-only Gateway is invalid.

`required` requires every connection to begin with a valid v1 or v2 header. Use it
only when the network boundary guarantees that every reachable peer is a
header-producing load balancer. `trustedSources` parses a header only when the
physical peer matches one of 1–256 `trustedCidrs`; an untrusted peer remains an
ordinary direct connection and cannot forge `RemoteIp` by sending header bytes.
Bare addresses mean `/32` or `/128`. Invalid entries, duplicates after
normalization, and `/0` reject the entire policy; there is no partial or wider
fallback.

The resource reference always targets `edgion.io/EdgionConfigData`. The target
must have `spec.enable: true`, no `spec.active`, and
`spec.data.type: ProxyProtocolTrust`. When EdgionConfigData reference validation
is disabled, the reference is allowed. With validation enabled, a same-namespace
reference is always allowed. A cross-namespace target with
`spec.visibility: Cluster` is allowed without a grant; a cross-namespace target
with `spec.visibility: Namespace` requires an exact Gateway API
`ReferenceGrant` from group `gateway.networking.k8s.io`, kind `Gateway`, to
group `edgion.io`, kind `EdgionConfigData`.

Invalid JSON, policy, selection, reference, or shared-listener configuration is
reported in Gateway conditions and makes Gateway startup fail closed. After
startup, any effective socket-policy change makes readiness return 503, rejects
new TCP connections, and terminates the process non-zero so a supervisor can
restart it with the newest valid snapshot.

The transport peer observed before parsing is `DirectPeerIp`; it is never
rewritten. A header accepted from a trusted peer supplies `RemoteIp`, the client
identity used by later layers. A PROXY protocol header is unauthenticated, so a
trusted endpoint must generate the header itself rather than transparently relay
client-authored bytes.

> In both modes the listener must only be reachable via something that writes
> the header **itself**: a transparent L4 relay forwards the client's first bytes
> verbatim, so a client can author a header and the relay launders it into a
> trusted position.
>
> Note also that a Gateway-level `edgion.io/edgion-stream-plugins` attachment
> runs before the header is parsed, so its IP rules match the load balancer, not
> the client. This listener `ConnectionFilter` path does not cover UDP listeners.

### Route / TLS / Backend

| Key | Resource | Effect |
|-----|----------|--------|
| `edgion.io/edgion-stream-plugins` | `TCPRoute` / `TLSRoute` / `UDPRoute` | Resolves an `EdgionStreamPlugins` reference. UDPRoute runs `spec.plugins` when admitting a new session; it does not use the Gateway listener `ConnectionFilter` |
| `edgion.io/proxy-protocol` | `TLSRoute` | Current implementation recognizes `"v2"` |
| `edgion.io/max-connect-retries` | `TLSRoute` | Max upstream connection attempts; default 2, capped at 5 |
| `edgion.io/tcp-keepalive-time` | `TCPRoute` / `TLSRoute` | Idle seconds before the first TCP keepalive probe on the upstream L4 connection. Presence (positive integer, 1–7200) enables keepalive; absent = disabled (default) |
| `edgion.io/tcp-keepalive-interval` | `TCPRoute` / `TLSRoute` | Seconds between keepalive probes (1–75). Only applied when `tcp-keepalive-time` is set; absent or invalid = 75 (OS default) |
| `edgion.io/tcp-keepalive-probes` | `TCPRoute` / `TLSRoute` | Number of failed probes before the connection is declared dead (1–20). Only applied when `tcp-keepalive-time` is set; absent or invalid = 9 (OS default) |
| `edgion.io/expose-client-cert` | `EdgionTls` | Exposes mTLS client certificate info to the plugin/session layer |

Service-level backend behavior (active health checks, outlier detection,
degrade threshold, load-balancing algorithm) is **not** an annotation — it is
configured via the `EdgionBackendTrafficPolicy` CRD, attached to a Service via
`spec.targetRefs`. See
[`skills/02-features/03-resources/20-edgion-backend-traffic-policy.md`](https://github.com/Pandaala/Edgion/blob/main/skills/02-features/03-resources/20-edgion-backend-traffic-policy.md)
for the full schema, or the user guides:
[Active Health Check](../user-guide/http-route/backends/health-check.md),
[Circuit Breaker](../user-guide/http-route/backends/circuit-breaker.md),
[Load Balancing Algorithms](../user-guide/http-route/lb-algorithms.md).

## Common Keys That Are Not Annotations

| Key | Actual location | Meaning |
|-----|-----------------|---------|
| `edgion.io/cert-provider` | `Gateway.spec.listeners[*].tls.options` | Listener TLS certificate-provider extension |
| `edgion.io/client-certificate-ref` | `BackendTLSPolicy.spec.options` | Upstream mTLS client certificate reference |
| `edgion.io/leader` | `metadata.labels` | K8s HA leader label |
| `edgion.io/managed-by` | `metadata.labels` | Provenance marker on ACME and other system-managed resources; not read by the system |
| `edgion.io/acme-resource` | `metadata.labels` | ACME ownership/resource label |

## Reserved And Test-Only Keys

These keys are often useful during debugging, but should not usually be hand-authored in regular manifests:

| Key | Type | Meaning |
|-----|------|---------|
| `edgion.io/hostname-resolution` | reserved annotation | Diagnostic data written by the controller on `HTTPRoute` / `GRPCRoute` |
| `edgion.io/sync-version` | reserved annotation | Correlates control-plane and data-plane state |
| `edgion.io/skip-load-validation` | test/tooling annotation | Lets the config-load validator skip a YAML file |
| `edgion.io/force-sync` | test annotation | Used by integration scripts to trigger Secret update events |

## Historical Drift To Watch For

- The current prefix is `edgion.io`, not `edgion.com`.
- The current stream-plugin key for Gateway/TCPRoute/TLSRoute/UDPRoute is `edgion.io/edgion-stream-plugins`.
- Older docs and examples previously used `edgion.io/stream-plugins`; do not copy that legacy form into new work.
- `edgion.io/cert-provider` and `edgion.io/client-certificate-ref` look like annotations, but they belong to `options`.

## Related Docs

- [HTTP to HTTPS Redirect](../ops-guide/gateway/http-to-https-redirect.md)
- [Backend Active Health Check](../user-guide/http-route/backends/health-check.md)
- [Stream Plugins User Guide](../user-guide/tcp-route/stream-plugins.md)
- [knowledge-source-map.md](./knowledge-source-map.md)

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
