---
name: edgion-backend-traffic-policy-features
description: Canonical EdgionBackendTrafficPolicy schema for direct Service and AI EdgionBackend targets.
---

# EdgionBackendTrafficPolicy

EdgionBackendTrafficPolicy configures direct backend resilience. A policy targets either core
Services or AI EdgionBackends in its own Namespace. It does not target HTTPRoute and does not
change the route's backend list, order, or weights.

## Target contract

```yaml
targetRefs:
  - group: edgion.io
    kind: EdgionBackend
    name: primary-provider
```

All targets in one policy must have the same kind. EdgionBackend policies accept 1–16 unique
targets; Service policies retain their existing fan-out behavior.

## Section applicability

| Section | Core Service | AI EdgionBackend |
|---------|--------------|------------------|
| `loadBalancer` | Supported | Rejected |
| `healthCheck` | Supported | Rejected |
| `outlierDetection` | Supported | Supported |
| `retryConstraint` | Supported | Supported |
| `circuitBreaker` | Supported | Supported |
| `upstreamAuthority` | Supported | Rejected |
| `connection` | Supported | Supported |

Unsupported sections reject the whole policy.

## Active health-check transport

`healthCheck.active.type` is the complete selector for the probe transport. The
values are `http`, `https`, `tcp`, `grpc`, and `grpcs`: HTTP and gRPC are plaintext,
HTTPS and GRPCS use verified TLS, and TCP checks only whether the socket can be connected. Probe `port` may differ
from the application port.

Active checks do not discover, inherit, or react to `BackendTLSPolicy`. In
particular, attaching a `BackendTLSPolicy` to the same Service does not convert
an HTTP probe to HTTPS, add SNI or CA validation, supply a client certificate,
or respawn the probe task. This separation lets operators use a simple
plaintext or TCP health endpoint for an otherwise TLS-protected application
backend and keeps each configuration's behavior locally understandable.

Encrypted probes require `active.tls`, reusing the shared `OutboundTlsLocal`
shape. `tls.validation.hostname` is an explicit DNS name used for SNI and
certificate verification; the actual dial address remains the selected endpoint.
The HTTP `host` override does not change TLS verification. Numeric hostname
spellings are rejected. `verify: false` is rejected; omitted verification is true.

| TLS field | Contract |
|-----------|----------|
| `validation.hostname` | Required DNS verification name and SNI |
| `validation.caCertificateRefs` | Same-namespace Secret or ConfigMap refs containing `ca.crt`; all must resolve |
| `validation.wellKnownCACertificates` | `System`; mutually exclusive with custom CA refs |
| `validation.subjectAltNames` | Optional additional SAN constraints, enforced after normal chain/hostname verification |
| `clientCertificateRef` | Optional same-namespace Secret containing `tls.crt` and `tls.key` |

Custom CA bundles replace public roots. Without a custom CA source, built-in
public roots apply. Global `outboundTls` defaults are not inherited. Resolved
material is Controller-owned, omitted from CRDs and redacted in admin output.

TLS configuration or Secret/ConfigMap changes replace the probe generation.
Invalid TLS or missing material keeps the winning encrypted check active and
produces probe failures; it does not disable checking or promote another policy.
The Controller reports configuration/reference failures through ancestor status.
Handshake errors use the configured unhealthy threshold. Existing panic/degrade
selection remains independent; operators needing strict unhealthy exclusion
should disable degradation for the Service.

See the [architecture contract](../../01-architecture/02-gateway/07-backends.md#transport-ownership)
and [user examples](../../../docs/en/user-guide/http-route/backends/health-check.md).

For AI traffic, passive outlier state is isolated by typed backend and credential alias.
`circuitBreaker.maxParallelRequests` is an aggregate limit for the direct AI backend.
`connection.connectTimeout` bounds provider connection establishment.
`retryConstraint` is an anti-storm budget; HTTPRoute retry configuration still decides whether a
request is eligible to retry.

## Example

```yaml
apiVersion: edgion.io/v1
kind: EdgionBackendTrafficPolicy
metadata:
  name: provider-resilience
  namespace: production
spec:
  targetRefs:
    - group: edgion.io
      kind: EdgionBackend
      name: primary-provider
  connection:
    connectTimeout: 2s
  retryConstraint:
    budget:
      percent: 20
      interval: 10s
  circuitBreaker:
    maxParallelRequests: 200
  outlierDetection:
    consecutiveErrors: 5
    consecutiveGatewayErrors: 3
    consecutiveLocalOriginFailures: 3
    ejectionTime: 30s
    maxEjectionTime: 5m
```

Oldest creation timestamp wins per typed target; policy name breaks ties. Status reports
`Accepted` and `ResolvedRefs` per Gateway ancestor.
