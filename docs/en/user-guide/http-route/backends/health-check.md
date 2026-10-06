# Backend Active Health Check

> **🔌 Edgion extension**
>
> This feature enables active probing for backends via the `EdgionBackendTrafficPolicy` custom resource, and is an Edgion extension capability.

## Overview

Active health checks periodically probe backend instances (HTTP/HTTPS/TCP/gRPC/GRPCS) from the gateway side, and automatically skip unhealthy instances during load balancing selection.
This feature is primarily used when backends are reachable but the application layer is abnormal, or as a supplementary readiness check in non-Kubernetes scenarios.

Health checks are configured via the `spec.healthCheck` section of an `EdgionBackendTrafficPolicy` resource attached to the target `Service` — not a Service annotation. `EdgionBackendTrafficPolicy` also carries [outlier detection](./circuit-breaker.md) and the [load balancing algorithm](../lb-algorithms.md) for the same Service; see the [full schema reference](https://github.com/Pandaala/Edgion/blob/main/skills/02-features/03-resources/20-edgion-backend-traffic-policy.md) for all three sections together.

## Quick Start

```yaml
apiVersion: edgion.io/v1
kind: EdgionBackendTrafficPolicy
metadata:
  name: my-backend-policy
  namespace: production
spec:
  targetRefs:
    - kind: Service
      name: my-backend
  healthCheck:
    active:
      type: http
      path: /healthz
      interval: 10s
      timeout: 3s
      healthyThreshold: 2
      unhealthyThreshold: 3
      expectedStatuses:
        - 200
---
apiVersion: v1
kind: Service
metadata:
  name: my-backend
  namespace: production
spec:
  ports:
    - port: 8080
      targetPort: 8080
```

The policy must live in the **same namespace** as the target Service (`spec.targetRefs[].kind: Service`, core API group only in v1).

## Configuration Parameters

`spec.healthCheck`:

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `active` | Object | none | Active probe configuration; health check is disabled if not configured |

`spec.healthCheck.active` sub-fields:

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `type` | Enum | `http` | Probe type. Allowed values: `http` / `https` / `tcp` / `grpc` / `grpcs` |
| `path` | String | `/` | HTTP probe path (effective for `http` and `https`) |
| `port` | Integer | uses backend port | Probe port override, from `1` to `65535`; omitted or `null` inherits the backend port |
| `interval` | Duration | `10s` | Probe interval |
| `timeout` | Duration | `3s` | Single probe timeout |
| `healthyThreshold` | Integer | `2` | Number of consecutive successes to recover healthy status |
| `unhealthyThreshold` | Integer | `3` | Number of consecutive failures to mark unhealthy |
| `expectedStatuses` | Array\<Integer\> | `[200]` | Expected HTTP status codes (effective for `http` and `https`) |
| `host` | String | none | HTTP Host header override (effective for `http` and `https`) |
| `grpcServiceName` | String | `""` (whole server) | gRPC Health Check service name (effective for `grpc` and `grpcs`) |
| `tls` | Object | none | Required local TLS configuration for `https` and `grpcs` |

## Encrypted Health Checks

Select `https` for an HTTP GET over TLS or `grpcs` for the gRPC Health Check
Protocol over TLS. Both require their own `tls` block. `http`, `tcp`, and `grpc`
retain their plaintext/connect-only behavior. Attaching or changing a
`BackendTLSPolicy` does not change probe security or reload probe clients.

Create the trust and optional client identity Secrets in the policy namespace:

```bash
kubectl -n production create secret generic health-ca --from-file=ca.crt=health-ca.pem
kubectl -n production create secret tls health-client --cert=client.pem --key=client-key.pem
```

For an existing `my-backend` Service with a health listener on port 8443:

```yaml
apiVersion: edgion.io/v1
kind: EdgionBackendTrafficPolicy
metadata:
  name: encrypted-health
  namespace: production
spec:
  targetRefs:
    - kind: Service
      name: my-backend
  healthCheck:
    active:
      type: https
      port: 8443
      path: /healthz
      interval: 10s
      timeout: 3s
      tls:
        validation:
          hostname: health.internal.example.com
          caCertificateRefs:
            - kind: Secret
              name: health-ca
        clientCertificateRef:
          name: health-client
```

For GRPCS, change `type` to `grpcs`, set the health listener's `port`, and optionally
set `grpcServiceName`. `path`, `host`, and `expectedStatuses` apply only to HTTP(S).
Remove `clientCertificateRef` when the health server does not require mTLS.

| TLS parameter | Type | Default | Description |
|---------------|------|---------|-------------|
| `validation.hostname` | String | required | DNS name for certificate verification and SNI; must match the health server certificate |
| `validation.caCertificateRefs` | Array | none | Same-namespace Secret or ConfigMap references containing `ca.crt`; custom roots replace public roots |
| `validation.wellKnownCACertificates` | Enum | `System` when no CA refs | Built-in public roots; cannot be combined with custom CA refs |
| `validation.subjectAltNames` | Array | none | Additional SAN constraints after normal certificate verification |
| `clientCertificateRef` | Object | none | Same-namespace client Secret containing `tls.crt` and `tls.key` |
| `verify` | Boolean | `true` | Verification is mandatory; `false` is invalid |

The TLS hostname is verified while the connection is pinned to the selected
backend endpoint. An HTTP `host` override changes only the HTTP authority.
Global `outboundTls` defaults do not supply trust or credentials for health checks.

Updates to the TLS configuration or referenced Secret/ConfigMap replace the
probe generation automatically. Missing or malformed TLS material causes failed
probes; there is no plaintext retry. The failed check remains configured and
uses the normal unhealthy threshold. Ancestor status reports configuration or
reference failures. Requests may still use unhealthy endpoints if the Service's
panic/degrade policy allows it; disable degradation when strict exclusion is needed.

## Behavior Details

### Attachment Is Per-Service

Health check configuration attaches at the Service level only — there is no
EndpointSlice- or Endpoints-level override tier anymore. If more than one
`EdgionBackendTrafficPolicy` targets the same Service, the **oldest**
`creationTimestamp` wins (ties break by resource name); the losing policy's
status reports `Accepted=False`/reason `Conflicted` on the affected ancestor
Gateway(s) in `status.ancestors[]`. See
[Conflict Resolution](https://github.com/Pandaala/Edgion/blob/main/skills/02-features/03-resources/20-edgion-backend-traffic-policy.md#conflict-resolution)
for the full rule and how the winner is promoted when deleted.

### Runtime Minimum Values (Safety Floor)

Even if smaller values are configured, runtime will enforce safety floors:

- `interval` is executed at a minimum of `1s`
- `timeout` is executed at a minimum of `100ms`

### How Health Status Takes Effect

Edgion does not directly remove backends from the LB pool. Instead, it filters out unhealthy instances during the `select_with()` selection phase.
Therefore:

- Health status changes take effect immediately on routing
- The backend list is still maintained by the `EndpointSlice/Endpoints` data source

## Scenario Examples

### Scenario 1: Standard K8s HTTP Service Health Check (Recommended)

Configure HTTP probing to cover all backends of a Service.

```yaml
apiVersion: edgion.io/v1
kind: EdgionBackendTrafficPolicy
metadata:
  name: web-policy
  namespace: production
spec:
  targetRefs:
    - kind: Service
      name: web-service
  healthCheck:
    active:
      type: http
      path: /healthz
      interval: 10s
      timeout: 3s
      healthyThreshold: 2
      unhealthyThreshold: 3
      expectedStatuses: [200, 204]
```

### Scenario 2: TCP Health Check

```yaml
apiVersion: edgion.io/v1
kind: EdgionBackendTrafficPolicy
metadata:
  name: legacy-backend-policy
  namespace: production
spec:
  targetRefs:
    - kind: Service
      name: legacy-backend
  healthCheck:
    active:
      type: tcp
      port: 9000
      interval: 5s
      timeout: 1s
```

### Scenario 3: gRPC Backend Health Check

For gRPC backends that implement the standard [gRPC Health Checking Protocol](https://github.com/grpc/grpc/blob/master/doc/health-checking.md) (`grpc.health.v1.Health`):

```yaml
apiVersion: edgion.io/v1
kind: EdgionBackendTrafficPolicy
metadata:
  name: grpc-backend-policy
  namespace: production
spec:
  targetRefs:
    - kind: Service
      name: grpc-backend
  healthCheck:
    active:
      type: grpc
      port: 9090
      interval: 10s
      timeout: 3s
      healthyThreshold: 2
      unhealthyThreshold: 3
      grpcServiceName: "my.package.MyService"  # optional, empty = check whole server
```

The gateway calls the `Health/Check` RPC and marks the backend as healthy only when the response status is `SERVING`.

## Notes

1. `expectedStatuses` only applies to `http` mode; `tcp` and `grpc` modes ignore this field. For `http`, the list must be non-empty and every entry must be a valid HTTP status code (100–599).
2. `path` applies to `http` mode only and must not be empty; an empty `path` makes the whole `EdgionBackendTrafficPolicy` invalid (`Accepted=False`).
3. `healthyThreshold` and `unhealthyThreshold` must be greater than or equal to `1`.
4. `interval` and `timeout` must parse to a non-zero duration (for example `10s`, `500ms`); otherwise the resource is rejected (`Accepted=False`), following the lenient-admission pattern — it still syncs, and the gateway skips the invalid spec.
5. Services without a health check section are not probed and participate in load balancing as "healthy" by default.
6. `grpcServiceName` only applies to `grpc` mode. An empty string (default) checks the overall server health.
7. An explicit probe `port` must be from `1` to `65535`. Direct file or etcd input with `port: 0` makes the whole policy invalid and ineligible for Gateway use. When its Service targets resolve to an ancestor Gateway, the Controller reports `Accepted=False/Invalid`. Omitting `port` or setting it to `null` preserves backend-port inheritance.

## Current Limitations

1. **Active probing only**
   - What: Passive health checks (automatic degradation based on request failures) live in a separate mechanism — see [Circuit Breaker (Outlier Detection)](./circuit-breaker.md).
   - Workaround: Use active probing to shorten failure detection time, and combine with outlier detection for request-driven ejection.
   - Tracking: N/A — this is by design.

2. **HTTP probing supports plaintext HTTP only**
   - What: There is currently no dedicated HTTPS probe configuration option
   - Workaround: Use TCP probing or expose a plaintext HTTP health endpoint on the internal network
   - Tracking: Planned

## Troubleshooting

### Issue 1: Policy is configured but does not seem to take effect

Cause: the `EdgionBackendTrafficPolicy` spec failed validation (an ancestor's `Accepted=False/Invalid`), or another policy targeting the same Service won oldest-wins arbitration (`Accepted=False/Conflicted`), or `targetRefs` does not resolve to an existing Service yet (`ResolvedRefs=False`).
Solution: `kubectl get edgionbackendtrafficpolicy <name> -n <namespace> -o yaml` and check `status.ancestors[].conditions` (one entry per ancestor Gateway — see [EdgionBackendTrafficPolicy status schema](https://github.com/Pandaala/Edgion/blob/main/skills/02-features/03-resources/20-edgion-backend-traffic-policy.md#status-schema)).

### Issue 2: Probing too frequently causes increased backend pressure

Cause: `interval` is configured too low.
Solution: Increase `interval`; for production environments, start from `5s~30s`.

### Issue 3: Two policies target the same Service and only one seems active

Cause: this is expected — whole-section, oldest-wins arbitration; only the oldest `EdgionBackendTrafficPolicy` targeting a Service is in effect for that Service at a time (per concern, atomically). Check for `status.ancestors[].conditions[].type: Accepted, reason: Conflicted` on the losing policy's affected ancestor(s).
Solution: consolidate into a single policy per Service, or delete the older one if it should no longer apply (the next-oldest is promoted automatically).

## Related Documentation

- [Service Reference](./service-ref.md)
- [Weight Configuration](./weight.md)
- [Backend TLS](./backend-tls.md)
- [Circuit Breaker (Outlier Detection)](./circuit-breaker.md)
- [Load Balancing Algorithms](../lb-algorithms.md)
- [Timeout Configuration](../resilience/timeouts.md)
