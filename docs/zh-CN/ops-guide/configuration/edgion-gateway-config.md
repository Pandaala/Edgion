# EdgionGatewayConfig

EdgionGatewayConfig 是 Edgion 的 Cluster 作用域 CRD（`edgion.io/v1alpha1`），用于设置 GatewayClass 级别的运行时行为，通过 `GatewayClass.spec.parametersRef` 关联，更新后动态生效，无需重启进程。

## 与 Gateway TOML 的区别

| 配置层 | Gateway TOML | EdgionGatewayConfig |
|--------|-------------|---------------------|
| 作用域 | 单个 Gateway 进程 | 整个 GatewayClass |
| 变更生效方式 | 需要重启进程 | 动态生效 |
| 典型用途 | 进程连接参数、日志路径、限流精度 | 超时策略、安全防护、全局插件、真实 IP 提取 |

**何时用 TOML，何时用 EdgionGatewayConfig：**
- 改 Controller gRPC 地址 → Gateway TOML `[gateway].server_addr`
- 改 Admin / Metrics 端口 → Gateway TOML（进程启动层）
- 配置 HTTP 超时、真实 IP、全局插件 → EdgionGatewayConfig

## 关联方式

通过 GatewayClass 的 `parametersRef` 字段关联 EdgionGatewayConfig：

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GatewayClass
metadata:
  name: edgion
spec:
  controllerName: edgion.io/gateway-controller
  parametersRef:
    group: edgion.io
    kind: EdgionGatewayConfig
    name: default-config  # 对应下方 EdgionGatewayConfig 的 metadata.name
---
apiVersion: edgion.io/v1alpha1
kind: EdgionGatewayConfig
metadata:
  name: default-config   # Cluster 作用域，无需指定 namespace
spec:
  server:
    threads: 0
    workStealing: true
    gracePeriodSeconds: 30
    gracefulShutdownTimeoutS: 10
    upstreamKeepalivePoolSize: 128
    enableCompression: false
    downstreamKeepaliveRequestLimit: 1000

  httpTimeout:
    client:
      readTimeout: "60s"
      writeTimeout: "60s"
      keepaliveTimeout: "75s"
    backend:
      defaultConnectTimeout: "5s"
      defaultRequestTimeout: "60s"
      defaultIdleTimeout: "300s"

  maxRetries: 3

  realIp:
    trustedIps: ["10.0.0.0/8", "172.16.0.0/12"]
    realIpHeader: "X-Forwarded-For"
    recursive: true

  securityProtect:
    xForwardedForLimit: 200
    requireSniHostMatch: true
    fallbackSni: ""
    tlsProxyLogRecord: true

  globalPluginsRef:
    - name: "global-cors"
      namespace: "edgion-system"

  preflightPolicy:
    mode: "cors-standard"
    statusCode: 204

  enableReferenceGrantValidation: false
