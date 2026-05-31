# UDPRoute Backend Configuration

A UDPRoute forwards UDP datagrams to one or more Kubernetes Services. Datagrams from the same client are pinned to a backend for the duration of the session; load balancing across endpoints is automatic.

## Basic example

```yaml
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: UDPRoute
metadata:
  name: dns-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
      sectionName: dns
  rules:
    - backendRefs:
        - name: dns-service
          port: 53
```

## Backend fields

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `name` | String | **Required** | Target Kubernetes Service name |
| `port` | Integer | **Required** | Service port to send datagrams to |
| `weight` | Integer | `1` | Relative share of sessions sent to this backend |
| `namespace` | String | UDPRoute namespace | Service namespace; cross-namespace requires a `ReferenceGrant` |
| `kind` | String | `Service` | Backend object kind |

## Sessions and timeouts

UDP is connectionless, so the gateway tracks a logical session per client source address and pins it to one backend. An idle session is reclaimed after the inactivity timeout (60 seconds), after which the next datagram from that client may be balanced to a different backend.

## Weighted backends

List multiple `backendRefs` to split sessions across backends by weight (a ratio, not a percentage):

```yaml
spec:
  rules:
    - backendRefs:
        - name: dns-primary
          port: 53
          weight: 2
        - name: dns-secondary
          port: 53
          weight: 1
```

## Related documents

- [UDPRoute Overview](../overview.md)
- [Cross-Namespace Reference](../../../ops-guide/infrastructure/reference-grant.md)
