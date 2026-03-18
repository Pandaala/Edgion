# Gateway TOML 参考

> 适用于 `config/edgion-gateway.toml` 和 `EdgionGatewayConfig` 进程级部分。

## 看哪些文件

- 样例配置：`config/edgion-gateway.toml`
- 配置结构：`src/core/gateway/cli/config.rs`
- 启动逻辑：`src/core/gateway/cli/mod.rs`
- work_dir：`src/types/work_dir.rs`
- 通用日志结构：`src/types/observe.rs`, `src/types/output.rs`

## 顶层 section 速查

| Section | 作用 | 关键字段 |
|---------|------|---------|
| `work_dir` | 运行目录 | 相对日志和配置路径基准 |
| `[gateway]` | Gateway 进程外部连接 | `server_addr`, `admin_listen` |
| `[logging]` | system log | `log_dir`, `log_level`, `json_format`, `console` |
| `[server]` | Pingora worker / keepalive | `threads`, `work_stealing`, `grace_period_seconds` 等 |
| `[access_log]` | HTTP/gRPC Access Log | `enabled`, `output.localFile.*` |
| `[ssl_log]` | TLS 握手日志 | `enabled`, `output.localFile.*` |
| `[tcp_log]` | TCP 连接日志 | `enabled`, `output.localFile.*` |
| `[udp_log]` | UDP 会话日志 | `enabled`, `output.localFile.*` |
| `[tls_log]` | 代码支持，样例文件未显式配置 | 默认路径 `logs/tls_access.log` |
| `[rate_limit]` | RateLimit 全局配置 | `default_estimator_slots_k`, `max_estimator_slots_k`, `gateway_instance_count` |

## `[gateway]`

| 字段 | 说明 |
|------|------|
| `server_addr` | 必填级别的重要字段，Gateway 连接 Controller gRPC sync 的地址 |
| `admin_listen` | 配置结构里有，但当前启动逻辑仍固定使用 `5900`，目前不生效 |

重要现实约束：
- Gateway Admin API 当前固定 `5900`
- Gateway Metrics API 当前固定 `5901`

所以如果你改了 `gateway.admin_listen` 但端口没变，这不是你操作有误，而是当前实现还没接上这个字段。

## `[logging]`

这部分控制的是 Gateway 自己的 system log，而不是 Access/TCP/UDP/SSL 这些业务日志。

| 字段 | 说明 |
|------|------|
| `log_dir` | system log 目录 |
| `log_prefix` | 文件名前缀，默认 `edgion-gateway` |
| `log_level` | tracing 级别，可带目标过滤，如 `debug,pingora_proxy=error` |
| `json_format` | 是否输出 JSON |
| `console` | 是否同时输出 stdout |
| `buffer_size` | appender buffer |

## `[server]`

| 字段 | 说明 |
|------|------|
| `threads` | worker 线程数 |
| `work_stealing` | Tokio work stealing |
| `grace_period_seconds` | 优雅关闭宽限期 |
| `graceful_shutdown_timeout_seconds` | 优雅关闭超时 |
| `upstream_keepalive_pool_size` | 上游 keepalive 池大小 |
| `error_log` | Pingora error log 文件 |
| `downstream_keepalive_request_limit` | 单个下游连接可承载的 HTTP/1.1 请求上限 |

注意：
- 这些是进程级默认行为
- 如果你是在调 GatewayClass 级别的 server 行为，应该转去看 `EdgionGatewayConfig`

## Access / SSL / TCP / UDP / TLS 日志

这些 section 都走相同模型：

- `enabled`
- `output.localFile.path`
- `output.localFile.queue_size`
- `output.localFile.rotation`

`rotation.strategy` 支持：
- `"daily"`
- `"hourly"`
- `"never"`
- `{ size = 104857600 }`

### 一个容易误判的点

`logging.log_dir` 不会自动改写这些 section 里的 `output.localFile.path`。

也就是说：
- system log 看 `[logging]`
- access/ssl/tcp/udp/tls 日志看各自 section

## `[rate_limit]`

| 字段 | 默认值 | 说明 |
|------|--------|------|
| `default_estimator_slots_k` | `64` | 默认 CMS slot 数，单位 K |
| `max_estimator_slots_k` | `1024` | 允许的最大 CMS slot 数，单位 K |
| `gateway_instance_count` | `1` | controller 不可达时的静态实例数 fallback |

这个 section 影响的是所有 RateLimit 插件的全局默认和上限，不是单个插件实例局部参数。

## `integration_testing_mode`

这是 CLI flag，不是 TOML section：

```bash
./target/debug/edgion-gateway --config-file config/edgion-gateway.toml --integration-testing-mode
```

开启后会额外激活：
- Access Log Store
- 测试专用 metrics 标记
- `/api/v1/testing/*` 调试接口

## 当前项目里最常见的 Gateway 配置改动

- 改 Controller gRPC 地址：改 `[gateway].server_addr`
- 改 system log 噪音：改 `[logging].log_level`
- 开关业务日志：改 `[access_log]` / `[ssl_log]` / `[tcp_log]` / `[udp_log]`
- 调整 Pingora worker 和 keepalive：改 `[server]`
- 调整 RateLimit 默认精度：改 `[rate_limit]`

## 相关

- [../04-config-reference.md](../04-config-reference.md)
- [config-reference-edgion-gateway-config.md](config-reference-edgion-gateway-config.md)
- [../../testing/03-debugging.md](../../testing/03-debugging.md)
