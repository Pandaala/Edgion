# CtxSet (HTTPRoute Filter)

> **🔌 Edgion Extension**
>  
> CtxSet is a context variable extraction plugin provided by the `EdgionPlugins` CRD and is not part of the standard Gateway API.

CtxSet can be used as an HTTPRoute filter or configured globally via `EdgionPlugins`. This page describes how to use it within HTTPRoute; for complete configuration parameters, see the [standalone plugin page](../../../edgion-plugins/ctx-setter.md).

## Using in HTTPRoute

### Minimal Configuration

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: example-route
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
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: my-ctx-setter-plugin
      backendRefs:
        - name: api-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: my-ctx-setter-plugin
  namespace: production
spec:
  requestPlugins:
    - type: CtxSet
      config:
        rules:
          - name: userId
            from:
              type: header
              name: "X-User-Id"
```

## Related Documentation

- [CtxSet Full Documentation (all configuration parameters)](../../../edgion-plugins/ctx-setter.md)
- [Plugin Composition and References](../plugin-composition.md)
- [Filters Overview](../overview.md)
