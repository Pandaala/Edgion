# 跨命名空间引用（ReferenceGrant）

`ReferenceGrant` 是标准 Gateway API 资源，用于显式授权跨命名空间资源引用。默认情况下，Gateway API 禁止某命名空间中的资源引用另一命名空间中的资源；`ReferenceGrant` 在*目标*命名空间中创建，授予例外权限，从而避免不受限制的跨命名空间访问带来的安全风险。

## 概述

Grant 始终位于被引用资源所在的命名空间（**目标**命名空间）。它声明哪些 `from` 资源（按 group、kind 和 namespace）可以引用该命名空间中哪些 `to` 资源（按 group 和 kind）。

```
namespace A (source)              namespace B (target)
  Gateway  ──── references ────►  Secret
                                  ReferenceGrant  ← lives here, allows the reference
```

> **API 版本**：ReferenceGrant 在 `gateway.networking.k8s.io/v1beta1` 提供服务。与 Gateway 和 HTTPRoute（GA 版本为 `v1`）不同，ReferenceGrant 仍处于 `v1beta1`。

## 典型场景

- `gateway-system` 中的 Gateway 引用 `security` 命名空间中的 TLS 证书 Secret。
- 业务命名空间中的 HTTPRoute 引用共享命名空间中的后端 Service。

## 示例：允许 Gateway 引用跨命名空间 Secret

Secret 位于 `security` 命名空间，因此 Grant 在 `security` 中创建：

```yaml
apiVersion: gateway.networking.k8s.io/v1beta1
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

## 示例：允许 HTTPRoute 引用跨命名空间 Service

Service 位于 `shared` 命名空间，因此 Grant 在 `shared` 中创建：

```yaml
apiVersion: gateway.networking.k8s.io/v1beta1
kind: ReferenceGrant
metadata:
  name: allow-route-to-service
  namespace: shared
spec:
  from:
    - group: gateway.networking.k8s.io
      kind: HTTPRoute
      namespace: production
  to:
    - group: ""
      kind: Service
```

## 字段参考

| 字段 | 类型 | 说明 |
|------|------|------|
| `spec.from[].group` | String | 引用方资源的 API group（如 `gateway.networking.k8s.io`） |
| `spec.from[].kind` | String | 引用方资源的类型（如 `Gateway`、`HTTPRoute`） |
| `spec.from[].namespace` | String | 引用方资源必须所在的命名空间 |
| `spec.to[].group` | String | 目标资源的 API group。`Secret` 和 `Service` 等核心资源使用 `""`（空字符串） |
| `spec.to[].kind` | String | 目标资源的类型，可选搭配 `name` 字段限定到单个对象 |

> 核心 API group（Secret、Service、ConfigMap）是空字符串 `""`，而非 `core`。

## 安全建议

1. `from` 粒度尽量小——精确指定命名空间和 kind。
2. `to` 中只放必要的 kind。
3. 在可行时使用可选的 `to[].name` 字段，将 Grant 限定到单个命名对象。
4. 定期审计跨命名空间授权。

## 常见问题

### Q1：我创建了 ReferenceGrant，但跨命名空间引用仍被拒绝。为什么？

**A**：确认 Grant 在**目标**命名空间（被引用资源所在的命名空间），而非源命名空间。同时确认 `from` 中的 group/kind/namespace 与引用方资源完全匹配，以及核心资源的 `to.group` 为 `""`。

### Q2：同命名空间的引用也需要 ReferenceGrant 吗？

**A**：不需要。同一命名空间内的引用始终允许；ReferenceGrant 仅在源和目标处于不同命名空间时才需要。

### Q3：为什么我的 ReferenceGrant 被 API server 拒绝？

**A**：使用 `apiVersion: gateway.networking.k8s.io/v1beta1`。ReferenceGrant 不在 `v1` 提供服务，因此 `v1` 的 apiVersion 会导致应用失败。

## 相关文档

- [Secret 管理](./secret-management.md)
- [mTLS 配置](./mtls.md)
- [GatewayClass 配置](../gateway/gateway-class.md)
