# RequestHeaderModifier

## Overview

`RequestHeaderModifier` modifies the HTTP request headers before the request is forwarded to the upstream. It is a standard Gateway API filter and runs in the request stage. Use it to inject routing metadata, strip internal headers a client should not control, or normalize header values.

## Configuration

```yaml
filters:
  - type: RequestHeaderModifier
    requestHeaderModifier:
      set:
        - name: X-Gateway
          value: edgion
      add:
        - name: X-Forwarded-Env
          value: production
      remove:
        - X-Internal-Token
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `set` | Array\<Object\> | none | Each entry `{name, value}` replaces the header, overwriting any existing value(s) |
| `add` | Array\<Object\> | none | Each entry `{name, value}` appends a value, keeping existing values |
| `remove` | Array\<String\> | none | Header names to delete from the request |

The three operations are applied in order: `set`, then `add`, then `remove`. Gateway API configuration allows only one action for each case-insensitive header name, so equivalent names cannot be repeated within a list or used across multiple lists.

## Examples

### Example 1: Inject and strip headers

Stamp every request with a gateway identifier and remove a header the client should never set itself.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: api-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /api/
      filters:
        - type: RequestHeaderModifier
          requestHeaderModifier:
            set:
              - name: X-Gateway
                value: edgion
            remove:
              - X-Internal-Token
      backendRefs:
        - name: api-service
          port: 8080
```

### Example 2: Append to an existing multi-value header

`set` replaces a header entirely, while `add` preserves values already present on the incoming request. If the client sends `X-Trace: client`, the example below forwards both `X-Trace: client` and `X-Trace: edge` to the upstream. It also replaces any incoming `Accept-Encoding` values with the single value `gzip`.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: header-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /
      filters:
        - type: RequestHeaderModifier
          requestHeaderModifier:
            set:
              - name: Accept-Encoding
                value: gzip
            add:
              - name: X-Trace
                value: edge
      backendRefs:
        - name: backend-service
          port: 8080
```

## Notes

- Header names are case-insensitive.
- Supported Gateway API configuration assigns each case-insensitive header name to exactly one action and contains no equivalent duplicate names.
- **Defensive behavior for invalid input:** if a non-admission source supplies duplicates, the Gateway keeps only the first case-insensitive equivalent entry within each of `set`, `add`, and `remove`, then applies the lists in `set` → `add` → `remove` order. This fallback does not make duplicate or cross-action names supported configuration.
- Values are treated as literals; path parameters (`:name`) and template variables are **not** expanded here. If you need dynamic substitution, use the [ProxyRewrite plugin](../edgion-plugins/proxy-rewrite.md).
- To modify response headers instead, use [ResponseHeaderModifier](./response-header-modifier.md).

## Related documents

- [ResponseHeaderModifier](./response-header-modifier.md)
- [URLRewrite](./url-rewrite.md)
- [ProxyRewrite plugin](../edgion-plugins/proxy-rewrite.md)
- [Plugin Composition and Execution Order](../plugin-composition.md)
