# HTTP 监听器

配置 HTTP 协议监听器。

## 基本配置

```yaml
listeners:
  - name: http
    port: 80
    protocol: HTTP
```

## 配置参考

| 字段 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| name | string | ✓ | | 监听器名称 |
| port | int | ✓ | | 监听端口 |
| protocol | string | ✓ | | 协议（HTTP） |
| hostname | string | | | 主机名过滤 |
| allowedRoutes | object | | | 允许的路由 |

## 主机名过滤

限制此监听器只处理特定域名：

```yaml
listeners:
  - name: api
    port: 80
    protocol: HTTP
    hostname: "api.example.com"
```

## 路由绑定控制

### 允许所有命名空间

```yaml
allowedRoutes:
  namespaces:
    from: All
```

### 只允许同命名空间

```yaml
allowedRoutes:
  namespaces:
    from: Same
```

### 允许指定命名空间

```yaml
allowedRoutes:
  namespaces:
    from: Selector
    selector:
      matchLabels:
        env: production
```

## 示例

### 示例 1: 多端口监听

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: multi-port
spec:
  gatewayClassName: edgion
  listeners:
    - name: http
      port: 80
      protocol: HTTP
    - name: http-alt
      port: 8080
      protocol: HTTP
```

### 示例 2: 按域名分离

```yaml
listeners:
  - name: api
    port: 80
    protocol: HTTP
    hostname: "api.example.com"
    allowedRoutes:
      kinds:
        - kind: HTTPRoute
  - name: web
    port: 80
    protocol: HTTP
    hostname: "www.example.com"
    allowedRoutes:
      kinds:
        - kind: HTTPRoute
```

## 相关文档

- [HTTPS 监听器](./https.md)
- [TCP 监听器](./tcp.md)
