# gRPC 监听器

如何通过 Gateway 监听器暴露 gRPC（HTTP/2）服务。

## 概述

gRPC 运行于 HTTP/2 之上，因此 Gateway API 中**没有**独立的 gRPC Listener 协议。通过将 `GRPCRoute` 附加到现有监听器来提供 gRPC 服务：

- **`HTTPS`**——h2 over TLS，标准且安全的选择。
- **`HTTP`**——h2c（明文 HTTP/2），仅在可信网络或上游已终结 TLS 时使用。

两种情况下 Listener 的 `protocol` 仍为 `HTTP` / `HTTPS`；通过在 `allowedRoutes.kinds` 中列出 `GRPCRoute` 来开启 gRPC 支持。

## 配置

接受 gRPC 路由的 TLS 终结监听器：

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: grpc-gateway
  namespace: edgion-system
spec:
  gatewayClassName: edgion
  listeners:
    - name: grpc-tls
      port: 443
      protocol: HTTPS
      hostname: grpc.example.com
      tls:
        mode: Terminate
        certificateRefs:
          - name: grpc-example-com-tls
      allowedRoutes:
        namespaces:
          from: Same
        kinds:
          - kind: GRPCRoute
```

## 字段参考

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `protocol` | enum | 是 | `HTTPS`（h2 over TLS）或 `HTTP`（h2c）。不存在 `GRPC` 协议值。 |
| `port` | int | 是 | 绑定的端口（h2 通常为 `443`，h2c 可为任意端口）。 |
| `hostname` | string | 否 | SNI / authority 匹配，支持通配符 `*.example.com`。 |
| `tls.mode` | enum | `HTTPS` 时必填 | `Terminate`。 |
| `tls.certificateRefs` | list | `Terminate` 时必填 | 持有证书和私钥的 Secret 引用。 |
| `allowedRoutes.kinds` | list | 是 | 必须包含 `GRPCRoute`，否则 Listener 不接受 gRPC 路由。 |

## 配置场景

### gRPC over TLS（h2）

推荐配置：终结 TLS 并路由 gRPC。

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: grpc-secure-gateway
  namespace: edgion-system
spec:
  gatewayClassName: edgion
  listeners:
    - name: grpc-tls
      port: 443
      protocol: HTTPS
      hostname: grpc.example.com
      tls:
        mode: Terminate
        certificateRefs:
          - name: grpc-example-com-tls
      allowedRoutes:
        namespaces:
          from: Same
        kinds:
          - kind: GRPCRoute
```

### gRPC over 明文（h2c）

内部流量场景，TLS 在其他地方处理，使用 `HTTP` 监听器加 `GRPCRoute`。

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: grpc-internal-gateway
  namespace: edgion-system
spec:
  gatewayClassName: edgion
  listeners:
    - name: grpc-h2c
      port: 8080
      protocol: HTTP
      allowedRoutes:
        namespaces:
          from: Same
        kinds:
          - kind: GRPCRoute
```

### 单个 Listener 同时承载 HTTP 和 gRPC 路由

单个 HTTPS 监听器可以通过列出两种路由种类来同时接受 `HTTPRoute` 和 `GRPCRoute`。

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: mixed-h2-gateway
  namespace: edgion-system
spec:
  gatewayClassName: edgion
  listeners:
    - name: https
      port: 443
      protocol: HTTPS
      hostname: api.example.com
      tls:
        mode: Terminate
        certificateRefs:
          - name: api-example-com-tls
      allowedRoutes:
        namespaces:
          from: Same
        kinds:
          - kind: HTTPRoute
          - kind: GRPCRoute
```

## 边界情况与注意事项

- 不存在 `GRPC` Listener 协议，使用该值无效。请始终使用 `HTTP` 或 `HTTPS`，并通过 `allowedRoutes.kinds` 选择 gRPC。
- **h2c 是明文**：不要在不受信任的网络上暴露 h2c 监听器。
- **GRPCRoute 必须在允许列表中**：如果 `allowedRoutes.kinds` 省略了 `GRPCRoute`，即使端口开放，gRPC 路由也不会附加。
- **hostname / SNI 选择**：与 HTTPS 一样，多个监听器可以共享一个端口，通过 `hostname` 区分。
- **跨命名空间 AllowedRoutes**：默认 `from: Same`；使用 `from: All` 或 `from: Selector` 扩大范围。

## 相关文档

- [监听器总览](README.md)
- [HTTPS 监听器](https.md)
- [HTTP 监听器](http.md)
- [GRPCRoute 总览](../../../user-guide/grpc-route/overview.md)
