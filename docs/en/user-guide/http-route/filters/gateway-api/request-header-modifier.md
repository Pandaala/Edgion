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
- Names must be valid HTTP header tokens of 1–256 bytes; values must be valid HTTP header values of 1–4096 characters. CR, LF, NUL, and other invalid control characters are rejected.
- Each of `set`, `add`, and `remove` accepts at most 16 entries. A case-insensitive name cannot appear in more than one action. For equivalent names within one list supplied outside native Kubernetes admission, the first entry is used; every entry still has to pass validation. Operations execute in `set` → `add` → `remove` order.
- Native HTTPRoute and GRPCRoute filters must pair their `type` with exactly the corresponding configuration field. Each request/response modifier type can occur only once in one rule or backend filter list. A rule-level modifier and a backend-level modifier are separate lists.
- Validation also applies to EdgionPlugins and filesystem/etcd configuration. Invalid native modifiers invalidate the owning rule while valid sibling rules remain available; status reports the invalid configuration. If invalid configuration reaches Gateway construction, the affected runtime rejects requests with a generic configuration error instead of silently skipping the modifier.
- Values are treated as literals; path parameters (`:name`) and template variables are **not** expanded here. If you need dynamic substitution, use the [ProxyRewrite plugin](../edgion-plugins/proxy-rewrite.md).
- For AI provider backend filters, use unique names within each list; the first-equivalent fallback is not currently guaranteed.
- To modify response headers instead, use [ResponseHeaderModifier](./response-header-modifier.md).

## Related documents

- [ResponseHeaderModifier](./response-header-modifier.md)
- [URLRewrite](./url-rewrite.md)
- [ProxyRewrite plugin](../edgion-plugins/proxy-rewrite.md)
- [Plugin Composition and Execution Order](../plugin-composition.md)
