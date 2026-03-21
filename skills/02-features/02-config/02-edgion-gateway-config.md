---
name: edgion-gateway-config-crd
description: EdgionGatewayConfig CRD Schema：GatewayClass 级别的运行时配置。
---

# EdgionGatewayConfig CRD Schema

> API Group: `edgion.io/v1alpha1` | Scope: Cluster | 通过 GatewayClass.spec.parametersRef 引用

## 关联方式

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
    name: default-config
```

## 完整 Schema

```yaml
apiVersion: edgion.io/v1alpha1
kind: EdgionGatewayConfig
metadata:
  name: default-config
spec:
  # ─── Pingora Server 配置 ───
  server:
    threads: 0                              # uint32, 默认 CPU 核心数
    workStealing: true                      # bool
    gracePeriodSeconds: 30                  # uint64
    gracefulShutdownTimeoutS: 10            # uint64
    upstreamKeepalivePoolSize: 128          # uint32
    errorLog: ""                            # string, 可选
    enableCompression: false                # bool, 下游响应压缩
    downstreamKeepaliveRequestLimit: 1000   # uint32, 0=无限

  # ─── HTTP 超时 ───
  httpTimeout:
    client:
      readTimeout: "60s"                    # Duration string
      writeTimeout: "60s"
      keepaliveTimeout: "75s"
    backend:
      defaultConnectTimeout: "5s"
      defaultRequestTimeout: "60s"
      defaultIdleTimeout: "300s"
      defaultMaxRetries: 3                  # uint32

  # ─── 最大重试 ───
  maxRetries: 3                             # uint32, 上游连接最大重试次数

  # ─── 真实 IP 提取 ───
  realIp:
    trustedIps: []                          # Vec<String>, 可信代理 IP/CIDR 列表
    realIpHeader: "X-Forwarded-For"         # string, 提取真实 IP 的请求头
    # recursive: 从右向左遍历 header，跳过 trustedIps，取第一个非信任 IP

  # ─── 安全防护 ───
  securityProtect:
    xForwardedForLimit: 200                 # usize, XFF 最大字节数
    requireSniHostMatch: true               # bool, HTTPS 要求 SNI 和 Host 匹配
    fallbackSni: ""                         # string?, 客户端无 SNI 时的备用值
    tlsProxyLogRecord: true                 # bool, 记录 TLS 代理连接日志

  # ─── 全局插件 ───
  globalPluginsRef:                         # Vec<PluginReference>, 应用于所有路由
    - name: "global-cors"
      namespace: "edgion-system"            # 默认 "default"

  # ─── Preflight 策略 ───
  preflightPolicy:
    mode: "cors-standard"                   # "cors-standard" | "all-options"
    statusCode: 204                         # uint16, 无 CORS 插件时的响应码

  # ─── ReferenceGrant 校验 ───
  enableReferenceGrantValidation: false     # bool
```

## spec 字段详解

### spec.server

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `threads` | `u32` | CPU 核心数 | Pingora 工作线程 |
| `workStealing` | `bool` | `true` | 任务窃取 |
| `gracePeriodSeconds` | `u64` | `30` | 优雅关闭等待 |
| `gracefulShutdownTimeoutS` | `u64` | `10` | 关闭超时 |
| `upstreamKeepalivePoolSize` | `u32` | `128` | 上游连接池 |
| `errorLog` | `String?` | — | Pingora 错误日志 |
| `enableCompression` | `bool` | `false` | 下游响应压缩 |
| `downstreamKeepaliveRequestLimit` | `u32` | `1000` | 下游每连接最大请求数 |

### spec.httpTimeout.client

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `readTimeout` | `Duration` | `60s` | 读取客户端请求超时 |
| `writeTimeout` | `Duration` | `60s` | 写入客户端响应超时 |
| `keepaliveTimeout` | `Duration` | `75s` | HTTP keepalive 超时 |

### spec.httpTimeout.backend

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `defaultConnectTimeout` | `Duration` | `5s` | 上游连接超时 |
| `defaultRequestTimeout` | `Duration` | `60s` | 总请求超时（含重试） |
| `defaultIdleTimeout` | `Duration` | `300s` | 连接池空闲超时 |
| `defaultMaxRetries` | `u32` | `3` | 最大重试次数 |

### spec.realIp

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `trustedIps` | `Vec<String>` | `[]` | 可信代理 IP/CIDR 列表 |
| `realIpHeader` | `String` | `X-Forwarded-For` | 真实 IP 提取的请求头名称 |

提取逻辑：从右向左遍历 `realIpHeader`，跳过 `trustedIps` 中的地址，取第一个非信任 IP。

### spec.securityProtect

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `xForwardedForLimit` | `usize` | `200` | X-Forwarded-For 最大字节数 |
| `requireSniHostMatch` | `bool` | `true` | HTTPS 要求 SNI 和 Host header 匹配 |
| `fallbackSni` | `String?` | — | 客户端未提供 SNI 时的备用主机名 |
| `tlsProxyLogRecord` | `bool` | `true` | 记录 TLS 代理连接日志 |

### spec.preflightPolicy

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `mode` | `String` | `cors-standard` | `cors-standard`：OPTIONS + Origin + ACRM；`all-options`：所有 OPTIONS 请求 |
| `statusCode` | `u16` | `204` | 无 CORS 插件时的 preflight 响应状态码 |

### spec.globalPluginsRef

```yaml
globalPluginsRef:
  - name: String        # EdgionPlugins 资源名称
    namespace: String?   # 默认 "default"
```

全局插件应用于所有路由，在路由级别插件之前执行。

## 与 TOML 配置的区别

| 配置项 | TOML | EdgionGatewayConfig |
|--------|------|---------------------|
| 作用域 | 单个进程实例 | 整个 GatewayClass |
| 变更方式 | 重启进程 | 动态生效（CRD 更新） |
| 适合 | 进程连接参数、日志路径 | 业务级超时、安全策略、全局插件 |

详细参考见 [references/config-reference-edgion-gateway-config.md](references/config-reference-edgion-gateway-config.md)。
