# RealIp（HTTPRoute 过滤器）

> **🔌 Edgion 扩展**
>  
> RealIp 是 `EdgionPlugins` CRD 提供的客户端真实 IP 提取插件，不属于标准 Gateway API。

RealIp 可以作为 HTTPRoute 的过滤器使用，也可以通过 `EdgionPlugins` 全局配置。本页介绍如何在 HTTPRoute 中使用；完整配置参数请见 [独立插件页](../../../edgion-plugins/real-ip.md)。

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
            name: my-real-ip-plugin
      backendRefs:
        - name: api-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: my-real-ip-plugin
  namespace: production
spec:
  requestPlugins:
    - type: RealIp
      config:
        trustedIps:
          - name: private-ranges
            cidrs:
              - "10.0.0.0/8"
```

## 相关文档

- [RealIp 完整文档（包含所有配置参数）](../../../edgion-plugins/real-ip.md)
- [插件组合与引用](../plugin-composition.md)
- [过滤器总览](../overview.md)
