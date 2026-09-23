# ForwardAuth（HTTPRoute 过滤器）

> **Edgion 扩展**
>
> ForwardAuth 是 `EdgionPlugins` CRD 提供的外部认证委托插件，不属于标准 Gateway API。

ForwardAuth 可以作为 HTTPRoute 的过滤器使用，也可以通过 `EdgionPlugins` 全局配置。本页介绍如何在 HTTPRoute 中使用；完整配置参数请见[独立插件页](../../../edgion-plugins/forward-auth.md)。

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

## 相关文档

- [ForwardAuth 完整文档（包含所有配置参数）](../../../edgion-plugins/forward-auth.md)
- [插件组合与引用](../plugin-composition.md)
- [过滤器总览](../overview.md)

## 响应头策略

配置校验拒绝 `decision.clientHeaders` 中的受保护或无效名称。拒绝响应会过滤所有收到的 `Connection` 值所提名的字段，包括自动复制的 `Content-Type`。若 `Connection` 含非文本值，则不复制远端响应头，但仍转发有界的拒绝响应体。允许的重复字段（包括 `Set-Cookie` 和 `WWW-Authenticate`）保留各个值。

对于 filesystem／etcd 配置中的内联 HTTPRoute `ExternalAuth`，控制器也会校验规则和 backend filter 配置，包括委派的源规则。无效规则会被排除，有效兄弟规则继续保留；修正配置后，无效规则状态会清除。此内联形式不表示兼容 Kubernetes Gateway API 的 `ExternalAuth` schema；Kubernetes 请使用本文说明的 `EdgionPlugins` 绑定。

[响应头策略](../gateway-api/response-header-modifier.md#response-header-policy).
