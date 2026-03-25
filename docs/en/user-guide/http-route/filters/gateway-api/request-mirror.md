# RequestMirror

RequestMirror is a standard Gateway API HTTPRoute filter used to mirror requests to another backend.

In Edgion, this capability commonly appears in two forms:

1. directly as the standard HTTPRoute `RequestMirror` filter
2. as a reusable `EdgionPlugins` `RequestMirror` configuration

If you only want standard Gateway API usage, start here. If you want reusable mirror logic as a plugin resource, continue to the extension page.

## Minimal example

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: request-mirror-route
spec:
  parentRefs:
    - name: public-gateway
      sectionName: http
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /
      filters:
        - type: RequestMirror
          requestMirror:
            backendRef:
              name: mirror-service
              port: 8080
      backendRefs:
        - name: primary-service
          port: 8080
```

## What matters in Edgion

- the primary request and the mirrored request use the same underlying RequestMirror runtime
- mirroring is asynchronous and does not determine primary request success
- if you want reusable configuration, central references, or richer Edgion-specific operational tuning, use the `EdgionPlugins` form instead

## Continue reading

- [Edgion extension Request Mirror plugin](../edgion-plugins/request-mirror.md)
- [Filters Overview](../overview.md)
