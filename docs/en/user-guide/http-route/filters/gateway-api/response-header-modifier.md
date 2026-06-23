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

The three operations are applied in order: `set`, then `add`, then `remove`.

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

`add` appends without removing existing values, so the response below carries both `Vary` values from the upstream and the gateway.

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
                value: Accept-Encoding
              - name: Vary
                value: Origin
      backendRefs:
        - name: backend-service
          port: 8080
```

## Notes

- Header names are case-insensitive.
- Values are treated as literals; no variable expansion is performed.
- This filter runs in the response stage; to modify request headers use [RequestHeaderModifier](./request-header-modifier.md).
- For status-code rewriting or richer response transformations, see the [ResponseRewrite plugin](../edgion-plugins/response-rewrite.md).

## Related documents

- [RequestHeaderModifier](./request-header-modifier.md)
- [ResponseRewrite plugin](../edgion-plugins/response-rewrite.md)
- [Plugin Composition and Execution Order](../plugin-composition.md)
