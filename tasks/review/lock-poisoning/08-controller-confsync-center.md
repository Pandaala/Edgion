# Controller ConfSync, ACME & Center — 锁中毒 Review

## 模块概述

本次 review 涵盖三个模块组，分属两个二进制：

### Controller ConfSync（Controller 二进制）
- **cache.rs** (`src/core/controller/conf_sync/cache_server/cache.rs`)：`ServerCache<T>` 是 Controller 向 Gateway 同步配置的核心缓存，包含 `ready`、`store`、`watchers` 三把 `std::sync::RwLock`。每种资源类型（HTTPRoute、Gateway、EdgionTls 等）各有一个 `ServerCache<T>` 实例，是锁密度最高的文件（22 处 unwrap）。
- **client_registry.rs** (`src/core/controller/conf_sync/conf_server/client_registry.rs`)：跟踪已连接 Gateway 实例数，`RwLock<HashMap>` + 4 处 unwrap。
- **config_sync_server.rs** (`src/core/controller/conf_sync/conf_server/config_sync_server.rs`)：持有 `server_id`、`endpoint_mode`、`watch_objects` 三把 RwLock，15 处 unwrap。
- **grpc_server.rs** (`src/core/controller/conf_sync/conf_server/grpc_server.rs`)：仅在测试代码中有 `Mutex::lock().unwrap()`（`MockConfigSyncProvider`），生产代码无直接锁使用。

### Controller ACME Service（Controller 二进制）
- **service.rs** (`src/core/controller/services/acme/service.rs`)：全局单例 `ACME_SERVICE: Mutex<Option<...>>`，加上 Worker 内部 `processing: RwLock<HashSet>` 和 `retry_tracker: RwLock<HashMap>`。**全部使用 `unwrap_or_else(|e| e.into_inner())` 恢复策略**，是项目中唯一完整实施 poison recovery 的模块。

### Center 模块（Center 二进制）
- **aggregator** (`src/core/center/aggregator/mod.rs`)：`RwLock<HashMap<String, ControllerSnapshot>>`，7 处 unwrap。
- **commander** (`src/core/center/commander/mod.rs`)：`PendingCommandMap = Arc<Mutex<HashMap>>`，3 处 unwrap。
- **registry** (`src/core/center/fed_sync/registry/mod.rs`)：`RwLock<HashMap<String, ControllerSession>>`，8 处 unwrap。
- **server** (`src/core/center/fed_sync/server/mod.rs`)：`PendingListMap` 和 `PendingCommandMap`（均 `Arc<Mutex<HashMap>>`），3 处 unwrap。
- **scheduler** (`src/core/center/scheduler/mod.rs`)：使用共享的 `PendingListMap`，3 处 unwrap。

### Common
- **types.rs** (`src/core/common/conf_sync/types.rs` 行 81)：`WatchClient.last_send_time: Arc<std::sync::RwLock<Option<SystemTime>>>`，在 `cache.rs` 行 151 通过 `if let Ok(mut last_time) = last_send_time.write()` 静默忽略 poison。

---

## 锁使用清单

### Controller ConfSync — cache.rs

