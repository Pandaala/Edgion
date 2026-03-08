# Path Matching

HTTPRoute supports three types of path matching.

## Match Types

### Exact - Exact Match

```yaml
matches:
  - path:
      type: Exact
      value: /api/users
```

Only matches `/api/users`, does not match `/api/users/` or `/api/users/123`.

### PathPrefix - Prefix Match

```yaml
matches:
  - path:
      type: PathPrefix
      value: /api
```

Matches all paths starting with `/api`:
- Yes: `/api`
- Yes: `/api/`
- Yes: `/api/users`
- No: `/apiV2`

### RegularExpression - Regex Match

```yaml
matches:
  - path:
      type: RegularExpression
      value: "^/api/v[0-9]+/.*"
```

Uses a regular expression to match paths.

## Configuration Reference

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| type | string | | PathPrefix | Match type |
| value | string | Yes | | Match value |

## Examples

### Example 1: API Version Routing

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: api-versioning
spec:
  parentRefs:
    - name: my-gateway
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /api/v1
      backendRefs:
        - name: api-v1
          port: 8080
    - matches:
        - path:
            type: PathPrefix
            value: /api/v2
      backendRefs:
        - name: api-v2
          port: 8080
```

### Example 2: Static Resources vs API Separation

```yaml
rules:
  - matches:
      - path:
          type: PathPrefix
          value: /static
    backendRefs:
      - name: static-server
        port: 80
  - matches:
      - path:
          type: PathPrefix
          value: /api
    backendRefs:
      - name: api-server
        port: 8080
```

## Related Documentation

- [Header Matching](./headers.md)
- [Query Parameter Matching](./query-params.md)
- [HTTP Method Matching](./method.md)
