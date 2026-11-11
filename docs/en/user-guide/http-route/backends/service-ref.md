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
- Its `edgion.io/dns-resolution-mode` annotation selects how the hostname is resolved (see [ExternalName resolution](#externalname-resolution)); without it the EdgionGatewayConfig `dnsResolver.endpointTable` default applies.
- The selected Service generation supplies its hostname, port and protocol throughout one attempt; later attempts use the current generation. BackendTLSPolicy absence remains plaintext even on port 443.
- Every mode works for HTTP, gRPC, TCP, UDP and TLS routes.

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

## ExternalName resolution

| `edgion.io/dns-resolution-mode` | Behavior |
|---|---|
| `gateway` (default) | The Gateway resolves the hostname in the background and turns every returned address into an ordinary endpoint, so load balancing, health checks, outlier ejection and session persistence apply. Requests never wait for DNS except, once per request or connection, for at most 2 seconds while the hostname has no answer yet (the Service just appeared, its hostname or resolver changed, or the Gateway just started). |
| `system` | The Gateway asks the operating-system resolver on every request (or connection / UDP session) and dials one address, rotating the start across the answer and preferring an address a retry has not tried. No load-balancing pool or health state. For UDP the lookup goes through a short-lived shared cache so the listener never waits more than 250 ms. |
| `controller` | The Controller resolves the hostname on a fixed interval and publishes the endpoints; the Gateway uses only those. Works in Kubernetes, file-system and etcd deployments. The endpoints live in Controller memory, so after a Controller restart they return with the first answer. |

Without the annotation, `EdgionGatewayConfig.spec.dnsResolver.endpointTable`
decides: `true` (default) means `gateway`, `false` means `system`. An unknown value
fails the backend instead of picking a mode. No mode falls back to another.

In `gateway` mode the refresh interval follows the record TTL (`ttl x 0.5`,
clamped to 5–300 seconds by default; see the
[EdgionGatewayConfig DNS settings](../../../ops-guide/configuration/edgion-gateway-config.md#backend-dns-endpoint-table)).
An unchanged answer changes nothing, so health and outlier state survive refreshes.
If the resolver fails, the last good addresses are kept for up to 5 minutes; a
name that no longer exists (authoritative empty answer) is withdrawn at once. The
operating-system resolver cannot tell a missing name from an outage, so with it a
removed name is kept for those 5 minutes too. Link-local and unspecified answers
such as `169.254.169.254` or `0.0.0.0` are never used. The Gateway drops the
endpoints of a deleted Service, or one whose hostname changed, immediately; a
resolver or address-family change keeps serving the previous answer, restricted to
the new family, until the new resolver's first result (a failing new resolver then
withdraws the endpoints); when the previous answer has no address of the new family,
requests wait for the first answer instead.

The resolver is chosen by the Service annotations `edgion.io/dns-resolver` or
`edgion.io/dns-resolver-ref`, then the EdgionGatewayConfig default (`gateway`
mode) or the Controller default (`controller` mode), then the operating-system
resolver. `system` mode always uses the operating-system resolver. Gateway
resource annotations do not apply to ExternalName Services.

```yaml
apiVersion: v1
kind: Service
metadata:
  name: payments
  annotations:
    edgion.io/dns-resolution-mode: gateway   # default; may be omitted
spec:
  type: ExternalName
  externalName: api.payments.example.com
  ports:
    - port: 443
```

## DNS address families

DNS-derived backend addresses default to `ipv4Only`. Set `ipv6Only` for IPv6 only
or `all` for both families. These are the only accepted values; an empty or unknown
annotation fails DNS dispatch rather than inheriting or widening the policy.
`all` does not enable Happy Eyeballs or connection racing. IPv6 requires a working
upstream IPv6 network.

The ExternalName Service annotation `edgion.io/dns-lookup-family` wins. Otherwise
`gateway` and `system` modes use `EdgionGatewayConfig.spec.dnsResolver.lookupFamily`,
and `controller` mode uses Controller `dns_resolver.lookup_family` (read at
Controller startup); the final default is `ipv4Only`. AI external hostnames use
the Gateway annotation, then the EdgionGatewayConfig default. Resolver selection
and family inheritance are independent: setting only the family does not replace a
chosen DNS server or HTTPDNS provider. Ordinary Service slices, literal non-DNS
backends, listeners, DNS-server transport and HTTPDNS bootstrap are outside this
policy.

```yaml
# ExternalName Service metadata
metadata:
  annotations:
    edgion.io/dns-lookup-family: ipv6Only
```

With explicit Standard DNS servers, an empty address family requires a NOERROR
negative response carrying an SOA. Ambiguous empty responses and unfinished CNAME
chains fail conservatively rather than removing that family's existing endpoints.
