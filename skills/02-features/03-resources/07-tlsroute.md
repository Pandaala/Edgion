---
name: tlsroute-features
description: TLSRoute 完整 Schema：基于 SNI 的 TLS 路由。
---

# TLSRoute 资源

> API: `gateway.networking.k8s.io/v1alpha2` | Scope: Namespaced
> Gateway API v1.4.0 Experimental 资源

TLSRoute 基于 TLS ClientHello 中的 SNI（Server Name Indication）进行路由，支持 TLS 透传或终止后转发。

## 完整 Schema

```yaml
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: TLSRoute
metadata:
  name: my-tls-route
  namespace: default
  annotations:
    edgion.io/edgion-stream-plugins: "default/my-stream-plugins"
    edgion.io/proxy-protocol: "2"
    edgion.io/upstream-tls: "true"
    edgion.io/max-connect-retries: "3"
spec:
  parentRefs:
    - name: my-gateway
      sectionName: tls-passthrough

  hostnames:                                       # SNI 匹配
    - "secure.example.com"
    - "*.internal.example.com"

  rules:
    - backendRefs:
        - name: tls-backend
          port: 8443
          weight: 100
```

## spec 字段

| 字段 | 类型 | 说明 |
|------|------|------|
| `parentRefs` | `Vec<ParentReference>` | 挂载到 TLS 类型 Listener |
| `hostnames` | `Vec<String>?` | SNI 主机名匹配列表（支持通配符） |
| `rules` | `Vec<TLSRouteRule>` | TLS 路由规则 |

### TLSRouteRule

| 字段 | 类型 | 说明 |
|------|------|------|
| `backendRefs` | `Vec<TLSBackendRef>` | 后端引用 |

### TLSBackendRef

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

## 与 Gateway TLS 的配合

| Gateway Listener TLS Mode | TLSRoute 行为 |
|---------------------------|--------------|
| `Passthrough` | TLS 透传：Gateway 不终止 TLS，按 SNI 路由到后端 |
| `Terminate` | TLS 终止：Gateway 解密后按 SNI 路由到后端 |

## 与 TCPRoute 的区别

| 维度 | TCPRoute | TLSRoute |
|------|---------|---------|
| 匹配维度 | 无（仅端口） | SNI hostname |
| 协议感知 | 无 | TLS ClientHello |
| 多路复用 | 一端口一路由 | 一端口多域名 |