| # | 行号 | 方法 | 锁类型 | 当前处理 | 风险等级 |
|---|------|------|--------|----------|----------|
| 1 | 52 | `is_ready()` | RwLock read (ready) | `.unwrap()` | 高 |
| 2 | 58 | `set_not_ready()` | RwLock write (ready) | `.unwrap()` | 高 |
| 3 | 72 | `clear()` | RwLock write (store) | `.unwrap()` | 高 |
| 4 | 94 | `list_owned()` | RwLock read (store) | `.unwrap()` | 高 |
| 5 | 106 | `get_by_key()` | RwLock read (store) | `.unwrap()` | 中 |
| 6 | 111 | `list_keys()` | RwLock read (store) | `.unwrap()` | 中 |
| 7 | 134 | `start_watcher_task()` 循环内 | RwLock read (store) | `.unwrap()` | 高 |
| 8 | 218 | `watch()` | RwLock read (watchers) | `.unwrap()` | 中 |
| 9 | 226 | `watch()` 日志内 | RwLock read (ready) | `.unwrap()` | 中 |
| 10 | 249 | `watch()` | RwLock write (watchers) | `.unwrap()` | 中 |
| 11 | 274 | `watch()` 日志内 | RwLock read (watchers) | `.unwrap()` | 低 |
| 12 | 303 | `push_event()` | RwLock write (store) | `.unwrap()` | 高 |
| 13 | 321 | `list_json()` (WatchObj) | RwLock read (store) | `.unwrap()` | 高 |
| 14 | 377 | `is_ready()` (WatchObj) | RwLock read (ready) | `.unwrap()` | 高 |
| 15 | 381 | `set_ready()` (WatchObj) | RwLock write (ready) | `.unwrap()` | 高 |
| 16 | 385 | `set_not_ready()` (WatchObj) | RwLock write (ready) | `.unwrap()` | 高 |
| 17 | 389 | `clear()` (WatchObj) | RwLock write (store) | `.unwrap()` | 高 |
| 18 | 424 | `apply_change(InitAdd)` | RwLock write (store) | `.unwrap()` | 高 |
| 19 | 449 | `set_ready()` (CacheEventDispatch) | RwLock write (ready) | `.unwrap()` | 高 |

### Controller ConfSync — client_registry.rs

| # | 行号 | 方法 | 锁类型 | 当前处理 | 风险等级 |
|---|------|------|--------|----------|----------|
| 20 | 39 | `register()` | RwLock write | `.unwrap()` | 中 |
| 21 | 62 | `unregister()` | RwLock write | `.unwrap()` | 中 |
| 22 | 76 | `count()` | RwLock read | `.unwrap()` | 中 |

### Controller ConfSync — config_sync_server.rs

| # | 行号 | 方法 | 锁类型 | 当前处理 | 风险等级 |
|---|------|------|--------|----------|----------|
| 23 | 96 | `server_id()` | RwLock read (server_id) | `.unwrap()` | 高 |
| 24 | 109 | `regenerate_server_id()` | RwLock write (server_id) | `.unwrap()` | 中 |
| 25 | 114 | `set_endpoint_mode()` | RwLock write (endpoint_mode) | `.unwrap()` | 低 |
| 26 | 119 | `endpoint_mode()` | RwLock read (endpoint_mode) | `.unwrap()` | 低 |
| 27 | 126 | `register_watch_obj()` | RwLock write (watch_objects) | `.unwrap()` | 低 |
| 28 | 137 | `register_all()` | RwLock write (watch_objects) | `.unwrap()` | 低 |
| 29 | 150 | `get_watch_obj()` | RwLock read (watch_objects) | `.unwrap()` | 高 |
| 30 | 155 | `all_kinds()` | RwLock read (watch_objects) | `.unwrap()` | 低 |
| 31 | 166 | `list()` | RwLock read (watch_objects) | `.unwrap()` | 高 |
| 32 | 202 | `watch()` | RwLock read (watch_objects) | `.unwrap()` | 高 |
| 33 | 257 | `is_all_ready()` | RwLock read (watch_objects) | `.unwrap()` | 中 |
| 34 | 263 | `not_ready_kinds()` | RwLock read (watch_objects) | `.unwrap()` | 低 |
| 35 | 272 | `set_cache_ready_by_kind()` | RwLock read (watch_objects) | `.unwrap()` | 低 |
| 36 | 287 | `set_all_not_ready()` | RwLock read (watch_objects) | `.unwrap()` | 低 |
| 37 | 295 | `clear_all()` | RwLock read (watch_objects) | `.unwrap()` | 低 |

### Controller ACME — service.rs

