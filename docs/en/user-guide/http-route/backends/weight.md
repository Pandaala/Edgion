# Weight Configuration

Use weights to distribute traffic, supporting scenarios such as canary releases and blue-green deployments.

## Basic Configuration

```yaml
backendRefs:
  - name: app-v1
    port: 8080
    weight: 90
  - name: app-v2
    port: 8080
    weight: 10
```

90% of traffic goes to v1, 10% goes to v2.

## Configuration Reference

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `weight` | Integer | `1` | Relative traffic share for this backend within the rule |

## How weights are distributed

Weights are **relative**, not percentages. Each backend receives `weight / (sum of weights)` of the rule's traffic. The values `90`/`10` and `9`/`1` distribute traffic identically.

## Default Behavior

- When `weight` is omitted, it defaults to `1`. With multiple backends and no explicit weights, traffic is split evenly.
- A backend with `weight: 0` receives no traffic but stays in the configuration — handy for parking the idle side of a blue-green pair.

## All-or-none weight rule

Within a single rule, **either every `backendRef` sets a `weight`, or none of them do**. Mixing weighted and unweighted backends in the same rule is rejected and the rule's backends fail to resolve.

```yaml
# ❌ Invalid: one backend has a weight, the other does not
backendRefs:
  - name: app-v1
    port: 8080
    weight: 90
  - name: app-v2
    port: 8080

# ✅ Valid: both backends set a weight
backendRefs:
  - name: app-v1
    port: 8080
    weight: 90
  - name: app-v2
    port: 8080
    weight: 10
```

## Examples

### Example 1: Canary Release

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: canary-release
spec:
  parentRefs:
    - name: main-gateway
  rules:
    - backendRefs:
        - name: app-stable
          port: 8080
          weight: 95
        - name: app-canary
          port: 8080
          weight: 5
```

### Example 2: Blue-Green Deployment

```yaml
# Blue environment 100%
backendRefs:
  - name: app-blue
    port: 8080
    weight: 100
  - name: app-green
    port: 8080
    weight: 0

# After switching: Green environment 100%
backendRefs:
  - name: app-blue
    port: 8080
    weight: 0
  - name: app-green
    port: 8080
    weight: 100
```

> **Remotely switchable, without a redeploy?** The weight edit above still requires editing and reapplying the route. If you want to flip a full cutover at runtime (e.g. via a `kubectl apply` to a small config object instead of the route itself), use the [Canary plugin](../filters/edgion-plugins/canary.md) with an `activeProfileRef` Selector instead.

### Example 3: A/B Testing

```yaml
rules:
  # Requests with specific header → Version B
  - matches:
      - headers:
          - name: X-Test-Group
            value: B
    backendRefs:
      - name: app-b
        port: 8080
  # Other requests → Version A
  - backendRefs:
      - name: app-a
        port: 8080
```

## Notes

- If every backend in a rule has `weight: 0`, no backend is selectable and requests to that rule return a 5xx. Always keep at least one backend with a non-zero weight.
- Weights control how the rule's traffic is shared; they do not change which backend a specific in-flight connection retries to. Health-checked or circuit-broken endpoints are still skipped regardless of weight.

## Related Documentation

- [Service Reference](./service-ref.md)
- [Backend TLS](./backend-tls.md)
- [Backend Active Health Check](./health-check.md)
- [Circuit Breaker](./circuit-breaker.md)
