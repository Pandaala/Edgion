# Query Parameter Matching

Route matching based on URL query parameters.

## Match Types

### Exact - Exact Match

```yaml
matches:
  - queryParams:
      - name: version
        type: Exact
        value: "2"
```

Matches `?version=2`.

### RegularExpression - Regex Match

```yaml
matches:
  - queryParams:
      - name: id
        type: RegularExpression
        value: "^[0-9]+$"
```

## Configuration Reference

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| name | string | Yes | | Parameter name |
| type | string | | Exact | Match type |
| value | string | Yes | | Match value |

## Examples

### Example 1: API Version Selection

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: api-version-param
spec:
  parentRefs:
    - name: my-gateway
  rules:
    - matches:
        - queryParams:
            - name: api_version
              value: "v2"
      backendRefs:
        - name: api-v2
          port: 8080
    - backendRefs:
        - name: api-v1
          port: 8080
```

## Related Documentation

- [Path Matching](./path.md)
- [Header Matching](./headers.md)
