# Gateway 资源总览

Gateway 是 Gateway API 的核心资源，定义了流量入口点。

## 资源结构

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: my-gateway
  namespace: default
spec:
  gatewayClassName: edgion  # 引用 GatewayClass
  listeners:                 # 监听器列表
    - name: http
      port: 80
      protocol: HTTP
    - name: https
      port: 443
      protocol: HTTPS
      tls:
        mode: Terminate
        certificateRefs:
          - name: tls-secret
```

## 核心概念

### GatewayClass

Gateway 必须引用一个 GatewayClass，它定义了 Gateway 的实现方式：

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GatewayClass
metadata:
  name: edgion
spec:
  controllerName: edgion.io/gateway-controller
```

### Listeners - 监听器

每个监听器定义一个流量入口：

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| name | string | ✓ | 监听器名称（路由通过此名称绑定） |
| port | int | ✓ | 监听端口 |
| protocol | string | ✓ | 协议（HTTP/HTTPS/TCP/TLS） |
| hostname | string | | 匹配的主机名 |
| tls | object | | TLS 配置（HTTPS/TLS 必需） |
| allowedRoutes | object | | 允许绑定的路由 |

## 示例

### 示例 1: HTTP Gateway

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: http-gateway
spec:
  gatewayClassName: edgion
  listeners:
    - name: http
      port: 80
      protocol: HTTP
      allowedRoutes:
        namespaces:
          from: All
```

### 示例 2: HTTPS Gateway

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: https-gateway
spec:
  gatewayClassName: edgion
  listeners:
    - name: https
      port: 443
      protocol: HTTPS
      tls:
        mode: Terminate
        certificateRefs:
          - name: wildcard-tls
      allowedRoutes:
        namespaces:
          from: All
```

## 相关文档

- [GatewayClass 配置](./gateway-class.md)
- [监听器配置](./listeners/)
- [TLS 配置](./tls/)
