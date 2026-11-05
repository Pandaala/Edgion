# Service Reference

A `backendRef` points an HTTPRoute rule at a Kubernetes Service. The Gateway resolves that Service to its live endpoints and load-balances requests across them.

## Basic Configuration

```yaml
backendRefs:
  - name: api-service
    port: 8080
```

## Configuration Reference

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `name` | String | **Required** | Service name |
| `port` | Integer | **Required** | Service port (the `port` exposed by the Service, not the Pod `targetPort`) |
| `namespace` | String | Route's namespace | Service namespace; cross-namespace needs a ReferenceGrant |
| `kind` | String | `Service` | Only omitted or exact `Service` is supported for native Service references |
| `group` | String | `""` | API group; `""` for core Services |
| `weight` | Integer | `1` | Relative traffic weight. See [Weight Configuration](./weight.md) |

## How a backendRef resolves to endpoints

1. The `name` (+ `namespace`) identifies the Service.
2. The `port` selects the matching Service port. Edgion maps it to the Pod-facing `targetPort` and the backing EndpointSlices.
3. The Controller watches the Service's EndpointSlices and pushes the ready endpoint addresses to the Gateway over gRPC.
4. The Gateway load-balances requests across those endpoints, skipping any that health checks or the circuit breaker have marked unhealthy.

Endpoint membership comes from the EndpointSlice API (not the legacy single `Endpoints` object), so endpoint sets scale and update incrementally. When Pods are added, removed, or change readiness, the updated EndpointSlice flows through to the Gateway without re-applying the route.

A Service change (such as a port rename) automatically triggers reprocessing of every route that references it, so dependent routes stay consistent.

## Default Behavior

### `kind` field

- When `kind` is not specified, it defaults to `Service`.
- The API group must be omitted or empty; literal `core` and pseudo kinds such as `ServiceExternalName` are unsupported.
- For HTTP/gRPC, an actual Service with `spec.type: ExternalName` selects DNS behavior. Keep `kind: Service` (or omit it), and declare a valid Service port.
- Its `edgion.io/dns-resolution-mode` annotation selects controller-provided ready EndpointSlices (default, with DNS fallback), Gateway cached DNS, or per-request DNS. The effective resolver follows Service, Gateway, EdgionGatewayConfig, then system precedence.
- The selected Service generation supplies its hostname, port and protocol throughout one attempt; later attempts use the current generation. BackendTLSPolicy absence remains plaintext even on port 443.
- TCP/UDP/TLS use published EndpointSlices; Gateway self-resolving DNS is not provided for those protocols.

### `namespace` field

- When `namespace` is omitted, the Gateway uses the HTTPRoute's own namespace.
- Referencing a Service in a different namespace requires a ReferenceGrant in the **target** namespace.

### Unresolved backends

If a referenced Service or port cannot be resolved (wrong name, missing ReferenceGrant, or no ready endpoints), the route's `ResolvedRefs` condition reflects the failure and requests to that backend return a 5xx. Fix the reference and the Gateway recovers automatically once endpoints become available.

## Cross-Namespace References

Referencing a Service in another namespace requires a ReferenceGrant created in the **Service's** namespace. Without it, the reference is rejected even if the Service exists:

```yaml
# HTTPRoute in the `production` namespace
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: api-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  rules:
    - backendRefs:
        - name: backend-service
          namespace: staging
          port: 8080

---
# ReferenceGrant created in the target namespace (`staging`)
apiVersion: gateway.networking.k8s.io/v1
kind: ReferenceGrant
metadata:
  name: allow-from-production
  namespace: staging
spec:
  from:
    - group: gateway.networking.k8s.io
      kind: HTTPRoute
      namespace: production
  to:
    - group: ""
      kind: Service
```

## Related Documentation

- [Weight Configuration](./weight.md)
- [Backend TLS](./backend-tls.md)
- [Backend Active Health Check](./health-check.md)
- [Circuit Breaker](./circuit-breaker.md)

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

With explicit Standard DNS servers, an empty address family requires a NOERROR
negative response carrying an SOA. Ambiguous empty responses and unfinished CNAME
chains fail conservatively rather than removing that family's existing endpoints.
