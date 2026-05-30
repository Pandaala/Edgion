# Path Matching

Path matching routes requests by their URL path. HTTPRoute supports three match types: `Exact`, `PathPrefix`, and `RegularExpression`.

## Match types

### Exact

```yaml
matches:
  - path:
      type: Exact
      value: /api/users
```

Matches only `/api/users`. It does not match `/api/users/` or `/api/users/123`.

### PathPrefix

```yaml
matches:
  - path:
      type: PathPrefix
      value: /api
```

Matches paths whose **path segments** start with the prefix:

- Matches: `/api`, `/api/`, `/api/users`
- Does not match: `/apiV2` (the prefix must align on a `/` segment boundary, not a substring)

### RegularExpression

```yaml
matches:
  - path:
      type: RegularExpression
      value: "^/api/v[0-9]+/.*"
```

Matches the path against a regular expression. Use this for patterns that prefix matching cannot express, such as version ranges.

## Configuration reference

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `type` | Enum | `PathPrefix` | Match type. Allowed values: `Exact` \| `PathPrefix` \| `RegularExpression` |
| `value` | String | `/` (for `PathPrefix`) | The path or pattern to match. Case-sensitive |

For `Exact` and `PathPrefix`, the configured value is normalized at load time (percent-encoded slashes decoded, dot-segments and duplicate slashes collapsed) so it matches the normalized request path. `RegularExpression` values are left untouched.

## Named path parameters

> **🔌 Edgion extension**
> 
> The `:name` path-parameter syntax is an Edgion extension to Gateway API path values.

For `Exact` and `PathPrefix` types, `value` may include named path parameters using `:name`:

| Pattern | Behavior |
|---------|----------|
| `/users/:id` | Captures the second segment as `id` |
| `/api/:version/users/:uid` | Captures multiple parameters |
| `/data/::literal` | `::` escapes to a literal `:`; the segment is `:literal`, not a parameter |

Captured values are available at runtime only through:

- The DSL plugin builtin `req.path_param("name")`.
- `$name` substitution in the `ProxyRewrite` plugin (URI and header values).

Standard Gateway API filters (`URLRewrite`, `RequestRedirect`, `RequestHeaderModifier`, `ResponseHeaderModifier`) treat their value strings as literals and do not expand `:name`. `RegularExpression` paths do not participate in `:name` capture — use regex capture groups (`$1`–`$9`) in `ProxyRewrite` instead.

## Precedence

When several rules match the same request, the most specific path wins:

- An `Exact` match is more specific than any prefix.
- A longer `PathPrefix` beats a shorter one (`/api/v1` beats `/api`).

## Examples

### Example 1: API version routing

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: api-versioning
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /api/v1
      backendRefs:
        - name: api-v1-service
          port: 8080
    - matches:
        - path:
            type: PathPrefix
            value: /api/v2
      backendRefs:
        - name: api-v2-service
          port: 8080
```

### Example 2: Specific exact path overriding a prefix

The exact `/api/health` rule wins over the broader `/api` prefix because exact matches are more specific:

```yaml
rules:
  - matches:
      - path:
          type: Exact
          value: /api/health
    backendRefs:
      - name: health-service
        port: 8080
  - matches:
      - path:
          type: PathPrefix
          value: /api
    backendRefs:
      - name: api-service
        port: 8080
```

### Example 3: Regular expression for versioned paths

```yaml
rules:
  - matches:
      - path:
          type: RegularExpression
          value: "^/api/v[0-9]+/users/[0-9]+$"
    backendRefs:
      - name: user-service
        port: 8080
```

## Related documents

- [Header matching](./headers.md)
- [Query parameter matching](./query-params.md)
- [HTTP method matching](./method.md)
