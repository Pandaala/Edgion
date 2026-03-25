---
name: gateway-observe
description: Gateway 可观测性：AccessLog 零拷贝 JSON、协议日志、Prometheus 指标、AccessLogStore 集成测试支持。
---

# Gateway 可观测性

> 可观测性模块提供访问日志、协议日志、Prometheus 指标三个维度的运行时洞察。

## Access Log

### AccessLogEntry

`AccessLogEntry<'a>` 是零拷贝的访问日志条目，所有字段引用 `EdgionHttpContext` 中的数据：

```rust
pub struct AccessLogEntry<'a> {
    pub timestamp: i64,
    pub request_info: &'a RequestInfo,
    pub match_info: Option<RouteMatchInfo<'a>>,  // HTTP 或 gRPC 路由匹配信息
    pub errors: &'a [EdgionStatus],
    pub backend_context: Option<&'a BackendContext>,
    pub stage_logs: &'a [StageLogs],             // 插件执行日志
    pub conn_est: Option<bool>,                   // 上游连接是否建立
    pub ctx: &'a HashMap<String, String>,         // 自定义上下文变量
}
```

- `RouteMatchInfo` 是 `untagged` 枚举，支持 HTTP（`MatchInfo`）和 gRPC（`GrpcMatchInfo`）两种匹配信息
- 通过 `AccessLogEntry::from_context(ctx)` 从 EdgionHttpContext 构建
- 序列化为 JSON 时使用 `serde` 的 `skip_serializing_if` 减少输出体积

### AccessLogger

AccessLogger 是日志分发器，将日志发送到第一个健康的 DataSender：

```rust
pub struct AccessLogger {
    senders: Vec<Box<dyn DataSender<String>>>,
}
```

- `register()` — 注册 DataSender（如 LocalFileWriter、Elasticsearch DataSender）
- `init()` — 初始化所有 sender
- `send()` — 遍历 senders，发送到第一个 `healthy()` 返回 true 的 sender

可配置输出目标：
- **LocalFile** — 本地文件（支持日志轮转）
- **LinkSys** — 外部系统（Elasticsearch、Redis 等）

## 协议日志

每种协议有独立的日志器，通过 `init_*_logger()` 初始化：

| 协议 | 初始化函数 | 日志条目 | 说明 |
|------|------------|----------|------|
| SSL | `init_ssl_logger()` | `log_ssl()` | SSL/TLS 握手日志 |
| TCP | `init_tcp_logger()` | `TcpLogEntry` / `log_tcp()` | TCP 连接日志 |
| TLS | `init_tls_logger()` | `log_tls()` | TLS 路由日志 |
| UDP | `init_udp_logger()` | `UdpLogEntry` / `log_udp()` | UDP 会话日志 |

日志基础设施：
- `create_async_logger()` — 创建异步日志器
- `create_sync_logger()` — 创建同步日志器
- `init_default()` / `init_logging()` — 初始化默认日志系统（`SysLogConfig`）
- `logs/buffer.rs` — 日志缓冲
- `logs/logger_factory.rs` — 日志器工厂

## Prometheus 指标

### 暴露端口

指标通过 `:5901` 端口暴露，Axum 路由：
- `GET /metrics` — Prometheus 格式指标
- `GET /health` — 健康检查端点

### 指标注册表

`GatewayMetrics` 全局单例，使用 `metrics` crate 注册以下指标：

| 指标名 | 类型 | 说明 |
|--------|------|------|
| `edgion_ctx_created_total` | Counter | 创建的上下文总数 |
| `edgion_ctx_active` | Gauge | 当前活跃上下文数 |
| `edgion_requests_total` | Counter | 请求总数 |
| `edgion_requests_failed_total` | Counter | 失败请求总数 |
| `edgion_access_log_dropped_total` | Counter | 丢弃的访问日志数 |
| `edgion_ssl_log_dropped_total` | Counter | 丢弃的 SSL 日志数 |
| `edgion_tcp_log_dropped_total` | Counter | 丢弃的 TCP 日志数 |
| `edgion_tls_log_dropped_total` | Counter | 丢弃的 TLS 日志数 |
| `edgion_udp_log_dropped_total` | Counter | 丢弃的 UDP 日志数 |
| `edgion_status_update_total` | Counter | K8s 状态更新总数 |
| `edgion_status_update_failed_total` | Counter | K8s 状态更新失败数 |
| `edgion_status_update_skipped_total` | Counter | K8s 状态更新跳过数 |
| `edgion_config_reload_signals_total` | Counter | 配置重载信号数 |
| `edgion_config_relist_total` | Counter | 配置重新列举数 |
| `edgion_backend_requests_total` | Counter | 后端请求总数（按后端分组） |
| `edgion_gateway_request_bytes_total` | Counter | 网关请求字节数 |
| `edgion_gateway_response_bytes_total` | Counter | 网关响应字节数 |
| `edgion_mirror_requests_total` | Counter | 镜像请求数 |
| `edgion_mirror_duration_ms` | Histogram | 镜像请求耗时 |
| `edgion_controller_connected_gateways` | Gauge | 已连接的网关实例数 |
| `edgion_controller_schema_validation_errors_total` | Counter | Schema 验证错误数 |

全局标签：`service=edgion-gateway`

辅助函数：
- `record_backend_request()` — 记录后端请求指标
- `record_mirror_metric()` — 记录镜像指标
- `status_group()` — 状态码分组

### 测试指标

`test_metrics.rs` 提供集成测试用的指标数据注入：
- `set_latency_test_data()` — 设置延迟测试数据
- `set_lb_test_data()` — 设置 LB 测试数据
- `set_retry_test_data()` — 设置重试测试数据

## AccessLogStore（集成测试模式）

仅在 `--integration-testing-mode` 下激活的内存日志存储：

```rust
pub struct AccessLogStore {
    entries: DashMap<String, StoredEntry>,  // trace_id -> JSON
    ttl: Duration,                          // 默认 5 分钟
    max_capacity: usize,                    // 默认 10,000 条
    total_stored: AtomicU64,
}
```

- 按 `trace_id` 索引，支持精确查询
- DashMap 提供无锁并发访问
- TTL 过期自动清理（每 100 次 store 触发一次）
- 容量满时先清理过期条目
- API：`store()`、`get()`、`delete()`、`list()`、`clear()`、`status()`
- 全局单例通过 `get_access_log_store()` 获取

## 目录布局

```
src/core/gateway/observe/
├── mod.rs                    # 模块导出
├── access_log/               # 访问日志
│   ├── entry.rs              # AccessLogEntry 定义（零拷贝）
│   ├── logger.rs             # AccessLogger（DataSender 分发）
│   └── mod.rs
├── access_log_store.rs       # AccessLogStore（集成测试内存存储）
├── logs/                     # 协议日志
│   ├── ssl_log.rs            # SSL 日志
│   ├── tcp_log.rs            # TCP 日志
│   ├── tls_log.rs            # TLS 日志
│   ├── udp_log.rs            # UDP 日志
│   ├── sys_log.rs            # 系统日志配置
│   ├── buffer.rs             # 日志缓冲
│   ├── logger_factory.rs     # 日志器工厂
│   └── mod.rs
└── metrics/                  # Prometheus 指标
    ├── api.rs                # Axum HTTP 端点（:5901）
    ├── registry.rs           # GatewayMetrics 定义
    ├── test_metrics.rs       # 测试用指标注入
    └── mod.rs
```
