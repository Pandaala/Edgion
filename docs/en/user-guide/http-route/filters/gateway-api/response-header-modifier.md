# ResponseHeaderModifier

## Overview

`ResponseHeaderModifier` modifies the HTTP response headers before the response is returned to the client. It is a standard Gateway API filter and runs in the response stage, after the upstream response headers arrive. Use it to add security headers, strip headers that leak backend details, or stamp the response with gateway metadata.

## Configuration

```yaml
filters:
  - type: ResponseHeaderModifier
    responseHeaderModifier:
      set:
        - name: X-Powered-By
          value: Edgion
      add:
        - name: Cache-Control
          value: no-store
      remove:
        - Server
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `set` | Array\<Object\> | none | Each entry `{name, value}` replaces the header, overwriting any existing value(s) |
| `add` | Array\<Object\> | none | Each entry `{name, value}` appends a value, keeping existing values |
| `remove` | Array\<String\> | none | Header names to delete from the response |

The three operations are applied in order: `set`, then `add`, then `remove`. Gateway API configuration allows only one action for each case-insensitive header name, so equivalent names cannot be repeated within a list or used across multiple lists.

## Examples

### Example 1: Add security headers and hide the backend

Add a strict transport security header and remove the upstream `Server` banner.

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
        - type: ResponseHeaderModifier
          responseHeaderModifier:
            set:
              - name: Strict-Transport-Security
                value: "max-age=31536000"
            remove:
              - Server
      backendRefs:
        - name: api-service
          port: 8080
```

### Example 2: Multi-value headers with add

`add` appends without removing existing values. If the upstream response contains `Vary: Accept-Encoding`, the example below returns both `Vary: Accept-Encoding` and the gateway-added `Vary: Origin` to the client.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: vary-route
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
        - type: ResponseHeaderModifier
          responseHeaderModifier:
            add:
              - name: Vary
                value: Origin
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
- Values are treated as literals; no variable expansion is performed.
- This filter runs in the response stage; to modify request headers use [RequestHeaderModifier](./request-header-modifier.md).
- For status-code rewriting or richer response transformations, see the [ResponseRewrite plugin](../edgion-plugins/response-rewrite.md).

## Related documents

- [RequestHeaderModifier](./request-header-modifier.md)
- [ResponseRewrite plugin](../edgion-plugins/response-rewrite.md)
- [Plugin Composition and Execution Order](../plugin-composition.md)
