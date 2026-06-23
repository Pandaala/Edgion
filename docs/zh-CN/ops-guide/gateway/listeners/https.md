# HTTPS 监听器

在网关终结 TLS 并可附加 `HTTPRoute`（或 `GRPCRoute`）的监听器。

## 概述

HTTPS 监听器以 `protocol: HTTPS` 绑定端口，并在 `Terminate` 模式下配置 `tls` 块。网关呈现证书、解密连接，并将明文请求转发到后端。这是提供公共 HTTPS 和 h2 gRPC over TLS 的标准方式。

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
        kinds:
          - kind: HTTPRoute
```

## 字段参考

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `name` | string | 是 | Gateway 内唯一的 Listener 名称。 |
| `port` | int | 是 | 绑定的端口（通常为 `443`）。 |
| `protocol` | enum | 是 | 必须为 `HTTPS`。 |
| `hostname` | string | 否 | SNI / Host 匹配，支持通配符 `*.example.com`。 |
| `tls.mode` | enum | 是 | HTTPS 时必须为 `Terminate`。 |
| `tls.certificateRefs` | list | 是 | 持有证书和私钥的 Secret 引用。 |
| `allowedRoutes.namespaces.from` | enum | 否 | `Same`（默认）、`All` 或 `Selector`。 |
| `allowedRoutes.kinds` | list | 否 | `HTTPRoute`（默认）或 `GRPCRoute`。 |

## 配置场景

### 单主机 TLS 终结

单个证书的 HTTPS 监听器。

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: app-gateway
  namespace: edgion-system
spec:
  gatewayClassName: edgion
  listeners:
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

### 在 443 端口服务多个主机（SNI）

两个 HTTPS 监听器共享 `443` 端口，通过 `hostname` 区分，各自使用独立证书。

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: multi-host-gateway
  namespace: edgion-system
spec:
  gatewayClassName: edgion
  listeners:
    - name: https-app
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
    - name: https-api
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
```

### 子域名通配符证书

用一张通配符证书服务所有单标签子域名。

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: wildcard-gateway
  namespace: edgion-system
spec:
  gatewayClassName: edgion
  listeners:
    - name: https-wildcard
      port: 443
      protocol: HTTPS
      hostname: "*.example.com"
      tls:
        mode: Terminate
        certificateRefs:
          - name: wildcard-example-com-tls
      allowedRoutes:
        namespaces:
          from: Same
```

## 边界情况与注意事项

- **仅支持 Terminate**：HTTPS 监听器使用 `tls.mode: Terminate`。如需在不解密的情况下端到端加密，请使用 `TLS` 监听器 + `Passthrough` + `TLSRoute`——详见 [../../../user-guide/tls-route/overview.md](../../../user-guide/tls-route/overview.md)。
- **certificateRefs 必填**：`Terminate` 模式至少需要一个证书 Secret；否则 Listener 无法完成 TLS 握手。
- **hostname / SNI 选择**：多个 HTTPS 监听器共享端口时，客户端的 SNI 决定使用哪个 Listener；最具体的 `hostname` 优先，通配符仅匹配一个标签。
- **跨命名空间证书引用**：`certificateRef` 引用其他命名空间的 Secret 时，需要在 Secret 所在命名空间创建 `ReferenceGrant`。
- **跨命名空间 AllowedRoutes**：默认 `from: Same`；使用 `from: All` 或 `from: Selector` 扩大范围。
- **gRPC over TLS**：在 `allowedRoutes.kinds` 中加入 `GRPCRoute` 以支持 h2 gRPC。详见 [grpc.md](grpc.md)。

## 相关文档

- [监听器总览](README.md)
- [HTTP 监听器](http.md)
- [gRPC 监听器](grpc.md)
- [HTTP to HTTPS 重定向](../http-to-https-redirect.md)
- [TLS 路由（Passthrough）](../../../user-guide/tls-route/overview.md)
