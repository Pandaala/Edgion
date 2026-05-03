---
name: tlsroute-features
description: TLSRoute 完整 Schema：基于 SNI 的 TLS 路由。
---

# TLSRoute 资源

> API: `gateway.networking.k8s.io/v1`（v1alpha3 兼容） | Scope: Namespaced
> Gateway API v1.5.0 Standard 资源（v1.4.0 Experimental 下为 v1alpha3）

TLSRoute 基于 TLS ClientHello 中的 SNI（Server Name Indication）进行路由，支持 TLS 透传或终止后转发。

### 多版本兼容

| 安装清单 | v1alpha3 | v1 |
|----------|----------|-----|
| v1.4.0 experimental | **served=true, storage=true** | 不存在 |
| v1.5.0 standard | served=false | **served=true, storage=true** |

Edgion 控制器优先 watch v1，v1 不可用时 fallback v1alpha3（单活 watch）。v1alpha3 → v1 是纯版本晋升，spec 字段结构完全相同。FileSystem 模式下 v1alpha3 YAML 在解析入口自动归一化到 v1。

## 完整 Schema

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: TLSRoute
metadata:
  name: my-tls-route
  namespace: default
  annotations:
    edgion.io/edgion-stream-plugins: "default/my-stream-plugins"
    edgion.io/proxy-protocol: "2"
    edgion.io/max-connect-retries: "3"             # 可选；默认 2，最高 5
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
| `edgion.io/max-connect-retries` | `u32` | `2` | 最大上游连接尝试次数，低于 `1` 归一化为 `1`，高于 `5` 封顶为 `5` |

## 与 Gateway TLS 的配合

| Gateway Listener TLS Mode | TLSRoute 行为 |
|---------------------------|--------------|
| `Passthrough` | TLS 透传：Gateway 不终止 TLS，按 SNI 路由到后端 |
| `Terminate` | TLS 终止：Gateway 解密后按 SNI 路由到后端 |

当前 Edgion 的产品边界：

- 支持标准 `Passthrough`
- 支持标准 `Terminate`
- 不支持 `TLSRouteModeMixed`
- BackendTLSPolicy 不对 TLSRoute 生效（Passthrough 透传原始 TLS，Terminate 走 stream 层；需要后端 TLS 应使用 HTTPRoute/GRPCRoute）

## 与 TCPRoute 的区别

| 维度 | TCPRoute | TLSRoute |
|------|---------|---------|
| 匹配维度 | 无（仅端口） | SNI hostname |
| 协议感知 | 无 | TLS ClientHello |
| 多路复用 | 一端口一路由 | 一端口多域名 |
