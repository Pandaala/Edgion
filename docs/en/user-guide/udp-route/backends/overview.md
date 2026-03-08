# UDPRoute Backend Configuration

UDPRoute forwards traffic to target Services through `backendRefs`.

## Example

```yaml
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: UDPRoute
metadata:
  name: game-route
  namespace: gateway-system
spec:
  parentRefs:
    - name: edge-gw
      sectionName: udp-game
  rules:
    - backendRefs:
        - name: game-udp
          port: 30000
```

## Best Practices

1. Keep backend instances stable to avoid frequent drift.
2. For latency-sensitive workloads, pay attention to network paths and packet loss rates.
