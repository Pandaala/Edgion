# 跨命名空间引用（ReferenceGrant）

`ReferenceGrant` 用于显式授权跨命名空间资源引用，避免默认放开带来的安全风险。

## 典型场景

- Gateway 在 `gateway-system`，证书 Secret 在 `security` 命名空间。
- Route 在业务命名空间，后端 Service 在共享命名空间。

## 示例：允许 Gateway 引用跨命名空间 Secret

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: ReferenceGrant
metadata:
  name: allow-gw-to-secret
  namespace: security
spec:
  from:
    - group: gateway.networking.k8s.io
      kind: Gateway
      namespace: gateway-system
  to:
    - group: ""
      kind: Secret
```

## 示例：允许 EdgionTls 引用跨命名空间 Secret

`EdgionTls` 的 `spec.secretRef`（服务端证书）和 `spec.clientAuth.caSecretRef`（mTLS CA）
都会经过同一套 ReferenceGrant 校验。除非存在匹配的授权，否则跨命名空间的 `secretRef`
会被拒绝（证书保持未解析，`ResolvedRefs` 条件报告 `RefNotPermitted`）：

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: ReferenceGrant
metadata:
  name: allow-edgiontls-to-secret
  namespace: security
spec:
  from:
    - group: edgion.io
      kind: EdgionTls
      namespace: gateway-system
  to:
    - group: ""
      kind: Secret
```

## 安全建议

1. `from` 粒度尽量小，精确到 namespace 和 kind。
2. `to` 只放必要的 kind。
3. 定期审计跨命名空间授权对象。

## 相关文档

- [Secret 管理](./secret-management.md)
- [mTLS 配置](./mtls.md)
- [GatewayClass 配置](../gateway/gateway-class.md)
