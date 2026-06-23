# HTTP 监听器

接受 HTTP/1.1 和 h2c 流量的明文监听器，可附加 `HTTPRoute`（或 h2c 时附加 `GRPCRoute`）。

## 概述

HTTP 监听器以 `protocol: HTTP` 绑定端口，处理明文流量。适用于内部流量、重定向到 HTTPS 的端口，或 TLS 在上游终结的 h2c gRPC 场景。无需 `tls` 块。

## 配置

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: web-gateway
  namespace: edgion-system
spec:
  gatewayClassName: edgion
  listeners:
    - name: http
      port: 80
      protocol: HTTP
      hostname: app.example.com
      allowedRoutes:
        namespaces:
          from: Same
        kinds:
          - kind: HTTPRoute
```

## 字段参考

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `name` | string | 是 | Gateway 内唯一的 Listener 名称。 |
| `port` | int | 是 | 绑定的端口（通常为 `80`）。 |
| `protocol` | enum | 是 | 必须为 `HTTP`。 |
| `hostname` | string | 否 | Host 头匹配，支持通配符 `*.example.com`，省略时匹配任意主机。 |
| `allowedRoutes.namespaces.from` | enum | 否 | `Same`（默认）、`All` 或 `Selector`。 |
| `allowedRoutes.kinds` | list | 否 | 允许的路由种类；默认 `HTTPRoute`，h2c 时可用 `GRPCRoute`。 |

HTTP 监听器无 `tls` 块，TLS 模式不适用。

## 配置场景

### 基础 HTTP 服务

内部服务的单个明文监听器。

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: internal-gateway
  namespace: edgion-system
spec:
  gatewayClassName: edgion
  listeners:
    - name: http
      port: 80
      protocol: HTTP
      allowedRoutes:
        namespaces:
          from: Same
```

### HTTP 重定向到 HTTPS

搭配重定向注解，使 80 端口的客户端自动升级到 HTTPS。

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: site-gateway
  namespace: edgion-system
  annotations:
    edgion.io/http-to-https-redirect: "true"
spec:
  gatewayClassName: edgion
  listeners:
    - name: http
      port: 80
      protocol: HTTP
      hostname: app.example.com
      allowedRoutes:
        namespaces:
          from: Same
    - name: https
      port: 443
      protocol: HTTPS
      hostname: app.example.com
      tls:
        mode: Terminate
        certificateRefs:
          - name: app-example-com-tls
      allowedRoutes:
        namespaces:
          from: Same
```

详见 [../http-to-https-redirect.md](../http-to-https-redirect.md)。

### 接受其他命名空间的路由

允许任意命名空间的 `HTTPRoute` 附加到共享入口 Listener。

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: shared-ingress
  namespace: edgion-system
spec:
  gatewayClassName: edgion
  listeners:
    - name: http
      port: 80
      protocol: HTTP
      allowedRoutes:
        namespaces:
          from: All
        kinds:
          - kind: HTTPRoute
```

## 边界情况与注意事项

- **多个 Listener**：HTTP 监听器可与同一 Gateway 上的 HTTPS/TLS/TCP/UDP 监听器共存，每个 Listener `name` 必须唯一。
- **hostname 选择**：请求匹配最具体的 `hostname`，无 `hostname` 的 Listener 接受任意 Host；`*.example.com` 仅匹配一个标签。
- **跨命名空间 AllowedRoutes**：默认 `from: Same` 限制附加到 Gateway 所在命名空间；使用 `from: All` 或 `from: Selector` 扩大范围。
- **h2c gRPC**：如需明文 gRPC 服务，将 `allowedRoutes.kinds` 设置为包含 `GRPCRoute`。详见 [grpc.md](grpc.md)。
- 明文 HTTP 在网络上传输流量；公共端点建议使用 HTTPS，可选配 HTTP Listener 上的重定向注解。

## 相关文档

- [监听器总览](README.md)
- [HTTPS 监听器](https.md)
- [gRPC 监听器](grpc.md)
- [HTTP to HTTPS 重定向](../http-to-https-redirect.md)
