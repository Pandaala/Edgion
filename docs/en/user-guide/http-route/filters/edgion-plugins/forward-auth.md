# ForwardAuth (HTTPRoute Filter)

> **🔌 Edgion extension**
>  
> ForwardAuth is an external authentication delegation plugin provided by the `EdgionPlugins` CRD and is not part of the standard Gateway API.

ForwardAuth can be used as an HTTPRoute filter or configured globally via `EdgionPlugins`. This page describes how to use it within HTTPRoute; for complete configuration parameters, see the [standalone plugin page](../../../edgion-plugins/forward-auth.md).

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
            name: my-forward-auth-plugin
      backendRefs:
        - name: api-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: my-forward-auth-plugin
  namespace: production
spec:
  requestPlugins:
    - type: ForwardAuth
      config:
        uri: "http://auth-service.production:8080/verify"
```

## Related Documentation

- [ForwardAuth Full Documentation (all configuration parameters)](../../../edgion-plugins/forward-auth.md)
- [Plugin Composition and References](../plugin-composition.md)
- [Filters Overview](../overview.md)
