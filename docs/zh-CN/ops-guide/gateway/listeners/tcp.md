# TCP 监听器

转发原始 TCP 连接并可附加 `TCPRoute` 的四层监听器。

## 概述

TCP 监听器以 `protocol: TCP` 绑定端口，在不检查应用层数据的情况下将连接转发到后端。适用于非 HTTP 协议——数据库、消息队列或任何自定义 TCP 服务。由于 TCP 路由是四层路由，监听器层面没有 `hostname` 匹配，也不进行 TLS 终结。

## 配置

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: tcp-gateway
  namespace: edgion-system
spec:
  gatewayClassName: edgion
  listeners:
    - name: postgres
      port: 5432
      protocol: TCP
      allowedRoutes:
        namespaces:
          from: Same
        kinds:
          - kind: TCPRoute
```

## 字段参考

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `name` | string | 是 | Gateway 内唯一的 Listener 名称。 |
| `port` | int | 是 | 绑定的端口。 |
| `protocol` | enum | 是 | 必须为 `TCP`。 |
| `hostname` | string | 否 | TCP 不使用此字段；四层没有主机概念，值会被忽略。 |
| `allowedRoutes.namespaces.from` | enum | 否 | `Same`（默认）、`All` 或 `Selector`。 |
| `allowedRoutes.kinds` | list | 否 | `TCPRoute`（`TCP` 协议的默认值）。 |

TCP 监听器无 `tls` 块，监听器层不进行 TLS 终结。如需加密转发，请使用 `Passthrough` 模式的 `TLS` 监听器（见下文）。

## 配置场景

### 转发数据库端口

通过网关暴露 PostgreSQL 后端。

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: db-gateway
  namespace: edgion-system
spec:
  gatewayClassName: edgion
  listeners:
    - name: postgres
      port: 5432
      protocol: TCP
      allowedRoutes:
        namespaces:
          from: Same
```

### 单个 Gateway 上的多个 TCP 服务

绑定多个端口，每个服务一个监听器。

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: l4-gateway
  namespace: edgion-system
spec:
  gatewayClassName: edgion
  listeners:
    - name: redis
      port: 6379
      protocol: TCP
      allowedRoutes:
        namespaces:
          from: Same
    - name: mysql
      port: 3306
      protocol: TCP
      allowedRoutes:
        namespaces:
          from: Same
```

### 接受其他命名空间的 TCP 路由

允许应用团队的 `TCPRoute` 附加到共享的四层监听器。

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: shared-tcp-gateway
  namespace: edgion-system
spec:
  gatewayClassName: edgion
  listeners:
    - name: broker
      port: 9092
      protocol: TCP
      allowedRoutes:
        namespaces:
          from: Selector
          selector:
            matchLabels:
              edgion.io/tcp-tenant: "true"
        kinds:
          - kind: TCPRoute
```

## 边界情况与注意事项

- **无 hostname 路由**：TCP 是四层协议，`hostname` 不适用。`TCPRoute` 选择后端而非主机。如需按 SNI 路由，请使用 `TLS` 监听器。
- **无 TLS 终结**：TCP 监听器不终结 TLS。如需加密后端，请使用 `protocol: TLS` + `tls.mode: Passthrough` + `TLSRoute`——详见 [../../../user-guide/tls-route/overview.md](../../../user-guide/tls-route/overview.md)。
- **每端口一个后端**：由于没有七层匹配，通常每个 TCP 监听器附加一个 `TCPRoute`（一个后端）；多个服务请使用不同端口。
- **跨命名空间 AllowedRoutes**：默认 `from: Same`；使用 `from: All` 或 `from: Selector` 扩大范围。

## 相关文档

- [监听器总览](README.md)
- [HTTPS 监听器](https.md)
- [TLS 路由（Passthrough）](../../../user-guide/tls-route/overview.md)