| # | 行号 | 方法 | 锁类型 | 当前处理 | 风险等级 |
|---|------|------|--------|----------|----------|
| 38 | 63 | `start_acme_service()` | Mutex (ACME_SERVICE) | `.unwrap_or_else(\|e\| e.into_inner())` | 低（已恢复） |
| 39 | 101 | `stop_acme_service()` | Mutex (ACME_SERVICE) | `.unwrap_or_else(\|e\| e.into_inner())` | 低（已恢复） |
| 40 | 150 | `get_sender()` | Mutex (ACME_SERVICE) | `.unwrap_or_else(\|e\| e.into_inner())` | 低（已恢复） |
| 41 | 238 | `run()` ResourceChanged | RwLock write (retry_tracker) | `.unwrap_or_else(\|e\| e.into_inner())` | 低（已恢复） |
| 42 | 259 | `handle_resource_check()` | RwLock read (processing) | `.unwrap_or_else(\|e\| e.into_inner())` | 低（已恢复） |
| 43 | 286 | `handle_resource_check()` | RwLock write (retry_tracker) | `.unwrap_or_else(\|e\| e.into_inner())` | 低（已恢复） |
| 44 | 316 | `handle_resource_check()` | RwLock read (retry_tracker) | `.unwrap_or_else(\|e\| e.into_inner())` | 低（已恢复） |
| 45 | 354 | `handle_resource_check()` | RwLock write (processing) | `.unwrap_or_else(\|e\| e.into_inner())` | 低（已恢复） |
| 46 | 378 | spawned task (success) | RwLock write (retry_tracker) | `.unwrap_or_else(\|e\| e.into_inner())` | 低（已恢复） |
| 47 | 384 | spawned task (failure) | RwLock write (retry_tracker) | `.unwrap_or_else(\|e\| e.into_inner())` | 低（已恢复） |
| 48 | 432 | spawned task (cleanup) | RwLock write (processing) | `.unwrap_or_else(\|e\| e.into_inner())` | 低（已恢复） |

### Common — types.rs

| # | 行号 | 方法 | 锁类型 | 当前处理 | 风险等级 |
|---|------|------|--------|----------|----------|
| 49 | 81 (定义) / cache.rs:151 (使用) | `last_send_time` watcher 更新 | RwLock write | `if let Ok(mut last_time) = ...write()` 静默忽略 | 低（仅元数据） |

### Center — aggregator

| # | 行号 | 方法 | 锁类型 | 当前处理 | 风险等级 |
|---|------|------|--------|----------|----------|
| 50 | 44 | `set_controller_info()` | RwLock write | `.unwrap()` | 中 |
| 51 | 54 | `update_snapshot()` | RwLock write | `.unwrap()` | 中 |
| 52 | 71 | `mark_offline()` | RwLock write | `.unwrap()` | 中 |
| 53 | 83 | `evict_stale()` | RwLock write | `.unwrap()` | 中 |
| 54 | 90 | `list_all_keys()` | RwLock read | `.unwrap()` | 中 |
| 55 | 100 | `list_keys_by_cluster()` | RwLock read | `.unwrap()` | 中 |
| 56 | 111 | `controller_summaries()` | RwLock read | `.unwrap()` | 低 |

### Center — commander

| # | 行号 | 方法 | 锁类型 | 当前处理 | 风险等级 |
|---|------|------|--------|----------|----------|
| 57 | 45 | `send_command()` insert pending | Mutex (pending) | `.unwrap()` | 中 |
| 58 | 55 | `send_command()` 发送失败清理 | Mutex (pending) | `.unwrap()` | 中 |
| 59 | 62 | `send_command()` 超时清理 | Mutex (pending) | `.unwrap()` | 中 |
| 60 | 66 | `send_command()` channel 断开清理 | Mutex (pending) | `.unwrap()` | 中 |

### Center — registry

| # | 行号 | 方法 | 锁类型 | 当前处理 | 风险等级 |
|---|------|------|--------|----------|----------|
| 61 | 52 | `register()` | RwLock write | `.unwrap()` | 中 |
| 62 | 56 | `get_session()` | RwLock read | `.unwrap()` | 中 |
| 63 | 68 | `update_last_seen()` | RwLock write | `.unwrap()` | 中 |
| 64 | 74 | `mark_offline()` | RwLock write | `.unwrap()` | 中 |
| 65 | 88 | `online_controller_ids()` | RwLock read | `.unwrap()` | 中 |
| 66 | 97 | `online_senders()` | RwLock read | `.unwrap()` | 中 |
| 67 | 108 | `evict_stale()` | RwLock write | `.unwrap()` | 低 |

### Center — server (FederationGrpcServer)

