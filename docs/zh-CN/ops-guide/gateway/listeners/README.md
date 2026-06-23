# 监听器

Gateway Listener 如何绑定端口、选择协议，以及决定哪些路由可以附加。

## 概述

`Gateway` 声明一个或多个 **Listener**。每个 Listener 绑定一个 `port` 和 `protocol`，可选地通过 `hostname` 约束流量，配置 TLS，并通过 `allowedRoutes` 控制哪些路由（以及来自哪些命名空间的路由）可以附加。单个 Gateway 通常混合配置多个 Listener——例如一个 HTTP Listener 重定向到一个 HTTPS Listener。

## Listener 字段

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `name` | string | 是 | Gateway 内唯一的 Listener 名称。 |
| `port` | int | 是 | 绑定的端口。 |
| `protocol` | enum | 是 | `HTTP`、`HTTPS`、`TLS`、`TCP`、`UDP` 之一。 |
| `hostname` | string | 否 | SNI / Host 头匹配，支持通配符 `*.example.com`。 |
| `tls` | object | 条件必填 | `HTTPS` 和 `TLS` 时必填，包含 `mode` 和 `certificateRefs`。 |
| `allowedRoutes` | object | 否 | 可附加的路由种类和命名空间。 |

### TLS 块

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `tls.mode` | enum | 是（`HTTPS`/`TLS`） | `Terminate`（在网关解密）或 `Passthrough`（转发加密流量）。 |
| `tls.certificateRefs` | list | `Terminate` 时必填 | 持有证书和私钥的 Secret 引用。 |
| `tls.options` | map | 否 | 通过 `edgion.io/*` 键配置的 Edgion TLS 扩展。 |

### AllowedRoutes 块

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `allowedRoutes.namespaces.from` | enum | 否 | `Same`（默认）、`All` 或 `Selector`。 |
| `allowedRoutes.namespaces.selector` | object | 条件必填 | 标签选择器，当 `from: Selector` 时使用。 |
| `allowedRoutes.kinds` | list | 否 | 允许的路由种类（如 `HTTPRoute`、`GRPCRoute`、`TLSRoute`、`TCPRoute`）。 |

## 协议 → 路由种类映射

| 协议 | 默认路由种类 | TLS 模式 |
|------|-------------|---------|
| `HTTP` | `HTTPRoute`（h2c 时也支持 `GRPCRoute`） | — |
| `HTTPS` | `HTTPRoute`（也支持 `GRPCRoute`） | `Terminate` |
| `TLS` | `TLSRoute` | `Passthrough`（或 `Terminate`） |
| `TCP` | `TCPRoute` | — |
| `UDP` | `UDPRoute` | — |

gRPC 没有独立的 Listener 协议。将 `GRPCRoute` 附加到 `HTTPS` Listener（h2 over TLS）或 `HTTP` Listener（h2c，明文），并在 `allowedRoutes.kinds` 中列出 `GRPCRoute`。详见 [grpc.md](grpc.md)。

## 各协议文档

- [HTTP 监听器](http.md)
- [HTTPS 监听器](https.md)
- [gRPC 监听器](grpc.md)
- [TCP 监听器](tcp.md)

## 边界情况

- **多个 Listener**：Gateway 可以同时暴露多个 Listener，每个 `name` 必须唯一。两个 Listener 仅在通过 `hostname` 区分且使用兼容协议时（例如多个具有不同 SNI 的 `HTTPS` Listener）才可以共享同一个 `port`。
- **hostname 选择**：请求匹配最具体的 Listener `hostname`。没有 `hostname` 的 Listener 匹配任意主机；通配符 `*.example.com` 仅匹配一个标签。
- **跨命名空间 AllowedRoutes**：默认（`from: Same`）仅允许 Gateway 所在命名空间的路由附加。使用 `from: All` 或 `from: Selector` 来允许其他命名空间的路由。
- **TLS Passthrough**：如需端到端加密到后端，使用 `protocol: TLS` + `mode: Passthrough` 并附加 `TLSRoute`。详见 [../../../user-guide/tls-route/overview.md](../../../user-guide/tls-route/overview.md)。

## 相关文档

- [Gateway 总览](../overview.md)
- [GatewayClass](../gateway-class.md)
- [TLS 路由（Passthrough）](../../../user-guide/tls-route/overview.md)
