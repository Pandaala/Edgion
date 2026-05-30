# OpenID Connect (HTTPRoute Filter)

> **🔌 Edgion extension**
>  
> OpenidConnect is an OIDC/OAuth2 authentication plugin provided by the `EdgionPlugins` CRD and is not part of the standard Gateway API.

OpenidConnect can be used as an HTTPRoute filter or configured globally via `EdgionPlugins`. This page describes how to use it within HTTPRoute; for complete configuration parameters, see the [standalone plugin page](../../../edgion-plugins/openid-connect.md).

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
            name: my-oidc-plugin
      backendRefs:
        - name: api-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: my-oidc-plugin
  namespace: production
spec:
  requestPlugins:
    - type: OpenidConnect
      config:
        discovery: "https://sso.example.com/.well-known/openid-configuration"
        clientId: "my-app"
```

## Related Documentation

- [OpenID Connect Full Documentation (all configuration parameters)](../../../edgion-plugins/openid-connect.md)
- [Plugin Composition and References](../plugin-composition.md)
- [Filters Overview](../overview.md)