| # | 行号 | 方法 | 锁类型 | 当前处理 | 风险等级 |
|---|------|------|--------|----------|----------|
| 68 | 198 | `sync()` ListResponse 处理 | Mutex (pending_lists) | `.unwrap()` | 中 |
| 69 | 204 | `sync()` CommandResponse 处理 | Mutex (pending_commands) | `.unwrap()` | 中 |

### Center — scheduler

| # | 行号 | 方法 | 锁类型 | 当前处理 | 风险等级 |
|---|------|------|--------|----------|----------|
| 70 | 19 | `send_list_requests()` insert | Mutex (pending) | `.unwrap()` | 中 |
| 71 | 40 | `send_list_requests()` 失败清理 | Mutex (pending) | `.unwrap()` | 中 |
| 72 | 72 | GC 任务 retain | Mutex (pending) | `.unwrap()` | 中 |

**总计：72 处锁操作**。其中 ACME service 的 11 处全部使用 poison recovery，cache.rs 的 1 处 `last_send_time` 使用 `if let Ok` 静默忽略，其余 60 处全部使用 `.unwrap()` 裸 panic。

---

## 问题分析

### 1. cache.rs：最高锁密度，级联风险最大

`ServerCache<T>` 有 3 把锁（`ready`、`store`、`watchers`），共 19 处 unwrap。关键问题：

**级联场景**：`store` 锁是核心。一旦 `push_event()` 或 `apply_change(InitAdd)` 的写路径 panic（例如 `store_guard.apply_event()` 内部 panic），`store` 锁被标记为 poisoned，随后所有读写操作全部 panic：
```
T1 (写): push_event() -> store.write().unwrap() -> apply_event() 内部 panic -> store 中毒
T2 (gRPC List): list_json() -> store.read().unwrap() -> PANIC
T3 (Watcher): start_watcher_task loop -> store.read().unwrap() -> PANIC (tokio task 退出)
T4 (gRPC Watch): watch() -> watchers.write().unwrap() -> OK（watchers 锁未中毒）
   ... 但新 watcher 的 task 仍会在 store.read() 时 panic
```

**watcher task 的特殊风险**：`start_watcher_task` 在 `tokio::spawn` 内循环调用 `store.read().unwrap()`（行 134）。store 中毒后，所有活跃 watcher task 会在下一次循环迭代时 panic，导致所有 Gateway 的 watch 流全部中断。每种资源类型独立有自己的 `ServerCache`，所以中毒只影响单个资源类型，不会跨类型传播。

**ready 锁的连带风险**：`ready` 锁独立于 `store`，但 `is_ready()` 出现在多个热路径中（`push_event` 的前置检查、`WatchObj::is_ready()`）。如果 `set_ready()`（行 381/449）的写路径 panic（虽然极不可能，因为操作只是 `*guard = true`），ready 中毒会阻塞后续所有 readiness 检查。

### 2. config_sync_server.rs：watch_objects 锁的放大效应

`watch_objects: RwLock<HashMap>` 保存所有资源类型的 WatchObj。`list()` 和 `watch()` 方法先获取 `watch_objects` 的读锁，再通过 WatchObj trait 调用 `ServerCache` 的方法（获取 store 的读锁）。这形成了**两级锁**：
- `watch_objects` 中毒：所有资源类型的 list/watch 全部失败
- 某个 `ServerCache.store` 中毒：只影响该资源类型

`watch_objects` 的写路径只在 `register_watch_obj()` / `register_all()` 中调用（初始化阶段），且操作简单（HashMap insert），实际 panic 概率极低。读路径的 unwrap 是主要风险点——一旦中毒，**所有资源类型的同步全部停止**。

### 3. ACME service.rs：poison recovery 的正确性分析

ACME 是本次 review 中**唯一全面实施 poison recovery 的模块**。分析其正确性：

**全局单例 `ACME_SERVICE: Mutex<Option<...>>`**：
- `start_acme_service()`、`stop_acme_service()`、`get_sender()` 三处全部使用 `unwrap_or_else(|e| e.into_inner())`
- 内部数据只是 `Option<AcmeServiceHandle>`，其中 `AcmeServiceHandle` 仅含 `mpsc::Sender`
- Recovery 语义正确：即使前一个持有者 panic，Sender 本身没有不变量被破坏

