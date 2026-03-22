---
name: tcproute-features
description: TCPRoute 完整 Schema：TCP 四层路由。
---

# TCPRoute 资源

> API: `gateway.networking.k8s.io/v1alpha2` | Scope: Namespaced
> Gateway API v1.4.0 Experimental 资源

TCPRoute 定义原始 TCP 流量的路由规则。TCP 路由不做任何协议解析，仅按端口转发。

## 完整 Schema

```yaml
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: TCPRoute
metadata:
  name: my-tcp-route
  namespace: default
  annotations:
    edgion.io/edgion-stream-plugins: "default/my-stream-plugins"  # StreamPlugins 绑定
    edgion.io/proxy-protocol: "1"                                  # Proxy Protocol 版本
    edgion.io/upstream-tls: "true"                                 # 上游启用 TLS
    edgion.io/max-connect-retries: "3"                             # 最大连接重试
spec:
  parentRefs:
    - name: my-gateway
      sectionName: tcp

  rules:
    - backendRefs:
        - name: tcp-service
          port: 9000
          weight: 100
```

## spec 字段

| 字段 | 类型 | 说明 |
|------|------|------|
| `parentRefs` | `Vec<ParentReference>` | 挂载到 Gateway 的 TCP 类型 Listener |
| `rules` | `Vec<TCPRouteRule>` | TCP 路由规则（通常只有一条） |

### TCPRouteRule

| 字段 | 类型 | 说明 |
|------|------|------|
| `backendRefs` | `Vec<TCPBackendRef>` | TCP 后端引用 |

### TCPBackendRef

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `name` | `String` | 是 | Service 名称 |
| `port` | `i32?` | 否 | 目标端口 |
| `weight` | `i32?` | 否 | 流量权重 |
| `namespace` | `String?` | 否 | 命名空间 |
| `kind` | `String?` | 否 | 默认 `Service` |
| `group` | `String?` | 否 | 默认 `""` |

## Edgion 扩展注解

| 注解 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `edgion.io/edgion-stream-plugins` | `ns/name` | — | 绑定 EdgionStreamPlugins |
| `edgion.io/proxy-protocol` | `"1"\|"2"` | — | 向上游发送 Proxy Protocol |
| `edgion.io/upstream-tls` | `"true"\|"false"` | `"false"` | 上游连接启用 TLS |
| `edgion.io/max-connect-retries` | `u32` | `1` | 最大上游连接重试次数 |

## 特点

- **无匹配条件**：TCP 路由仅按端口转发，不解析协议内容
- **per-port 隔离**：每个 Listener 端口独立路由
- **StreamPlugins**：通过注解绑定 TCP 层插件（如 IP 限制）
