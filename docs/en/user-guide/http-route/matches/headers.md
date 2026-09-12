# Header Matching

Header matching routes requests by their HTTP request headers. It supports `Exact` and `RegularExpression` match types.

## Match types

### Exact

```yaml
matches:
  - headers:
      - name: X-Env
        type: Exact
        value: production
```

Matches when the `X-Env` header value is exactly `production`.

### RegularExpression

```yaml
matches:
  - headers:
      - name: X-Request-ID
        type: RegularExpression
        value: "^[a-f0-9-]{36}$"
```

Matches when the header value satisfies the regular expression.

## Configuration reference

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | String | **Required** | Header name. Case-insensitive |
| `type` | Enum | `Exact` | Match type. Allowed values: `Exact` \| `RegularExpression` |
| `value` | String | **Required** | Value to match. Case-sensitive |

## How header conditions combine

- Conditions for distinct names in one `headers` list are **ANDed** — every effective header condition must hold.
- If names repeat, ignoring ASCII case, only the first entry's type and value apply. Later entries are ignored and do not increase routing priority. For example, `X-Mode: one` followed by `x-mode: two` matches `X-Mode: one`; use separate match blocks to accept either value.
- Header conditions are also ANDed with `path`, `queryParams`, and `method` in the same match block.
- Header **names** are case-insensitive (per HTTP); header **values** are matched case-sensitively.

## Examples

### Example 1: Header-based canary routing

Requests carrying `X-Env: canary` go to the canary backend; everything else falls through to the stable backend. Because the canary rule has a more specific match, it is evaluated first:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: env-routing
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
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

### Example 2: Multiple conditions ANDed

The request matches only when the path prefix **and** both header conditions hold:

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

### Example 3: OR across two header values

Two separate match blocks are ORed, so a request matches if either header value is present:

```yaml
matches:
  - headers:
      - name: X-Tier
        value: gold
  - headers:
      - name: X-Tier
        value: platinum
```

## Related documents

- [Path matching](./path.md)
- [Query parameter matching](./query-params.md)
- [HTTP method matching](./method.md)