**Worker 内部 `processing: RwLock<HashSet>` 和 `retry_tracker: RwLock<HashMap>`**：
- 读路径和写路径全部使用 `unwrap_or_else(|e| e.into_inner())`，**覆盖完整且一致**
- 数据语义上是"尽力跟踪"：processing 集合用于去重，retry_tracker 用于计数。即使数据不一致（例如某 key 残留在 processing 中），后果仅是跳过一次检查或多一次重试，不会导致数据损坏
- ACME 流程的状态持久化在 K8s CRD 上，内存中的 processing/retry_tracker 丢失后通过 startup scan 恢复

**结论**：ACME 的 poison recovery 策略**设计正确且实施完整**。原因在于：
1. 所有路径（读/写）一致使用 recovery，不存在"写恢复但读 panic"的矛盾
2. 锁保护的数据是可丢失的瞬态状态，真正的持久状态在 K8s
3. Recovery 后的数据不一致影响有界（最多重复处理一个 ACME 请求）

**`last_send_time` 的 `if let Ok` 模式**（cache.rs 行 151）：这是另一种有效策略——静默忽略 poison，放弃本次写入。对于仅用于统计/诊断的元数据，这是合理的。但注意该 RwLock 一旦 poisoned 后**永远不会再成功写入**，该 watcher 的 `last_send_time` 将永久停留在最后一次成功的值。

### 4. Center 模块：独立二进制，隔离度高

Center 模块运行在独立的 Center 二进制中，与 Controller/Gateway 完全隔离。风险特征：

**Aggregator + Registry**：保存 Controller 连接状态和资源 key 快照。所有方法操作简单（HashMap CRUD），写路径 panic 概率极低。但如果 poison 发生，影响是 Center 完全无法管理联邦连接。

**Commander + Server + Scheduler 的 PendingMap**：`PendingListMap` 和 `PendingCommandMap` 是 `Arc<Mutex<HashMap>>`，在多个模块间共享。Commander 的 `send_command()` 有一个值得注意的模式：

```rust
// 行 45: insert
self.pending.lock().unwrap().insert(request_id.clone(), tx);

// 行 55: 发送失败时清理
session.stream_tx.send(msg).await.map_err(|_| {
    self.pending.lock().unwrap().remove(&request_id);  // 如果 Mutex 已中毒，这里 panic
    anyhow!("Failed to send command: stream closed")
})?;

// 行 62: 超时清理
tokio::time::timeout(self.timeout, rx).await.map_err(|_| {
    self.pending.lock().unwrap().remove(&request_id);  // 同上
    anyhow!("Command timed out")
})?
```

如果行 45 的 `insert` 内部 panic（极不可能），Mutex 中毒后行 55/62 的清理路径也会 panic，导致 `PendingListEntry` 泄漏。但由于 `oneshot::Sender` 被 drop 后 `oneshot::Receiver` 会收到 `RecvError`，不会造成永久阻塞。

**Center 整体风险低于 Controller ConfSync**：
- Center 是可选组件，非核心数据路径
- 连接断开后 Controller 会自动重连
- 没有 hot-path 读操作（不像 cache.rs 的 watcher task 循环读取）

### 5. grpc_server.rs 的 Mutex 使用

生产代码中 `ConfigSyncGrpcServer` 不直接使用 Mutex/RwLock。测试代码 `MockConfigSyncProvider` 中的 `Mutex<Arc<ConfigSyncServer>>` 使用 `.unwrap()` 是可接受的——测试中 panic 就是测试失败，语义正确。

---

## 最佳处理方案

### 方案选择

本模块组推荐**分层策略**，根据锁保护数据的特性选择不同方案：

#### 层 1：cache.rs 的 store/ready/watchers — 换用 `parking_lot::RwLock`

**理由**：
- `ServerCache` 是 Controller 配置同步的核心数据结构，锁密度最高（19 处）
- `store` 锁在 watcher task 的 hot loop 中被反复获取，性能敏感
- 锁保护的数据（EventStore、ready 标志、watcher 列表）没有外部不变量——如果写端 panic，store 中的数据可能不完整，但这比整个资源类型的同步全部崩溃要好
- `parking_lot::RwLock` 不 poison，省去 72 处中 19 处的 unwrap，代码更干净
- 项目已依赖 `parking_lot`（通过 tokio feature 或直接依赖）

