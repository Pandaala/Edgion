# TLSRoute 总览

TLSRoute 通过 ClientHello 中的 SNI 主机名匹配入站 TLS 连接，并将加密字节流转发到后端，由后端自行负责 TLS 终止。

TLSRoute 是标准 Gateway API 资源。它是 TCPRoute 的加密感知对应物：TCPRoute 仅凭监听器端口路由原始 TCP 流，而 TLSRoute 则读取 TLS 握手中未加密的 SNI 字段并按主机名路由，使多个主机名可以共用同一个监听器端口。

**TLS 透传**意味着网关不解密流量，仅读取 SNI 来选择后端，然后将原始加密流端到端地代理出去。这要求父 Gateway 监听器的 TLS 模式为 `Passthrough`。这与**TLS 终止**不同——TLS 终止时网关会解密流量并处理明文（详见 [TLS 终止](../../ops-guide/gateway/tls/tls-termination.md)）。

## 基本结构

最简 TLSRoute 附加到 TLS 监听器，匹配一个 SNI 主机名，并转发到单个后端。

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: TLSRoute
metadata:
  name: example-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
      # TLS 模式设为 Passthrough 的监听器名称
      sectionName: tls-passthrough
  hostnames:
    - "secure.example.com"
  rules:
    - backendRefs:
        - name: api-service
          port: 443
```

关键字段：

- `parentRefs` — 将路由附加到 TLS 模式为 `Passthrough` 的 Gateway 监听器。
- `hostnames` — 此路由匹配的 SNI 值。
- `rules[].backendRefs` — 接收转发 TLS 流的后端。

## 核心概念

### ParentRefs

`parentRefs` 将 TLSRoute 绑定到 Gateway 监听器。使用 `sectionName` 按名称指定目标监听器，该监听器必须设置 `tls.mode: Passthrough`，以便网关转发加密字节而不是解密。一个 TLSRoute 可以附加到多个 Gateway。

更多关于监听器的内容，见 [TLS 终止](../../ops-guide/gateway/tls/tls-termination.md)。

### Hostnames（SNI）

`hostnames` 是 SNI 匹配列表。网关将每个值与 ClientHello 中发送的 SNI 进行比较。支持精确名称（`secure.example.com`）和通配符（`*.internal.example.com`），通配符匹配恰好一个标签。如果省略 `hostnames`，则路由匹配到达该监听器的所有 SNI。当多个 TLSRoute 共享一个监听器时，最精确的主机名匹配获胜，因此应将精确名称与通配符并列。

### Rules

`rules` 是路由规则列表。与 HTTPRoute 不同，TLSRoute 规则除了 `hostnames` 已完成的 SNI 选择外没有其他匹配条件——规则仅用于携带 `backendRefs`。大多数 TLSRoute 只有一条规则。

### BackendRefs

`rules[].backendRefs` 列出匹配连接的目标。每个条目命名一个 Service 和端口；当列出多个后端时，流量按 `weight` 分配。跨命名空间的后端需要 ReferenceGrant。

更多关于后端的内容，见 [后端配置](./backends/overview.md)。

## 完整示例

此 TLSRoute 匹配两个精确 SNI 主机名和一个通配符，并将流量分配到两个带权重的后端。

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
    - "api.example.com"
    - "*.internal.example.com"
  rules:
    - backendRefs:
        # 90% 的匹配连接发往稳定后端
        - name: api-service
          port: 443
          weight: 90
        # 10% 发往金丝雀后端
        - name: api-service-canary
          port: 443
          weight: 10
```

## 子文档导航

| 主题 | 文档 | 说明 |
|------|------|------|
| 后端配置 | [backends/](./backends/overview.md) | 定义目标 Service、端口、权重及跨命名空间引用 |

## 相关文档

- [TLS 终止](../../ops-guide/gateway/tls/tls-termination.md)
- [后端配置](./backends/overview.md)
- [TCPRoute 总览](../tcp-route/overview.md)
