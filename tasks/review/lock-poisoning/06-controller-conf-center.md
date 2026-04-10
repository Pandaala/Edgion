# Controller Conf Center — 锁中毒 Review

## 模块概述

本次 review 覆盖 Controller 端配置中心和处理器注册表三个文件：

- **FileSystemCenter** (`src/core/controller/conf_mgr/conf_center/file_system/center.rs`) — 文件系统模式的配置中心，管理 ConfigSyncServer 生命周期、Controller 任务句柄、关停信号和重载信号。
- **KubernetesCenter** (`src/core/controller/conf_mgr/conf_center/kubernetes/center.rs`) — K8s 模式的配置中心，结构与 FileSystemCenter 对称，额外支持 Leader Election 和 All-Serve HA 模式。
- **ProcessorRegistry** (`src/core/controller/conf_mgr/processor_registry.rs`) — 全局处理器注册表，按 kind 名称索引所有 ResourceProcessor，供跨资源 requeue、缓存就绪检查、WatchObj 收集使用。

三个文件全部使用 `std::sync::{Mutex, RwLock}`，所有获取锁的位置均以 `.unwrap()` 处理 `PoisonError`。项目已依赖 `parking_lot = "0.12"`（在 `Cargo.toml` L130），但这些模块未使用。

## 锁使用清单

### FileSystemCenter (file_system/center.rs)

| # | 行号 | 方法 | 锁类型 | 操作 | 当前处理 | 风险等级 |
|---|------|------|--------|------|----------|----------|
| F1 | 90 | `set_config_sync_server()` | `RwLock<Option<Arc<ConfigSyncServer>>>` | `.write().unwrap()` | panic | 中 |
| F2 | 108 | `set_shutdown_handle()` | `Mutex<Option<ShutdownHandle>>` | `.lock().unwrap()` | panic | 高 |
| F3 | 114 | `set_controller_handle()` | `Mutex<Option<JoinHandle<()>>>` | `.lock().unwrap()` | panic | 中 |
| F4 | 120 | `abort_controller()` | `Mutex<Option<JoinHandle<()>>>` | `.lock().unwrap()` | panic | 高 |
| F5 | 127 | `set_reload_tx()` | `Mutex<Option<mpsc::Sender<()>>>` | `.lock().unwrap()` | panic | 中 |
| F6 | 389 | `is_ready()` (trait impl) | `RwLock<Option<Arc<ConfigSyncServer>>>` | `.read().unwrap()` | panic | 高 |
| F7 | 394 | `config_sync_server()` (trait impl) | `RwLock<Option<Arc<ConfigSyncServer>>>` | `.read().unwrap()` | panic | 高 |
| F8 | 404 | `request_reload()` (trait impl) | `Mutex<Option<mpsc::Sender<()>>>` | `.lock().unwrap()` | panic | 高 |

### KubernetesCenter (kubernetes/center.rs)

| # | 行号 | 方法 | 锁类型 | 操作 | 当前处理 | 风险等级 |
|---|------|------|--------|------|----------|----------|
| K1 | 145 | `set_config_sync_server()` | `RwLock<Option<Arc<ConfigSyncServer>>>` | `.write().unwrap()` | panic | 中 |
| K2 | 163 | `set_shutdown_handle()` | `Mutex<Option<ShutdownHandle>>` | `.lock().unwrap()` | panic | 高 |
| K3 | 169 | `set_reload_tx()` | `Mutex<Option<mpsc::Sender<()>>>` | `.lock().unwrap()` | panic | 中 |
| K4 | 1008 | `is_ready()` (trait impl) | `RwLock<Option<Arc<ConfigSyncServer>>>` | `.read().unwrap()` | panic | 高 |
| K5 | 1013 | `config_sync_server()` (trait impl) | `RwLock<Option<Arc<ConfigSyncServer>>>` | `.read().unwrap()` | panic | 高 |
| K6 | 1023 | `request_reload()` (trait impl) | `Mutex<Option<mpsc::Sender<()>>>` | `.lock().unwrap()` | panic | 高 |

### ProcessorRegistry (processor_registry.rs)

