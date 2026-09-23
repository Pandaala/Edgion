# ForwardAuth (HTTPRoute Filter)

> **Edgion extension**
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
        target:
          name: auth-service
          namespace: production
          port: 8080
        request:
          path: { template: "/verify" }
        decision:
          upstreamHeaders: [X-User-ID]
```

## Related Documentation

- [ForwardAuth Full Documentation (all configuration parameters)](../../../edgion-plugins/forward-auth.md)
- [Plugin Composition and References](../plugin-composition.md)
- [Filters Overview](../overview.md)

## Response header policy

`decision.clientHeaders` rejects protected or invalid names during configuration validation. Denial responses filter all fields nominated by every received `Connection` value, including automatically copied `Content-Type`. A non-text `Connection` value suppresses remote header copying, while the bounded denial body is still relayed. Allowed repeated fields, including `Set-Cookie` and `WWW-Authenticate`, retain their values.

[Response header policy](../gateway-api/response-header-modifier.md#response-header-policy).
