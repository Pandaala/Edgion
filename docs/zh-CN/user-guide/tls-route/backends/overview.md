# TLSRoute 后端配置

后端是 TLSRoute 将匹配的 TLS 连接转发到的目标。由于 TLSRoute 工作在透传模式，网关将原始加密流直接代理到后端，不进行解密。

## 概述

TLSRoute 中的每条规则都携带一个 `backendRefs` 列表。后端引用指向 Kubernetes Service 和端口。网关向选定的后端建立连接并传输加密字节；由后端负责终止 TLS。当规则列出多个后端时，按 `weight` 在它们之间分配连接。

## 基本配置

单个后端接收所有匹配连接：

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: TLSRoute
metadata:
  name: example-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
      sectionName: tls-passthrough
  hostnames:
    - "secure.example.com"
  rules:
    - backendRefs:
        - name: api-service
          port: 443
```

## 后端引用参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `name` | String | —（必填） | 目标 Service 名称 |
| `port` | Integer | 无 | Service 上的目标端口；当 Service 暴露多个端口时为必填 |
| `weight` | Integer | `1` | 列出多个后端时，此后端接收连接的相对份额 |
| `namespace` | String | 当前命名空间 | 目标 Service 的命名空间；跨命名空间引用需要 ReferenceGrant |
| `kind` | String | `Service` | 被引用对象的类型 |
| `group` | String | `""`（空） | 被引用对象的 API 组；`""` 表示核心 Kubernetes API 组 |

## 常见配置场景

### 1. 单后端

最简场景——每条匹配路由 SNI 的连接都发往同一个 Service。

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: TLSRoute
metadata:
  name: example-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
      sectionName: tls-passthrough
  hostnames:
    - "secure.example.com"
  rules:
    - backendRefs:
        - name: api-service
          port: 443
```

### 2. 带权重的后端（流量分割）

列出多个带 `weight` 的后端来分割流量，例如在金丝雀发布期间。每个后端获得的份额等于其权重除以所有权重之和。

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: TLSRoute
metadata:
  name: api-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
      sectionName: tls-passthrough
  hostnames:
    - "secure.example.com"
  rules:
    - backendRefs:
        # 80% 发往稳定后端
        - name: api-service
          port: 443
          weight: 80
        # 20% 发往金丝雀后端
        - name: api-service-canary
          port: 443
          weight: 20
```

`weight: 0` 的后端不接收任何连接，但仍保留在声明中，适合在切流前暂存后端。

被拒绝的跨命名空间 backendRef 以及非 Service 的 `kind` 仍保留其声明权重。这些选中份额会失败关闭（`RefDenied` / `UnsupportedBackend`），并且不会重试到另一个后端。

### 3. 跨命名空间后端（ReferenceGrant）

若要转发到不同命名空间的 Service，在后端引用中设置 `namespace`，并在目标命名空间中创建一个 ReferenceGrant，允许从路由所在命名空间引用 TLSRoute。没有匹配的授权时，Controller 会将 backendRef 标记为拒绝，Gateway 在查找或拨号 Service 之前即关闭连接。

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: TLSRoute
metadata:
  name: example-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
      sectionName: tls-passthrough
  hostnames:
    - "secure.example.com"
  rules:
    - backendRefs:
        - name: api-service
          namespace: staging
          port: 443
---
apiVersion: gateway.networking.k8s.io/v1beta1
kind: ReferenceGrant
metadata:
  name: allow-production-tlsroutes
  namespace: staging
spec:
  from:
    - group: gateway.networking.k8s.io
      kind: TLSRoute
      namespace: production
  to:
    - group: ""
      kind: Service
```

## 注意事项

- **仅限透传模式。** 网关不解密流量，因此后端 TLS 验证（BackendTLSPolicy）不适用于 TLSRoute。如果需要网关验证或重新加密到后端的连接，请改用 HTTPRoute 或 GRPCRoute 进行 TLS 终止。
- **后端负责终止 TLS。** 由于原始加密字节原封不动地转发，后端 Service 必须持有对客户端发送的 SNI 有效的证书。
- **多端口 Service 必须指定 port。** 当目标 Service 仅暴露单个端口时可以省略 `port`；否则必须显式指定。

## 相关文档

- [TLSRoute 总览](../overview.md)
- [TLS 终止](../../../ops-guide/gateway/tls/tls-termination.md)
- [TCPRoute 后端配置](../../tcp-route/backends/overview.md)