| # | 行号 | 方法 | 锁类型 | 操作 | 当前处理 | 风险等级 |
|---|------|------|--------|------|----------|----------|
| P1 | 56 | `register()` | `RwLock<HashMap>` | `.write().unwrap()` | panic | 中 |
| P2 | 65 | `get()` | `RwLock<HashMap>` | `.read().unwrap()` | panic | 高 |
| P3 | 70 | `all_kinds()` | `RwLock<HashMap>` | `.read().unwrap()` | panic | 低 |
| P4 | 75 | `count()` | `RwLock<HashMap>` | `.read().unwrap()` | panic | 低 |
| P5 | 82 | `is_all_ready()` | `RwLock<HashMap>` | `.read().unwrap()` | panic | 高 |
| P6 | 89 | `not_ready_kinds()` | `RwLock<HashMap>` | `.read().unwrap()` | panic | 低 |
| P7 | 106 | `all_watch_objs()` | `RwLock<HashMap>` | `.read().unwrap()` | panic | 中 |
| P8 | 219 | `set_all_not_ready()` | `RwLock<HashMap>` | `.read().unwrap()` | panic | 中 |
| P9 | 226 | `clear_all()` | `RwLock<HashMap>` | `.read().unwrap()` | panic | 中 |
| P10 | 237 | `requeue_all()` | `RwLock<HashMap>` | `.read().unwrap()` | panic | 中 |
| P11 | 258 | `clear_processors()` | `RwLock<HashMap>` | `.write().unwrap()` | panic | 中 |

**合计：25 个 unwrap() 调用点**（FileSystemCenter 8 个，KubernetesCenter 6 个，ProcessorRegistry 11 个）。

## 问题分析

### 1. ProcessorRegistry 在半热路径上使用（风险：高）

`ProcessorRegistry::get()` (P2) 不仅在启动/重载时使用，还在 **reconciliation handler 的执行路径** 中被频繁调用：

- `route_utils::service_exists()` — 每次路由资源 reconcile 都调用 `registry.get("Service")`
- `route_utils` L449 — 调用 `registry.get("Gateway")`
- `HandlerContext::requeue()` — 跨资源 requeue 时调用 `registry.get(kind)`（经 `requeue_with_chain` 间接）
- `ref_grant/revalidation_listener.rs` — ReferenceGrant 重校验时多次调用 `registry.get()`

这些路径虽然不是 per-request 数据面热路径（gateway proxy），但它们是 **Controller reconciliation 循环的核心路径**。如果 `ProcessorRegistry` 的 RwLock 中毒，所有资源的 reconciliation 都将 panic，导致 Controller 完全丧失配置处理能力。

### 2. `is_ready()` 和 `config_sync_server()` 是健康检查路径（风险：高）

F6/F7 和 K4/K5 实现了 `CenterLifeCycle` trait 方法，被 Admin API 健康检查端点和 gRPC 请求路由调用：

- `is_ready()` 同时检查 `processor_registry.is_all_ready()`（P5）和 `config_sync_server.read().unwrap()`，任一中毒都会 panic
- `config_sync_server()` 被 gRPC 服务获取 ConfigSyncServer 实例时调用

如果 `config_sync_server` 的 RwLock 中毒，健康检查会 panic 而非返回 not-ready，可能导致 K8s 探针误判并反复重启 Pod。

### 3. Shutdown/Reload 信号路径中毒导致挂起（风险：高）

**shutdown_handle** (F2/K2)：
- `set_shutdown_handle()` 在 `start()` 入口处调用，如果此锁中毒，shutdown handle 无法存储，后续无法响应优雅关停信号。
- 不过在当前实现中，`shutdown_handle` 只写入一次且临界区极小（仅赋值），中毒概率极低。

**reload_tx** (F5/F8/K3/K6)：
- `request_reload()` 被 Admin API 调用。如果 `reload_tx` 的 Mutex 中毒，reload 请求将 panic 而非返回错误。
- 在 FileSystemCenter 中，cleanup 阶段调用 `set_reload_tx(None)` (L343)，如果此时 Mutex 已中毒（比如 `request_reload` 在另一个线程 panic），cleanup 也会 panic，可能导致 reload loop 卡死。

**controller_handle** (F3/F4，仅 FileSystemCenter)：
- `abort_controller()` 在 cleanup 路径中调用 (L340)。如果 `controller_handle` 的 Mutex 中毒，Controller 任务无法被 abort，造成资源泄漏。

