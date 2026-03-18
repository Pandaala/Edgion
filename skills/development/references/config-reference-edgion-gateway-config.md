# EdgionGatewayConfig 参考

> 这是 GatewayClass 级别的运行时默认配置，不是进程级 TOML。

## 什么时候改这一层

改这里，而不是改 Gateway TOML 的场景：

- 某个 GatewayClass 想要自己的 server / timeout / security 默认值
- 想配置全局插件引用
- 想开启 preflight policy
- 想把 real IP / ReferenceGrant 校验作为 GatewayClass 级策略

入口关系：

`GatewayClass.spec.parametersRef` -> `EdgionGatewayConfig`

示例文件：
- `examples/test/conf/base/EdgionGatewayConfig.yaml`

代码定义：
- `src/types/resources/edgion_gateway_config.rs`

## `spec` 顶层字段

| 字段 | 作用 |
|------|------|
| `server` | GatewayClass 级 server 默认值 |
| `httpTimeout` | client/backend timeout 默认值 |
| `maxRetries` | 全局默认最大重试次数 |
| `realIp` | 真实客户端 IP 提取策略 |
| `securityProtect` | 安全保护项，如 XFF 限制、SNI/Host 一致性 |
| `globalPluginsRef` | 所有 route 之前执行的全局插件引用 |
| `preflightPolicy` | 预检请求处理策略 |
| `enableReferenceGrantValidation` | 是否启用跨命名空间引用校验 |

## `spec.server`

| 字段 | 默认值 |
|------|--------|
| `threads` | CPU 核数 |
| `workStealing` | `true` |
| `gracePeriodSeconds` | `30` |
| `gracefulShutdownTimeoutS` | `10` |
| `upstreamKeepalivePoolSize` | `128` |
| `errorLog` | 无 |
| `enableCompression` | `false` |
| `downstreamKeepaliveRequestLimit` | `1000` |

## `spec.httpTimeout`

### `client`

| 字段 | 默认值 |
|------|--------|
| `readTimeout` | `60s` |
| `writeTimeout` | `60s` |
| `keepaliveTimeout` | `75s` |

### `backend`

| 字段 | 默认值 |
|------|--------|
| `defaultConnectTimeout` | `5s` |
| `defaultRequestTimeout` | `60s` |
| `defaultIdleTimeout` | `300s` |
| `defaultMaxRetries` | `3` |

## `spec.realIp`

来自 `RealIpConfig`，最常用字段：

| 字段 | 默认值 | 说明 |
|------|--------|------|
| `trustedIps` | 空 | 受信代理列表，不能为空才真正生效 |
| `realIpHeader` | `X-Forwarded-For` | 读取真实 IP 的 header |
| `recursive` | `true` | 是否按 nginx 风格从右向左跳过受信代理 |

## `spec.securityProtect`

| 字段 | 默认值 | 说明 |
|------|--------|------|
| `xForwardedForLimit` | `200` | XFF Header 最大长度 |
| `requireSniHostMatch` | `true` | HTTPS 请求是否要求 SNI 与 Host 一致 |
| `fallbackSni` | 无 | 客户端没带 SNI 时的 fallback |
| `tlsProxyLogRecord` | `true` | 是否记录 TLS proxy 连接日志 |

## `spec.globalPluginsRef`

数组元素结构：

| 字段 | 说明 |
|------|------|
| `name` | `EdgionPlugins` 资源名 |
| `namespace` | 可选，默认按实现处理 |

这层插件会在 route-level plugin 之前执行。

## `spec.preflightPolicy`

| 字段 | 默认值 | 说明 |
|------|--------|------|
| `mode` | `cors-standard` | `cors-standard` 或 `all-options` |
| `statusCode` | `204` | 没有 CORS 插件时的默认响应码 |

## `spec.enableReferenceGrantValidation`

| 字段 | 默认值 | 说明 |
|------|--------|------|
| `enableReferenceGrantValidation` | `false` | 是否在这层开启跨命名空间引用校验 |

## 什么时候不要改这一层

不要用 `EdgionGatewayConfig` 解决这些问题：

- Gateway 进程连哪个 Controller：那是 Gateway TOML 的 `server_addr`
- Gateway Admin / Metrics 端口：这是进程启动层
- Controller 用 file_system 还是 kubernetes：这是 Controller TOML 的 `conf_center`

## 相关

- [../04-config-reference.md](../04-config-reference.md)
- [config-reference-gateway.md](config-reference-gateway.md)
