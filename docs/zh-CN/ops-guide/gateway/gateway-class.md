# GatewayClass 配置

GatewayClass 定义了 Gateway 的实现类型，类似于 IngressClass。

> **🔌 Edgion 扩展**
> 
> `parametersRef` 可引用 `EdgionGatewayConfig` CRD 进行高级配置，这是 Edgion 扩展功能。

## 资源结构

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GatewayClass
metadata:
  name: edgion
spec:
  controllerName: edgion.io/gateway-controller
  parametersRef:              # 可选：引用配置参数
    group: edgion.io
    kind: EdgionGatewayConfig
    name: default-config
```

## 配置参考

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| controllerName | string | ✓ | 控制器标识 |
| parametersRef | object | | 配置参数引用 |
| description | string | | 描述信息 |

## Edgion 控制器名称

Edgion 使用的控制器名称：

```yaml
controllerName: edgion.io/gateway-controller
```

## 示例

### 示例 1: 基本配置

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GatewayClass
metadata:
  name: edgion
spec:
  controllerName: edgion.io/gateway-controller
```

### 示例 2: 带参数配置

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GatewayClass
metadata:
  name: edgion-custom
spec:
  controllerName: edgion.io/gateway-controller
  parametersRef:
    group: edgion.io
    kind: EdgionGatewayConfig
    name: custom-config
---
apiVersion: edgion.io/v1alpha1
kind: EdgionGatewayConfig
metadata:
  name: custom-config
spec:
  server:
    threads: 4
    gracePeriodSeconds: 30
```

## EdgionGatewayConfig 参考

`EdgionGatewayConfig` 是 Edgion 扩展的 CRD，通过 GatewayClass 的 `parametersRef` 引用，提供网关级别的高级配置。

### spec.server

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| threads | integer | CPU 核数 | Worker 线程数 |
| workStealing | boolean | true | 启用 work-stealing 调度 |
| gracePeriodSeconds | integer | 30 | 优雅关闭宽限期（秒） |
| gracefulShutdownTimeoutS | integer | 10 | 优雅关闭超时（秒） |
| upstreamKeepalivePoolSize | integer | 128 | 上游 keepalive 连接池大小 |
| enableCompression | boolean | false | 启用下游响应压缩 |
| downstreamKeepaliveRequestLimit | integer | 1000 | 下游连接复用请求数上限 |

#### downstreamKeepaliveRequestLimit

限制单个下游 TCP 连接可以服务的最大 HTTP 请求数，达到上限后关闭连接。等价于 Nginx 的 [`keepalive_requests`](https://nginx.org/en/docs/http/ngx_http_core_module.html#keepalive_requests)。

- **Per-connection**：每个 TCP 连接有独立的计数器，非全局限制
- **仅 HTTP/1.1**：HTTP/2 多路复用不受此限制
- **默认 1000**：与 Nginx 一致。设为 `0` 可禁用限制

**作用**：防止单连接长期占用导致的内存累积和负载不均衡。

### spec.httpTimeout

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| client.readTimeout | duration | 60s | 客户端读超时 |
| client.writeTimeout | duration | 60s | 客户端写超时 |
| client.keepaliveTimeout | duration | 75s | HTTP keepalive 超时 |
| backend.defaultConnectTimeout | duration | 5s | 后端连接超时 |
| backend.defaultRequestTimeout | duration | 60s | 后端请求超时 |
| backend.defaultIdleTimeout | duration | 300s | 后端连接池空闲超时 |
| backend.defaultMaxRetries | integer | 3 | 最大重试次数 |

### 完整示例

```yaml
apiVersion: edgion.io/v1alpha1
kind: EdgionGatewayConfig
metadata:
  name: production-config
spec:
  server:
    threads: 4
    workStealing: true
    gracePeriodSeconds: 30
    upstreamKeepalivePoolSize: 256
    downstreamKeepaliveRequestLimit: 1000
  httpTimeout:
    client:
      readTimeout: 60s
      writeTimeout: 60s
      keepaliveTimeout: 75s
    backend:
      defaultConnectTimeout: 5s
      defaultRequestTimeout: 60s
      defaultIdleTimeout: 300s
```

## 相关文档

- [Gateway 总览](./overview.md)
