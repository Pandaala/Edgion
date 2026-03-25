---
name: udproute-features
description: UDPRoute 完整 Schema：UDP 四层路由。
---

# UDPRoute 资源

> API: `gateway.networking.k8s.io/v1alpha2` | Scope: Namespaced
> Gateway API v1.4.0 Experimental 资源

UDPRoute 定义 UDP 流量的路由规则。

## 完整 Schema

```yaml
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: UDPRoute
metadata:
  name: my-udp-route
  namespace: default
spec:
  parentRefs:
    - name: my-gateway
      sectionName: udp

  rules:
    - backendRefs:
        - name: dns-service
          port: 53
          weight: 100
```

## spec 字段

| 字段 | 类型 | 说明 |
|------|------|------|
| `parentRefs` | `Vec<ParentReference>` | 挂载到 UDP 类型 Listener |
| `rules` | `Vec<UDPRouteRule>` | UDP 路由规则 |

### UDPRouteRule

| 字段 | 类型 | 说明 |
|------|------|------|
| `backendRefs` | `Vec<UDPBackendRef>` | 后端引用 |

### UDPBackendRef

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `name` | `String` | 是 | Service 名称 |
| `port` | `i32?` | 否 | 目标端口 |
| `weight` | `i32?` | 否 | 流量权重 |
| `namespace` | `String?` | 否 | 命名空间 |
| `kind` | `String?` | 否 | 默认 `Service` |
| `group` | `String?` | 否 | 默认 `""` |

## 特点

- **无匹配条件**：UDP 路由仅按端口转发
- **per-port 隔离**：每个 Listener 端口独立路由
- **无连接状态**：UDP 无连接概念，每个数据报独立路由
