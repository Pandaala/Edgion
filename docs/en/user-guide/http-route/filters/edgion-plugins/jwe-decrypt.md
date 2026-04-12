# JWE Decrypt (HTTPRoute Filter)

> **🔌 Edgion Extension**
>  
> JweDecrypt is a JWE Token decryption plugin provided by the `EdgionPlugins` CRD and is not part of the standard Gateway API.

JweDecrypt can be used as an HTTPRoute filter or configured globally via `EdgionPlugins`. This page describes how to use it within HTTPRoute; for complete configuration parameters, see the [standalone plugin page](../../../edgion-plugins/jwe-decrypt.md).

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
            name: my-jwe-decrypt-plugin
      backendRefs:
        - name: api-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: my-jwe-decrypt-plugin
  namespace: production
spec:
  requestPlugins:
    - type: JweDecrypt
      config:
        secretRef:
          name: jwe-key-secret
```

For key configuration and full parameters (`keyManagementAlgorithm`, `contentEncryptionAlgorithm`, `payloadToHeaders`, etc.), see the standalone plugin page.

## Related Documentation

- [JWE Decrypt Full Documentation (all configuration parameters)](../../../edgion-plugins/jwe-decrypt.md)
- [Plugin Composition and References](../plugin-composition.md)
- [Filters Overview](../overview.md)
