---
name: controller-startup-shutdown
description: edgion-controller 启动与关闭流程：CLI 入口、初始化序列、ShutdownHandle 架构、优雅关闭。
---

# Controller 启动与关闭

## 入口点

二进制入口在 `src/bin/edgion_controller.rs`，启动 `#[tokio::main(flavor = "multi_thread")]` 多线程运行时，执行以下两步：

```text
EdgionControllerCli::parse_args()   // clap 解析命令行
    └─► cli.run().await              // 主流程
```

`EdgionControllerCli` 定义在 `src/core/controller/cli/mod.rs`，通过 `#[derive(Parser)]` 扁平嵌入 `EdgionControllerConfig`。

## 启动序列（`run()` 方法）

`run()` 按以下顺序执行，每一步依赖前一步完成：

| 步骤 | 动作 | 说明 |
|------|------|------|
| 1 | `EdgionControllerConfig::load()` | 合并 CLI、环境变量、配置文件参数 |
| 2 | `init_controller_config()` / `init_global_test_mode()` | 初始化全局配置和测试模式标志 |
| 3 | `init_environment()` | 初始化工作目录（work_dir 优先级：CLI > ENV > Config > 默认 `.`）、日志系统 |
| 4 | `ShutdownHandle::new()` | 创建关闭句柄，spawn 后台任务等待 SIGINT/SIGTERM |
| 5 | `ConfMgr::create()` | 创建配置管理器（此时 ConfigSyncServer 为 None） |
| 6 | spawn `ConfMgr::start_with_shutdown()` | 后台启动 ConfMgr 生命周期（leader 选举、watcher、link） |
| 7 | `load_schemas()` | 加载 CRD Schema（仅非 K8s 模式；K8s 模式由 API Server 校验） |
| 8 | `start_services()` | 启动 gRPC + Admin API，阻塞直到关闭 |

### init_environment 细节

- 工作目录优先级：CLI `--work-dir` > 环境变量 `EDGION_WORK_DIR` > 配置文件字段 > 默认 `.`
- 调用 `init_work_dir()` 初始化并 `validate()` 校验目录结构
- 通过 `init_logging()` 初始化 tracing 日志系统，返回 `WorkerGuard`（guard 必须持有到进程退出，否则日志丢失）

### load_schemas 细节

- **K8s 模式**：跳过加载，返回 `SchemaValidator::empty()`，校验由 K8s API Server 负责
- **非 K8s 模式**：从 `{work_dir}/config/crd` 目录加载 CRD 文件构建 `SchemaValidator`；加载失败或数量为 0 时 `process::exit(1)`

### start_services 细节

gRPC 和 Admin API 通过 `tokio::join!` 并发运行，各自持有独立的 `ShutdownSignal`：

```text
tokio::join!(
    grpc_server.serve_with_shutdown(grpc_addr, grpc_shutdown),
    serve_admin_api_with_shutdown(conf_mgr, schema_validator, admin_addr, admin_shutdown),
)
```

gRPC 服务器使用 Provider 模式（`ConfMgrProvider`）动态获取最新的 `ConfigSyncServer` 实例，支持 reload 后无需重启服务。在 `ConfigSyncServer` 就绪前，gRPC 返回 `UNAVAILABLE`。

## ShutdownHandle 架构

关闭机制定义在 `src/core/controller/conf_mgr/sync_runtime/shutdown.rs`，基于 `tokio::sync::watch` 通道实现广播：

```text
ShutdownHandle
├── inner: Arc<ShutdownController>     // 持有 watch::Sender<bool>
└── signal: ShutdownSignal             // 持有 watch::Receiver<bool>（可 Clone）
```

### 三个核心类型

| 类型 | 角色 | 关键方法 |
|------|------|----------|
| `ShutdownController` | 发送端，触发关闭 | `new()` → `(Self, ShutdownSignal)`；`shutdown()` 发送 `true` |
| `ShutdownSignal` | 接收端，可 Clone 分发给任意组件 | `is_shutdown()` 立即检查；`wait()` 异步等待 |
| `ShutdownHandle` | 对外接口，包装上述两者 | `signal()` 克隆接收端；`shutdown()` 触发；`wait_for_signals()` 监听 OS 信号 |

### 信号监听

`wait_for_signals()` 在 spawn 的后台任务中运行：

```text
tokio::select! {
    result = ctrl_c       => ...,   // tokio::signal::ctrl_c()  → SIGINT
    result = terminate    => ...,   // tokio::signal::unix::SignalKind::terminate() → SIGTERM
}
self.shutdown();   // 通过 watch 通道广播 true
```

非 Unix 平台上 SIGTERM 分支为 `std::future::pending()`，仅响应 SIGINT。

## 优雅关闭流程

关闭由 OS 信号或内部调用 `shutdown()` 触发，信号通过 watch 通道广播到所有持有 `ShutdownSignal` 的组件：

```text
SIGINT / SIGTERM
  │
  ▼
ShutdownHandle::wait_for_signals()
  │  self.shutdown() → watch::Sender 发送 true
  │
  ├─► gRPC server shutdown signal → tonic graceful_shutdown
  ├─► Admin API shutdown signal → axum graceful_shutdown
  └─► ConfMgr shutdown signal → 停止 watcher、ResourceController、worker
```

关闭顺序：

1. OS 信号被 `wait_for_signals()` 捕获
2. `watch::Sender` 发送 `true`，所有 `ShutdownSignal::wait()` 返回
3. gRPC 和 Admin API 各自的 shutdown future 完成，`tokio::join!` 解除阻塞
4. ConfMgr 收到信号后停止 watcher 和 worker 循环
5. `run()` 返回，`main()` 退出

### 设计要点

- **单一信号源**：CLI 创建唯一的 `ShutdownHandle`，通过 `clone()` 传递给 ConfMgr，避免重复注册信号处理器
- **广播语义**：`watch` 通道允许任意数量的接收端，每个组件独立响应
- **幂等**：多次调用 `shutdown()` 安全，`watch::Sender::send(true)` 幂等
- **日志 Guard**：`_log_guard` 在 `run()` 作用域内持有，确保关闭前所有日志刷盘

## 关键源文件

| 文件 | 职责 |
|------|------|
| `src/bin/edgion_controller.rs` | 二进制入口，tokio main |
| `src/core/controller/cli/mod.rs` | `EdgionControllerCli` 定义，`parse_args()` / `run()` |
| `src/core/controller/conf_mgr/sync_runtime/shutdown.rs` | `ShutdownHandle` / `ShutdownController` / `ShutdownSignal` |
| `src/core/controller/conf_mgr/manager.rs` | `ConfMgr::create()` / `start_with_shutdown()` |
| `src/core/controller/api/` | Admin API（Axum） |
| `src/core/controller/conf_sync/` | gRPC ConfigSyncServer |
