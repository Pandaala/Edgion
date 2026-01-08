# HTTP to HTTPS 重定向指南

本指南介绍如何通过 Gateway annotation 开启 HTTP 到 HTTPS 的全局重定向。

## 功能说明

当启用此功能后，所有发送到 HTTP 端口的请求都会被自动重定向到 HTTPS，类似于 nginx 的配置：

```nginx
return 301 https://$host$request_uri;
```

## 快速开始

### 启用重定向

在 Gateway 资源上添加 annotation：

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: my-gateway
  namespace: default
  annotations:
    edgion.io/http-to-https-redirect: "true"
spec:
  gatewayClassName: edgion
  listeners:
    # HTTP 监听器 - 会自动重定向到 HTTPS
    - name: http
      port: 80
      protocol: HTTP
    # HTTPS 监听器 - 正常处理请求
    - name: https
      port: 443
      protocol: HTTPS
      tls:
        certificateRefs:
          - name: my-tls-secret
```

### 自定义 HTTPS 端口

如果 HTTPS 服务运行在非标准端口，可以指定重定向目标端口：

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: my-gateway
  annotations:
    edgion.io/http-to-https-redirect: "true"
    edgion.io/https-redirect-port: "8443"
spec:
  gatewayClassName: edgion
  listeners:
    - name: http
      port: 8080
      protocol: HTTP
    - name: https
      port: 8443
      protocol: HTTPS
      tls:
        certificateRefs:
          - name: my-tls-secret
```

## Annotation 参考

| Annotation | 类型 | 默认值 | 说明 |
|------------|------|--------|------|
| `edgion.io/http-to-https-redirect` | string | `"false"` | 设置为 `"true"` 启用 HTTP 到 HTTPS 重定向 |
| `edgion.io/https-redirect-port` | string | `"443"` | HTTPS 重定向目标端口 |

## 工作原理

1. 当 Gateway 配置了 `edgion.io/http-to-https-redirect: "true"` 时
2. 所有 HTTP 协议的监听器会使用轻量级的重定向处理器
3. 收到请求后立即返回 `301 Moved Permanently` 响应
4. `Location` 头设置为对应的 HTTPS URL

### 示例请求/响应

**请求：**
```
GET /api/users?page=1 HTTP/1.1
Host: example.com
```

**响应：**
```
HTTP/1.1 301 Moved Permanently
Location: https://example.com/api/users?page=1
Content-Length: 0
Connection: close
```

## 注意事项

1. **仅影响 HTTP 监听器**：此 annotation 只对 `protocol: HTTP` 的监听器生效，HTTPS 监听器不受影响

2. **全局生效**：启用后，该 Gateway 下所有 HTTP 监听器都会重定向，无法针对单个监听器配置

3. **不处理业务逻辑**：重定向发生在请求处理的最早阶段，不会执行任何插件或路由匹配

4. **SEO 友好**：使用 301 永久重定向，搜索引擎会自动更新索引

## 常见问题

### Q: 如何只对特定路径启用重定向？

A: 此功能是全局重定向，不支持路径级别的控制。如果需要更细粒度的控制，建议在 HTTPRoute 中配置 RequestRedirect 过滤器。

### Q: 重定向会保留查询参数吗？

A: 是的，完整的 URI（包括路径和查询参数）都会被保留。

### Q: 性能影响如何？

A: 重定向处理器非常轻量，不涉及任何上游连接或复杂的路由匹配，性能开销极低。

## 相关文档

- [EdgionTLS 证书配置](../user-guide/edgiontls-user-guide.md)
- [Annotations 指南](../developer-doc/annotations-guide.md)
- [Gateway API 官方文档](https://gateway-api.sigs.k8s.io/)

