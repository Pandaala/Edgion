# HTTP to HTTPS 重定向

自动将 HTTP 明文请求重定向到对应的 HTTPS 地址。

> **🔌 Edgion 扩展**
>
> 此重定向通过 Gateway 的 `edgion.io/http-to-https-redirect` 注解配置，不属于标准 Gateway API 规范。

## 概述

在 Gateway 上设置 `edgion.io/http-to-https-redirect` 注解，Edgion 会对 HTTP 明文请求返回 `301 Moved Permanently` 重定向，目标为对应的 `https://` URL。这是在不丢失通过 80 端口访问的客户端的前提下强制 TLS 的常用方式。重定向会保留完整的路径和查询字符串，并在请求处理的最早阶段执行——不会执行任何插件或路由匹配。

注解接受 `"true"` 或 `"false"`；默认为 `"false"`，此时 HTTP 请求正常路由。

## 配置

在 Gateway 层面应用注解，它会覆盖该 Gateway 上的所有 `HTTP` Listener：

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: main-gateway
  namespace: edgion-system
  annotations:
    edgion.io/http-to-https-redirect: "true"
spec:
  gatewayClassName: edgion
  listeners:
    # HTTP listener：请求将被重定向到 HTTPS
    - name: http
      port: 80
      protocol: HTTP
      allowedRoutes:
        namespaces:
          from: Same
    # HTTPS listener：正常处理请求
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

## 注解参考

| 注解 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `edgion.io/http-to-https-redirect` | String | `"false"` | 设置为 `"true"` 时，Gateway 上的所有 `HTTP` Listener 返回 `301` 重定向，目标为对应的 `https://` 地址。 |
| `edgion.io/https-redirect-port` | String | `"443"` | HTTPS 重定向的目标端口，当 HTTPS 服务运行在非标准端口时使用。 |

## 配置场景

### 将所有 HTTP 流量重定向到标准 HTTPS

注解将 Gateway 上的所有 `HTTP` Listener 转为重定向到 443 端口。

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

### 重定向到非标准 HTTPS 端口

当 HTTPS 使用自定义端口时，设置 `edgion.io/https-redirect-port`，使 `Location` 头指向正确的端口。

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: custom-port-gateway
  namespace: edgion-system
  annotations:
    edgion.io/http-to-https-redirect: "true"
    edgion.io/https-redirect-port: "8443"
spec:
  gatewayClassName: edgion
  listeners:
    - name: http
      port: 8080
      protocol: HTTP
      allowedRoutes:
        namespaces:
          from: Same
    - name: https
      port: 8443
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

### 通过 HTTPRoute filter 实现按路径重定向

Gateway 注解对所有 HTTP Listener 全局生效。如需按路径控制，请去掉注解，改用 HTTPRoute 上的 `RequestRedirect` filter。

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: redirect-route
  namespace: edgion-system
spec:
  parentRefs:
    - name: site-gateway
      sectionName: http
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /secure/
      filters:
        - type: RequestRedirect
          requestRedirect:
            scheme: https
            statusCode: 301
```

## 边界情况与注意事项

- **注解值需加引号**：写 `"true"` 而非 `true`，避免 YAML 将其解析为布尔值。
- **Gateway 级别，非 Listener 级别**：启用后，Gateway 上的*所有* `HTTP` Listener 都会重定向，无法按 Listener 单独覆盖；如需更细粒度控制，请使用 HTTPRoute 的 `RequestRedirect` filter。
- **仅影响 HTTP Listener**：注解仅对 `protocol: HTTP` 的 Listener 生效，`HTTPS`、`TLS`、`TCP`、`UDP` Listener 不受影响。
- **301 会被缓存**：浏览器会缓存永久重定向，迭代测试时请使用新客户端。
- **需要提供匹配的 HTTPS Listener**：重定向目标为 `https://<host>`（在 `https-redirect-port` 端口上），确保有对应的监听，否则客户端会访问到死端口。
- **保留路径和查询参数**：完整 URI（包括查询参数）都会写入 `Location` 头。

## 相关文档

- [HTTPS 监听器](listeners/https.md)
- [HTTP 监听器](listeners/http.md)
- [Preflight 策略](preflight-policy.md)
- [Gateway 总览](overview.md)
