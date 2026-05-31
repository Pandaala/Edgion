# Gateway

Edgion 如何实现 Kubernetes Gateway API 以及各组件如何协同工作。

## 概述

Edgion 是 Gateway API 的实现。您通过标准 Gateway API 资源声明意图，Edgion 控制器（Controller）负责编程数据面（Gateway）以匹配配置。典型部署包含三层：

- **GatewayClass**：绑定 Edgion 控制器，可选择指向调优参数。
- 一个或多个 **Gateway** 资源：每个声明 **Listener**（端口 + 协议 + TLS）。
- **路由**（`HTTPRoute`、`GRPCRoute`、`TLSRoute`、`TCPRoute`、`UDPRoute`）：附加到 Listener，定义流量的匹配与转发规则。

Edgion 支持 HTTP/1.1、HTTP/2、gRPC、TCP、UDP、TLS 和 WebSocket，并提供插件系统、负载均衡和 TLS/mTLS。

## 请求流转

1. 客户端连接到 Listener 地址（`port` + `protocol`）。
2. Listener 通过 `hostname`（SNI / Host 头）和 `allowedRoutes` 选择匹配的路由。
3. 路由匹配请求（路径、请求头、方法、gRPC service/method、SNI），并将其转发到后端。

## 资源速览

| 资源 | API group | 用途 |
|------|-----------|------|
| `GatewayClass` | `gateway.networking.k8s.io/v1` | 绑定 Edgion 控制器；可选 `parametersRef`。 |
| `Gateway` | `gateway.networking.k8s.io/v1` | 声明 Listener（端口、协议、TLS、允许的路由）。 |
| `HTTPRoute` | `gateway.networking.k8s.io/v1` | 匹配并转发 HTTP 流量。 |
| `GRPCRoute` | `gateway.networking.k8s.io/v1` | 匹配并转发 gRPC（HTTP/2）流量。 |
| `TLSRoute` | `gateway.networking.k8s.io/v1alpha2` | 用于 `Passthrough` TLS 的 SNI 路由。 |
| `TCPRoute` | `gateway.networking.k8s.io/v1alpha2` | 四层 TCP 转发。 |
| `UDPRoute` | `gateway.networking.k8s.io/v1alpha2` | 四层 UDP 转发。 |
| `EdgionGatewayConfig` | `edgion.io/v1alpha1` | 集群作用域的网关调优配置，通过 `parametersRef` 引用。 |

## 最小示例

一个 GatewayClass 加上一个包含单个 HTTP Listener 的 Gateway：

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GatewayClass
metadata:
  name: edgion
spec:
  controllerName: edgion.io/gateway-controller
---
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
      allowedRoutes:
        namespaces:
          from: Same
```

## Edgion 扩展

> **🔌 Edgion 扩展**
>
> 以下功能是 Edgion 特有的，不属于标准 Gateway API 规范。

Edgion 通过 `edgion.io/*` 注解和 `EdgionGatewayConfig` 资源在标准规范之上添加了额外行为：

- `edgion.io/http-to-https-redirect`（Gateway 注解）——将 HTTP 请求重定向到 HTTPS。详见 [http-to-https-redirect.md](http-to-https-redirect.md)。
- `EdgionGatewayConfig.spec.preflightPolicy`——CORS 预检（`OPTIONS`）处理。详见 [preflight-policy.md](preflight-policy.md)。

## 相关文档

- [GatewayClass](gateway-class.md)
- [监听器](listeners/README.md)
- [HTTP to HTTPS 重定向](http-to-https-redirect.md)
- [Preflight 策略](preflight-policy.md)
- [EdgionGatewayConfig](../configuration/edgion-gateway-config.md)
- [TLS 路由（Passthrough）](../../user-guide/tls-route/overview.md)
