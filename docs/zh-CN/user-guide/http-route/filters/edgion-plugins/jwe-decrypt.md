# JWE Decrypt（HTTPRoute 过滤器）

> **🔌 Edgion 扩展**
>  
> JweDecrypt 是 `EdgionPlugins` CRD 提供的 JWE Token 解密插件，不属于标准 Gateway API。

JweDecrypt 可以作为 HTTPRoute 的过滤器使用，也可以通过 `EdgionPlugins` 全局配置。本页介绍如何在 HTTPRoute 中使用；完整配置参数请见 [独立插件页](../../edgion-plugins/jwe-decrypt.md)。

## 在 HTTPRoute 中使用

### 最简配置

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

密钥配置及完整参数（`keyManagementAlgorithm`、`contentEncryptionAlgorithm`、`payloadToHeaders` 等）请见独立插件页。

## 相关文档

- [JWE Decrypt 完整文档（包含所有配置参数）](../../edgion-plugins/jwe-decrypt.md)
- [插件组合与引用](../plugin-composition.md)
- [过滤器总览](../overview.md)