```rust
// 修改前
use std::sync::RwLock;
pub struct ServerCache<T> {
    ready: RwLock<bool>,
    store: Arc<RwLock<EventStore<T>>>,
    watchers: RwLock<Vec<WatchClient<T>>>,
}

// 修改后
use parking_lot::RwLock;
pub struct ServerCache<T> {
    ready: RwLock<bool>,
    store: Arc<RwLock<EventStore<T>>>,
    watchers: RwLock<Vec<WatchClient<T>>>,
}
// 所有 .read().unwrap() → .read()
// 所有 .write().unwrap() → .write()
```

同步修改 `types.rs` 中 `WatchClient.last_send_time` 的 `std::sync::RwLock` 为 `parking_lot::RwLock`，消除 `if let Ok` 模式。

#### 层 2：config_sync_server.rs / client_registry.rs — 换用 `parking_lot::RwLock`

**理由**：与 cache.rs 同理。`server_id`、`endpoint_mode`、`watch_objects` 都是简单数据，没有需要 poison 机制保护的复杂不变量。

#### 层 3：Center 全部模块 — 换用 `parking_lot::RwLock` / `parking_lot::Mutex`

**理由**：
- Center 是独立二进制，改动隔离度高
- aggregator、registry 的 RwLock 和 commander/server/scheduler 的 Mutex 都保护简单 HashMap
- 统一使用 parking_lot 简化代码，消除所有 `.unwrap()`

#### 层 4：ACME service.rs — 保持现状

**理由**：
- 当前的 `unwrap_or_else(|e| e.into_inner())` 策略正确且完整
- ACME_SERVICE 是 `static Mutex`，`parking_lot::Mutex` 不支持 `const fn new()`（需要 `const_mutex` feature 或 `LazyLock` 包装）
- Worker 内部的 processing/retry_tracker 可以换 parking_lot，但当前实现已经是正确的 poison recovery，改动收益不大
- 如果全项目统一换 parking_lot 则一并修改，否则保持现有模式即可

### 实施要点

1. **确认 parking_lot 依赖**：检查 `Cargo.toml` 是否已有 `parking_lot` 直接依赖。若无，添加之。
2. **批量替换**：`std::sync::RwLock` → `parking_lot::RwLock`，`std::sync::Mutex` → `parking_lot::Mutex`。注意 API 差异：
   - `parking_lot::RwLock::read()` 返回 `RwLockReadGuard`（非 Result）
   - `parking_lot::RwLock::write()` 返回 `RwLockWriteGuard`（非 Result）
   - 去掉所有 `.unwrap()`
3. **types.rs 联动修改**：`WatchClient.last_send_time` 的类型从 `Arc<std::sync::RwLock<...>>` 改为 `Arc<parking_lot::RwLock<...>>`，同时修改 cache.rs 行 151 的 `if let Ok` 为直接写入。
4. **ACME 可选**：若选择统一 parking_lot，ACME 的 static Mutex 需要改为 `static ACME_SERVICE: parking_lot::Mutex<Option<...>> = parking_lot::const_mutex(None);`（需 parking_lot 的 `const_mutex` macro）。

---

## 优先级

| 优先级 | 模块 | 处理数 | 理由 |
|--------|------|--------|------|
| P1 | cache.rs + types.rs | 19+1 | 锁密度最高，watcher task hot loop 中 unwrap 导致级联中断所有 Gateway watch 流 |
| P1 | config_sync_server.rs | 15 | watch_objects 中毒阻塞所有资源类型的 list/watch |
| P2 | client_registry.rs | 3 | 影响 Gateway 实例计数（rate limit 依赖），但非核心数据路径 |
| P2 | Center 全部（aggregator + commander + registry + server + scheduler） | 24 | 独立二进制，影响范围有限，但统一修复成本低 |
| P3 | ACME service.rs | 0（已完成） | 已正确实施 poison recovery，无需改动 |
| N/A | grpc_server.rs 测试代码 | 3 | 仅测试代码，unwrap 即正确语义 |
