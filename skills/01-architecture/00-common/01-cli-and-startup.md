---
name: cli-and-startup
description: 三个 bin 共同遵守的命令行约定、工作目录结构、配置文件路径规范、日志初始化、信号处理。
---

# 统一命令行与配置约定

## 入口点模式

三个 bin 都采用**薄入口点**（thin entry point）模式 — `src/bin/*.rs` 只做最少的工作：

| bin | 入口逻辑 |
|-----|---------|
| edgion-controller | 安装 rustls 加密提供程序 → `parse_args()` → `run()` |
| edgion-gateway | 安装 rustls 加密提供程序 → `parse_args()` → `run()`（普通 `fn main()`，自管理 Tokio 运行时） |
| edgion-ctl | `Cli::parse()` → `cli.run().await`（单线程 Tokio） |

## 命令行参数

### 共享标志

| 标志 | Controller | Gateway | ctl | 默认值 |
|------|:---------:|:-------:|:---:|--------|
| `--work-dir` / `-w` | ✓ | ✓ | ✗ | `"."` |
| `--config-file` / `-c` | ✓ | ✓ | ✗ | `config/edgion-{type}.toml` |
| `--log-dir` | ✓ | ✓ | ✗ | `logs` |
| `--log-level` | ✓ | ✓ | ✗ | `info` |
| `--json-format` | ✓ | ✓ | ✗ | `false` |
| `--console` | ✓ | ✓ | ✗ | `true` |
| `--test-mode` | ✓ | ✗ | ✗ | `false` |

### Controller 专用

| 标志 | 默认值 | 说明 |
|------|--------|------|
| `--grpc-listen` | `0.0.0.0:50051` | gRPC 监听地址 |
| `--admin-listen` | `0.0.0.0:5800` | Admin API 地址 |
| `--conf-dir` | — | FileSystem 模式的配置目录 |

### Gateway 专用

| 标志 | 默认值 | 说明 |
|------|--------|------|
| `--server-addr` | （必需） | Controller gRPC 地址 |
| `--admin-listen` | — | Admin API 地址 |
| `--threads` | CPU 核心数 | Pingora 工作线程 |
| `--work-stealing` | `true` | 任务窃取 |
| `--grace-period` | `30s` | 优雅关闭期 |
| `--graceful-shutdown-timeout` | `10s` | 关闭超时 |
| `--upstream-keepalive-pool-size` | `128` | 上游连接池 |
| `--integration-testing-mode` | `false` | 集成测试模式 |

### ctl 专用

| 标志 | 默认值 | 说明 |
|------|--------|------|
| `--target` / `-t` | `center` | 目标 API（center/server/client） |
| `--server` | — | 服务器地址（HTTP） |
| `--socket` | — | Unix socket 路径 |
| `-f, --file` | — | 文件/目录路径（apply/delete） |
| `-n, --namespace` | — | 命名空间过滤 |
| `-o, --output` | `table` | 输出格式（table/json/yaml/wide） |

## 配置文件

### 加载优先级（高 → 低）

```
1. 命令行参数（CLI flags）
2. 环境变量（EDGION_WORK_DIR）
3. 配置文件（TOML）
4. 代码内默认值
```

### Controller 配置示例

```toml
[server]
grpc_listen = "0.0.0.0:50051"
admin_listen = "0.0.0.0:5800"

[logging]
log_dir = "logs"
log_prefix = "edgion-controller"
log_level = "info"
json_format = false
console = true

[conf_center]
type = "file_system"  # 或 "kubernetes"
conf_dir = "examples/test/conf"

[conf_sync]
default_capacity = 200
no_sync_kinds = ["ReferenceGrant", "Secret"]
```

### Gateway 配置示例

```toml
[gateway]
server_addr = "http://127.0.0.1:50051"

[logging]
log_dir = "logs"
log_prefix = "edgion-gateway"
log_level = "debug,pingora_proxy=error,pingora_core=error"

[server]
threads = 4
work_stealing = true
grace_period_seconds = 30
graceful_shutdown_timeout_seconds = 10

[access_log]
enabled = true
[access_log.output.localFile]
path = "logs/edgion_access.log"
```

## 工作目录

### WorkDir 结构（`src/types/work_dir.rs`）

```
work_dir/
├── logs/              # 日志文件
├── runtime/           # 运行时状态
└── config/            # 配置文件
```

### 初始化流程

1. **路径确定**：CLI `--work-dir` > 环境变量 `EDGION_WORK_DIR` > 配置文件 > 默认 `"."`
2. **规范化**：空字符串或 `.` → `std::env::current_dir()` 转绝对路径；符号链接通过 `canonicalize()` 解析
3. **验证创建**：检查目录存在或可创建、测试写权限（临时文件）、创建子目录

### 全局访问

```rust
init_work_dir(base);          // 启动时调用一次
work_dir();                   // 获取全局实例
wd.resolve("logs/app.log");   // 相对路径 → base/logs/app.log
wd.resolve("/var/log/app.log"); // 绝对路径 → 保持不变
```

## 日志初始化

### 共享日志函数（`src/core/gateway/observe/logs/sys_log.rs`）

Controller 和 Gateway 都调用同一个 `init_logging()` 函数：

```rust
pub async fn init_logging(config: LogConfig) -> Result<WorkerGuard>
```

**初始化步骤**：
1. 创建日志目录
2. 创建文件追加器（固定文件名，类 nginx 风格，外部 logrotate 处理轮转）
3. 包装为非阻塞 appender（后台 OS 线程）
4. 构建 tracing subscriber（JSON 格式或纯文本 + 可选控制台彩色输出）
5. 返回 `WorkerGuard`（必须保活）

### Gateway 额外日志

Gateway 除系统日志外，还初始化多个专用协议日志：

```rust
init_access_logger(&config.access_log);
init_ssl_logger(&config.ssl_log);
init_tcp_logger(&config.tcp_log);
init_tls_logger(&config.tls_log);
init_udp_logger(&config.udp_log);
```

## 信号处理

### Controller — ShutdownHandle

位置：`src/core/controller/conf_mgr/sync_runtime/shutdown.rs`

```rust
pub struct ShutdownHandle {
    inner: Arc<ShutdownController>,
    signal: ShutdownSignal,
}
```

- 使用 `tokio::signal` 监听 SIGTERM 和 SIGINT
- `tokio::select!` 等待任一信号后触发 `shutdown()`
- 传递给 ConfMgr 和所有服务，实现协调关闭

### Gateway — Pingora 内置

Gateway 无显式信号处理，依赖 Pingora 的内置优雅关闭：
- `grace_period_seconds`（默认 30s）
- `graceful_shutdown_timeout_seconds`（默认 10s）

### ctl — 不需要

ctl 是同步 CLI 工具，命令完成即退出。

## 全局配置存储

| 全局变量 | 位置 | 用途 |
|---------|------|------|
| `CONTROLLER_CONFIG` | `src/core/common/config/mod.rs` | Controller 全局配置（`OnceLock`） |
| `GATEWAY_INSTANCE_COUNT` | 同上 | Gateway 集群实例数（`AtomicU32`，用于集群级速率限制） |
| `GLOBAL_TEST_MODE` | 同上 | 测试模式标志（强制 endpoint_mode=Both） |
| `GLOBAL_INTEGRATION_TESTING_MODE` | 同上 | 集成测试模式（启用 AccessLog 存储 + 指标测试数据） |
