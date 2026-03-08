# Header Matching

Route matching based on HTTP request headers.

## Match Types

### Exact - Exact Match

```yaml
matches:
  - headers:
      - name: X-Env
        type: Exact
        value: production
```

### RegularExpression - Regex Match

```yaml
matches:
  - headers:
      - name: X-Request-ID
        type: RegularExpression
        value: "^[a-f0-9-]{36}$"
```

## Configuration Reference

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| name | string | Yes | | Header name |
| type | string | | Exact | Match type |
| value | string | Yes | | Match value |

## Examples

### Example 1: Environment-Based Routing

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: env-routing
spec:
  parentRefs:
    - name: my-gateway
  rules:
    - matches:
        - headers:
            - name: X-Env
              value: canary
      backendRefs:
        - name: app-canary
          port: 8080
    - matches:
        - path:
            type: PathPrefix
            value: /
      backendRefs:
        - name: app-stable
          port: 8080
```

### Example 2: Multi-Condition Combination

```yaml
matches:
  - path:
      type: PathPrefix
      value: /api
    headers:
      - name: X-Auth-Type
        value: jwt
      - name: X-Version
        value: "2"
```

The request matches only when both the path prefix and the two header conditions are satisfied.

## Related Documentation

- [Path Matching](./path.md)
- [Query Parameter Matching](./query-params.md)
