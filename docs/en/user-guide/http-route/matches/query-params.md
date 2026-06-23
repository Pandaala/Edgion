# Query Parameter Matching

Query parameter matching routes requests by their URL query string. It supports `Exact` and `RegularExpression` match types.

## Match types

### Exact

```yaml
matches:
  - queryParams:
      - name: version
        type: Exact
        value: "2"
```

Matches `?version=2`.

### RegularExpression

```yaml
matches:
  - queryParams:
      - name: id
        type: RegularExpression
        value: "^[0-9]+$"
```

Matches when the parameter value satisfies the regular expression.

## Configuration reference

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | String | **Required** | Query parameter name. Case-sensitive |
| `type` | Enum | `Exact` | Match type. Allowed values: `Exact` \| `RegularExpression` |
| `value` | String | **Required** | Value to match. Case-sensitive |

## How query conditions combine

- Multiple entries in one `queryParams` list are **ANDed** — every condition must hold.
- Query conditions are ANDed with `path`, `headers`, and `method` in the same match block.
- Both parameter names and values are matched case-sensitively.

## Examples

### Example 1: Route by API version parameter

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: api-version-param
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  rules:
    - matches:
        - queryParams:
            - name: api_version
              value: "v2"
      backendRefs:
        - name: api-v2-service
          port: 8080
    - backendRefs:
        - name: api-v1-service
          port: 8080
```

### Example 2: Multiple parameters ANDed

The request matches only when both `region=eu` and `tier=premium` are present:

```yaml
matches:
  - queryParams:
      - name: region
        value: eu
      - name: tier
        value: premium
```

## Related documents

- [Path matching](./path.md)
- [Header matching](./headers.md)
- [HTTP method matching](./method.md)