```

## 配置参数

### spec.server

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `threads` | `Integer` | CPU 核心数 | Pingora 工作线程数，`0` 表示自动检测 |
| `workStealing` | `Boolean` | `true` | 启用 Tokio 任务窃取 |
| `gracePeriodSeconds` | `Integer` | `30` | 优雅关闭期间等待连接结束的秒数 |
| `gracefulShutdownTimeoutS` | `Integer` | `10` | 优雅关闭超时秒数，超过后强制终止 |
| `upstreamKeepalivePoolSize` | `Integer` | `128` | 上游 keepalive 连接池大小 |
| `errorLog` | `String` | — | Pingora 内部错误日志文件路径，可选 |
| `enableCompression` | `Boolean` | `false` | 是否对下游响应启用压缩 |
| `downstreamKeepaliveRequestLimit` | `Integer` | `1000` | 下游每连接最大请求数，`0` 表示无限 |

### spec.httpTimeout.client

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `readTimeout` | `Duration` | `60s` | 读取客户端请求的超时时间 |
| `writeTimeout` | `Duration` | `60s` | 写入客户端响应的超时时间 |
| `keepaliveTimeout` | `Duration` | `75s` | HTTP keepalive 连接空闲超时 |

### spec.httpTimeout.backend

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `defaultConnectTimeout` | `Duration` | `5s` | 连接上游服务器的超时时间 |
| `defaultRequestTimeout` | `Duration` | `60s` | 完整请求（含重试）的总超时时间 |
| `defaultIdleTimeout` | `Duration` | `300s` | 连接池中连接的空闲超时 |

### spec.maxRetries

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `maxRetries` | `Integer` | `3` | 上游连接失败时的最大重试次数 |

### spec.realIp

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `trustedIps` | `Array<String>` | `[]` | 可信代理 IP 或 CIDR 列表 |
| `realIpHeader` | `String` | `X-Forwarded-For` | 从哪个请求头提取真实 IP |
| `recursive` | `Boolean` | `true` | 是否从右向左遍历 header，跳过可信代理，取第一个非信任 IP |

提取逻辑：从右向左遍历 `realIpHeader`，跳过 `trustedIps` 中的地址，取第一个非信任 IP。

### spec.securityProtect

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `xForwardedForLimit` | `Integer` | `200` | `X-Forwarded-For` 头部最大字节数，超过则截断 |
| `requireSniHostMatch` | `Boolean` | `true` | HTTPS listener 级别 421 Misdirected Request 检测，SNI 与 Host 不匹配时拒绝请求 |
| `fallbackSni` | `String` | — | 客户端未提供 SNI 时使用的备用主机名，可选 |
| `tlsProxyLogRecord` | `Boolean` | `true` | 是否记录 TLS 代理连接日志 |

### spec.globalPluginsRef

应用于该 GatewayClass 下所有路由的全局插件列表，在路由级插件之前执行。

```yaml
globalPluginsRef:
  - name: "global-cors"       # EdgionPlugins 资源名称
    namespace: "edgion-system" # 默认 "default"
```

### spec.preflightPolicy

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `mode` | `String` | `cors-standard` | `cors-standard`：仅拦截带 `Origin` + `ACRM` 的 OPTIONS 请求；`all-options`：拦截所有 OPTIONS 请求 |
| `statusCode` | `Integer` | `204` | 无 CORS 插件时 preflight 响应的状态码 |

### spec.enableReferenceGrantValidation

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `enableReferenceGrantValidation` | `Boolean` | `false` | 是否启用 ReferenceGrant 跨命名空间引用校验 |

## 常见配置场景

### 配置真实 IP 提取

当 Gateway 部署在负载均衡器后面时，配置可信代理 IP 以正确提取客户端真实 IP：

```yaml
spec:
  realIp:
    trustedIps:
      - "10.0.0.0/8"
      - "172.16.0.0/12"
      - "192.168.0.0/16"
    realIpHeader: "X-Forwarded-For"
    recursive: true
```

### 调整超时策略

针对慢接口场景放宽超时：

```yaml
spec:
  httpTimeout:
    client:
      readTimeout: "120s"
      writeTimeout: "120s"
      keepaliveTimeout: "75s"
    backend:
      defaultConnectTimeout: "10s"
      defaultRequestTimeout: "120s"
      defaultIdleTimeout: "300s"
```

### 挂载全局 CORS 插件

```yaml
spec:
  globalPluginsRef:
    - name: "global-cors"
      namespace: "edgion-system"
  preflightPolicy:
    mode: "cors-standard"
    statusCode: 204
```

### 开启下游响应压缩

```yaml
spec:
  server:
    enableCompression: true
```

### 关闭 SNI 主机名匹配检测

在某些特殊场景下，允许 SNI 与 Host 头不一致：

```yaml
spec:
  securityProtect:
    requireSniHostMatch: false
```

## 相关文档

- [Controller 配置](./controller-config.md)
- [Gateway 配置](./gateway-config.md)
- [GatewayClass 配置](../gateway/gateway-class.md)
- [Preflight Policy](../gateway/preflight-policy.md)