### 4. 中毒实际发生的概率评估

所有临界区都非常短（仅赋值、读取、或 `HashMap::get/insert/clear`），内部没有复杂逻辑或外部调用，**临界区内 panic 的概率极低**。但：

- `std::sync` 锁的中毒可以由 **同一 Mutex/RwLock 的其他使用者** 的 panic 传播。如果某线程在持有锁时因其他原因 panic（如 OOM、栈溢出），后续所有尝试获取该锁的线程都会遇到 `PoisonError`。
- 在 Controller 场景下，panic 通常由 `tokio::spawn` 的 task 捕获，不会直接传播。但如果 panic 发生在同步代码持有 `std::sync` 锁期间（比如 reconciliation handler 中调用 `get()` 拿到读锁后在链式操作中 panic），锁就会中毒。

### 5. std::sync vs parking_lot 对比

| 维度 | `std::sync` | `parking_lot` |
|------|-------------|---------------|
| 中毒机制 | 有，返回 `PoisonError` | 无，panic 释放锁后状态自动恢复 |
| API | `.lock().unwrap()` / `.read().unwrap()` | `.lock()` / `.read()` — 直接返回 guard |
| 性能 | 一般 | 更优（更小的锁体积、更快的无竞争路径） |
| 项目依赖 | 已使用 | 已在 `Cargo.toml` 中依赖，gateway 模块已使用 |

## 最佳处理方案

### 方案：迁移到 `parking_lot`（推荐）

项目已依赖 `parking_lot = "0.12"`，且 gateway 模块已有使用先例。迁移可以一次性消除所有 25 个 unwrap 调用点的中毒风险。

**变更范围**：

1. **ProcessorRegistry** — 将 `use std::sync::RwLock` 替换为 `use parking_lot::RwLock`，删除所有 `.unwrap()`:
   ```rust
   // Before
   use std::sync::RwLock;
   self.processors.read().unwrap().get(kind).cloned()

   // After
   use parking_lot::RwLock;
   self.processors.read().get(kind).cloned()
   ```

2. **FileSystemCenter** — 将 `use std::sync::{Mutex, RwLock}` 替换为 `use parking_lot::{Mutex, RwLock}`，删除所有 `.unwrap()`。

3. **KubernetesCenter** — 同上。

**注意事项**：
- `parking_lot::Mutex` 的 `MutexGuard` 不是 `Send`（和 `std` 一样），但这些锁都在同步代码中短暂持有，不跨 `.await` 点，所以无影响。
- `parking_lot::RwLock` 的 guard 同样不跨 `.await`，验证无问题。
- `FileSystemCenter::abort_controller()` 在 `lock()` 后立即 `.take()` 并 `abort()`，无 `.await`，安全。

**不推荐的替代方案**：
- 将 `.unwrap()` 改为 `.unwrap_or_else(|e| e.into_inner())` —— 忽略中毒错误继续使用可能损坏的数据，在 `shutdown_handle` 等场景不安全。
- 将 `.unwrap()` 改为 `expect("msg")` —— 仅改善 panic 信息，不解决根本问题。

## 优先级

| 优先级 | 项目 | 理由 |
|--------|------|------|
| **P1** | ProcessorRegistry `get()` / `is_all_ready()` | reconciliation 核心路径 + 健康检查路径，中毒后 Controller 完全不可用 |
| **P1** | `config_sync_server` RwLock（两个 Center） | 健康检查和 gRPC 服务获取路径，中毒后 K8s 探针 panic |
| **P2** | `reload_tx` Mutex（两个 Center） | Admin API reload 路径，中毒后 reload 功能不可用 |
| **P2** | `controller_handle` Mutex（FileSystemCenter） | cleanup 路径，中毒后 Controller 任务泄漏 |
| **P3** | `shutdown_handle` Mutex（两个 Center） | 仅启动时写入一次，中毒概率极低 |
| **P3** | ProcessorRegistry 其余方法 (`count`, `all_kinds` 等) | 非关键路径，仅启动/重载时使用 |

**建议执行顺序**：三个文件一起迁移到 `parking_lot`，变更量小（约 25 行 unwrap 删除 + 3 行 import 修改），可在一个 PR 内完成。
