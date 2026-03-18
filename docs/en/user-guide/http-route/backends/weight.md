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

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| weight | int | | 1 | Weight value (0-1000000) |

## Default Behavior

- When `weight` is not specified, the default value is `1`
- A backend with `weight: 0` receives no traffic (useful for blue-green deployment switching)

## Examples

### Example 1: Canary Release

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: canary-release
spec:
  parentRefs:
    - name: my-gateway
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

## Related Documentation

- [Service Reference](./service-ref.md)
- Canary and blue-green rollout patterns currently build on the weight configuration described on this page.
