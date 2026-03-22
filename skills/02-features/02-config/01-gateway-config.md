---
name: gateway-config
description: edgion-gateway TOML 配置完整 Schema。
---

# Gateway TOML 配置 Schema

> 文件路径默认：`config/edgion-gateway.toml`，通过 `--config-file` 指定。

## 看哪些文件

- 样例配置：`config/edgion-gateway.toml`
- 配置结构：`src/core/gateway/cli/config.rs`
- 启动逻辑：`src/core/gateway/cli/mod.rs`
- work_dir：`src/types/work_dir.rs`
- 通用日志结构：`src/types/observe.rs`, `src/types/output.rs`

## 完整 Schema

```toml
# 工作目录
work_dir = "."

[gateway]
server_addr = "http://127.0.0.1:50051"   # 必填：Controller gRPC 地址
admin_listen = "0.0.0.0:5900"            # Admin API（当前固定 :5900）

[logging]
# 系统日志（非业务日志）
log_dir = "logs"
log_prefix = "edgion-gateway"
log_level = "info"                        # 支持模块过滤：info,edgion::core::gateway::routes=debug
json_format = false
console = true
buffer_size = 10000

[server]
threads = 0                               # 0 = CPU 核心数
work_stealing = true                      # Tokio work-stealing
grace_period_seconds = 30                 # 优雅关闭等待
graceful_shutdown_timeout_seconds = 10    # 关闭超时
upstream_keepalive_pool_size = 128        # 上游连接池大小
downstream_keepalive_request_limit = 1000 # 下游每连接最大请求数（0=无限）
# error_log = "logs/pingora_error.log"    # Pingora 内部错误日志

# ─── 业务日志 ───
# 每种日志独立开关和路径，logging.log_dir 不影响这些路径

[access_log]
enabled = true
[access_log.output.localFile]
path = "logs/edgion_access.log"
# queue_size = 80000                      # 默认 cpu_cores * 10000
# [access_log.output.localFile.rotation]
# strategy = "daily"                      # daily | hourly | never | { Size = 104857600 }
# max_files = 7
# check_interval_secs = 30

[ssl_log]
enabled = true
[ssl_log.output.localFile]
path = "logs/ssl.log"

[tcp_log]
enabled = false
[tcp_log.output.localFile]
path = "logs/tcp_access.log"

[tls_log]
enabled = true
[tls_log.output.localFile]
path = "logs/tls_access.log"

[udp_log]
enabled = false
[udp_log.output.localFile]
path = "logs/udp_access.log"

# ─── 全局插件配置 ───

[rate_limit]
default_estimator_slots_k = 64           # CMS 默认精度（× 64KB 内存）
max_estimator_slots_k = 1024             # CMS 最大精度上限
gateway_instance_count = 1               # 静态实例数（无 Controller 推送时的 fallback）
```

## 字段详解

### [gateway]

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `server_addr` | `String` | **必填** | Controller gRPC 地址 |
| `admin_listen` | `String` | — | Admin API 地址（**注意**：当前代码固定 `:5900`，此配置暂不生效） |

> 注意：如果你改了 `gateway.admin_listen` 但端口没变，这不是你操作有误，而是当前实现还没接上这个字段。Gateway Admin API 当前固定 `5900`，Gateway Metrics API 当前固定 `5901`。

### [server]

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `threads` | `usize` | CPU 核心数 | Pingora 工作线程数 |
| `work_stealing` | `bool` | `true` | Tokio 任务窃取 |
| `grace_period_seconds` | `u64` | `30` | 优雅关闭等待秒数 |
| `graceful_shutdown_timeout_seconds` | `u64` | `10` | 关闭超时秒数 |
| `upstream_keepalive_pool_size` | `usize` | `128` | 上游 keepalive 连接池大小 |
| `downstream_keepalive_request_limit` | `u32` | `1000` | 下游每连接最大请求数（0=无限） |
| `error_log` | `String?` | — | Pingora 内部错误日志路径 |

> 注意：这些是进程级默认行为。如果你是在调 GatewayClass 级别的 server 行为，应该转去看 `EdgionGatewayConfig`。

### 业务日志 Schema

所有业务日志（access_log / ssl_log / tcp_log / tls_log / udp_log）共享相同的 Schema：

```yaml
LogConfig:
  enabled: bool                    # 是否启用
  output:
    localFile:                     # 当前唯一输出类型
      path: String                 # 日志文件路径（相对 work_dir）
      queue_size: usize?           # 写入队列大小（默认 cpu_cores × 10000）
      rotation:                    # 轮转配置（可选）
        strategy: RotationStrategy # daily | hourly | never | { Size: u64 }
        max_files: usize           # 保留文件数（0=无限）
        check_interval_secs: u64   # 轮转检查间隔（默认 30s）
```

**RotationStrategy 值**:
| 值 | 说明 |
|----|------|
| `"daily"` | 每日零点轮转 |
| `"hourly"` | 每小时轮转 |
| `"never"` | 不轮转 |
| `{ Size = 104857600 }` | 按文件大小轮转（字节，默认 100MB） |

**重要**：`[logging].log_dir` 只影响系统日志路径，不影响业务日志路径。业务日志路径在各自的 `output.localFile.path` 中独立配置。

### [rate_limit]

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `default_estimator_slots_k` | `usize` | `64` | CMS 默认 slot 数（×1K），内存 ≈ slots_k × 64KB |
| `max_estimator_slots_k` | `usize` | `1024` | CMS 最大 slot 数上限 |
| `gateway_instance_count` | `u32` | `1` | Gateway 实例数（集群限流分母，无 Controller 时的静态 fallback） |

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
