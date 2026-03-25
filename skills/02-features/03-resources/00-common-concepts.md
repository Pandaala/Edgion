---
name: route-common-concepts
description: 路由通用概念：parentRef 挂载、backendRef 后端引用、跨命名空间、resolved_ports。
---

# 路由通用概念

## parentRef — 路由挂载

所有路由通过 `parentRefs` 挂载到 Gateway Listener：

```yaml
spec:
  parentRefs:
    - name: my-gateway          # Gateway 名称（必填）
      namespace: default        # 命名空间（跨命名空间需 ReferenceGrant）
      sectionName: https        # 指定 Listener 名称（可选）
      port: 443                 # 指定端口（可选）
```

### parentRef Schema

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `name` | `String` | 是 | Gateway 资源名称 |
| `namespace` | `String?` | 否 | Gateway 命名空间 |
| `sectionName` | `String?` | 否 | Listener 名称 |
| `port` | `u16?` | 否 | 监听端口 |
| `group` | `String?` | 否 | 默认 `gateway.networking.k8s.io` |
| `kind` | `String?` | 否 | 默认 `Gateway` |

### 端口解析规则（resolved_ports）

Controller 根据 parentRef 自动计算路由的目标端口：

1. `parentRef.port` 已指定 → 直接使用
2. `parentRef.sectionName` 已指定 → 查找对应 Listener 获取端口
3. 都未指定 → 使用 Gateway 所有匹配 Listener 的端口

## backendRef — 后端引用

```yaml
rules:
  - backendRefs:
      - name: my-service        # Service 名称
        namespace: backend-ns   # 可选：跨命名空间需 ReferenceGrant
        port: 8080              # 目标端口
        weight: 80              # 权重（用于流量分割）
        kind: Service           # 默认 Service
        group: ""               # 默认 core group
```

### backendRef Schema

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `name` | `String` | 是 | 后端资源名称 |
| `port` | `i32?` | 否 | 目标端口 |
| `weight` | `i32?` | 否 | 流量权重（默认 1） |
| `namespace` | `String?` | 否 | 命名空间 |
| `kind` | `String?` | 否 | 默认 `Service` |
| `group` | `String?` | 否 | 默认 `""` |

## 跨命名空间引用

路由引用其他命名空间的资源（Gateway、Service）需要目标命名空间有对应的 ReferenceGrant：

```yaml
apiVersion: gateway.networking.k8s.io/v1beta1
kind: ReferenceGrant
metadata:
  name: allow-route-from-app
  namespace: backend-ns        # 被引用资源所在的命名空间
spec:
  from:
    - group: gateway.networking.k8s.io
      kind: HTTPRoute
      namespace: app-ns        # 引用来源命名空间
  to:
    - group: ""
      kind: Service
```

未授权的跨命名空间引用会在 backendRef 上设置 `ref_denied`，Gateway 侧据此拒绝请求（返回 500）。

## hostname 交集

对于支持 hostname 的路由（HTTPRoute、GRPCRoute、TLSRoute），Controller 会计算路由 hostnames 与 Gateway Listener hostnames 的交集，存入 `resolved_hostnames`。只有交集内的域名才会实际路由。

## Status 通用结构

所有路由资源的 status 按 parentRef 分组：

```yaml
status:
  parents:
    - parentRef:
        name: my-gateway
        namespace: default
        sectionName: https
      controllerName: edgion.io/gateway-controller
      conditions:
        - type: Accepted         # 路由已接受
          status: "True"
        - type: ResolvedRefs     # 所有引用已解析
          status: "True"
```
