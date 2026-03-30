# ProcessorRegistry 与 Conf Center 全局状态：`unwrap()` 与锁中毒（Poison）传播风险审查

## 1. 问题概述

`ProcessorRegistry` 及同类组件在 `RwLock` / `Mutex` 的加锁结果上普遍使用 `.unwrap()`。一旦锁进入 **poisoned** 状态，后续每次 `read()` / `write()` / `lock()` 都会在 `unwrap()` 处 **直接 panic**，在 **long-running 控制面进程** 中表现为**不可恢复的进程级联崩溃**。

需要区分两类风险：

| 机制 | `std::sync::RwLock` | `std::sync::Mutex` |
|------|---------------------|---------------------|
| Poison 触发条件 | **仅当持有写锁的线程在仍持有锁时 panic**（读锁持有者 panic **不会** 使 `RwLock` 中毒） | **任意**持有者在持锁期间 panic 即中毒 |
| `.unwrap()` 后果 | 中毒后所有后续加锁在 `unwrap` 处 panic | 同上 |

因此：`ProcessorRegistry` 的 poison **主要压力在写路径**（`register`、`clear_registry`），但一旦发生，**所有读路径**（`get`、`is_all_ready`、`requeue` 间接调用等）都会在 `.unwrap()` 上集体 panic，与「多数调用是只读」并不矛盾。

本文档统计 `processor_registry.rs` 内 `unwrap`、归纳调用上下文与线程模型，并扩展到同模式的 `BidirectionalRefManager`、`FsRawStore` 及 Conf Center 中的 `Mutex` 用法，最后给出策略与优先级建议。

---

## 2. `processor_registry.rs`：`unwrap()` 全量统计与上下文

**文件**：`src/core/controller/conf_mgr/processor_registry.rs`  
**锁类型**：`RwLock<HashMap<&'static str, Arc<dyn ProcessorObj>>>`  
**`.unwrap()` 仅出现在对 `self.processors` 的 `read()` / `write()` 结果上**（无其它 `Option`/`Result` 的无关 unwrap）。

| 行号（约） | 代码形态 | 锁模式 | 方法 / 场景 |
|------------|----------|--------|-------------|
| 46 | `self.processors.write().unwrap()` | 写 | `register`：启动期为各 `ResourceProcessor` 注册 |
| 55 | `self.processors.read().unwrap()` | 读 | `get`：按 Kind 取处理器 |
| 60 | `read().unwrap()` | 读 | `all_kinds` |
| 65 | `read().unwrap()` | 读 | `count` |
| 72 | `read().unwrap()` | 读 | `is_all_ready`：就绪门控 |
| 78–80 | `read().unwrap()` | 读 | `not_ready_kinds` |
| 95–97 | `read().unwrap()` | 读 | `all_watch_objs`：ConfigSync 注册 |
| 197 | `read().unwrap()` | 读 | `set_all_not_ready` |
| 204 | `read().unwrap()` | 读 | `clear_all`（清各 processor 缓存） |
| 214–215 | `read().unwrap()` | 读 | `requeue_all`：先快照再异步 requeue |
| 245 | `self.processors.write().unwrap()` | 写 | `clear_registry`：清空 registry 并联动清理其它全局状态 |

**合计**：**12 次** `RwLock` 加锁后的 `.unwrap()`（其中写锁 **2 处**，读锁 **10 处**）。

### 2.1 是否落在「用户数据路径」上？

此处「用户数据路径」指：随 K8s Informer / 文件 watch / gRPC 等进入的 reconcile、handler、跨资源 requeue，而非仅单元测试或一次性启动代码。

- **明确在热路径 / 控制面路径**：`get`（被 handler、`HandlerContext::requeue` 链、`route_utils`、`gateway` handler 等间接或直接调用）、`is_all_ready` / `not_ready_kinds` / `all_watch_objs`（`KubernetesCenter` / `FileSystemCenter` 就绪与 ConfigSync 注册）、`requeue` / `requeue_with_chain`（内部调用 `get`）、`wait_kinds_ready`（内部 `get`）、`requeue_all`（Leader 切换后全量重排）。
- **偏启动 / 运维路径但仍影响可用性**：`register`（每 Kind 一次，但若失败或 panic 后果是全局性的）、`clear_registry`（失败恢复、丢主、重试前清理）。
- **运维 / 降级路径**：`set_all_not_ready`、`clear_all`。

结论：**绝大多数 unwrap 暴露在「高频读 + 低频写」的共享全局上**；写路径虽少，却是 **唯一能触发 `RwLock` poison 的路径**（在标准库语义下），且 `clear_registry` 还与多个其它全局单例的清理顺序耦合，panic 窗口内易出现**部分已清理、部分未清理**的状态（与 poison 叠加时更难恢复）。

---

## 3. `PROCESSOR_REGISTRY` 的调用面与线程模型（Poison 传播路径）

### 3.1 谁在读、谁在写？

- **写**：
  - `KubernetesController` / `FileSystemController` 启动：`register`。
  - `KubernetesCenter`（及错误恢复路径）：`clear_registry`（多处：重试、丢主、停止等）。
- **读**：
  - 同上 Center 的就绪轮询、`all_watch_objs`、`get` / `is_all_ready`。
  - `HandlerContext`、`revalidation_listener`、各 handler（如 `gateway`、`route_utils`）中的 `get` / `requeue`。
  - CLI 等辅助路径。

### 3.2 线程与异步任务

- **Tokio 异步运行时**：大量 `requeue`、`requeue_all`、handler 在 worker 上执行；读锁短时间持有，通常不跨 `.await`（需结合具体实现维持不变量，见同系列 `04-sync-locks-in-async-context.md`）。
- **可能的阻塞线程**：Informer 回调、同步 gRPC 线程等若直接或间接调用 `PROCESSOR_REGISTRY`，与异步任务**共享同一把 `RwLock`**。

### 3.3 传播链（示意）

```text
[写线程] register / clear_registry
    └─ write().unwrap()  → 若在持锁期间 panic → RwLock POISONED
            │
            ▼
[任意 N 个读/写线程] get / is_all_ready / requeue / …
    └─ read()/write().unwrap() → PoisonError → unwrap panic → 进程级联退出
```

**要点**：不是「handler panic 必然 poison registry」，而是 **「在持有 registry 写锁时 panic」** 才会 poison；但 **handler 若在自定义逻辑中先拿到写锁再调用可能 panic 的用户代码**，仍会把风险收敛到写路径。实际工程中更需警惕的是 **`clear_registry` 与 `register` 内的复杂逻辑未来演进**（例如在持锁范围内加入更多断言或可能 panic 的第三方调用）。

---

## 4. 同类模式：`BidirectionalRefManager`、`FsRawStore` 与其它 Conf Center 锁

### 4.1 `FsRawStore`（`raw_store.rs`）

- **结构**：`RwLock<HashMap<...>>`。
- **unwrap**：`upsert` / `remove` 上 `write().unwrap()`；`RawStore::get` 上 `read().unwrap()`。
- **路径**：FileSystem 模式下，每条 Apply/Delete 成功解析后更新；**跨资源 Raw 查询**走读锁。
- **Poison**：写路径 panic → 全 store 后续访问 unwrap 全灭。

### 4.2 `BidirectionalRefManager`（`ref_manager.rs`）

- **结构**：两把 `RwLock`（`refs`、`dependencies`）。
- **unwrap**：所有 `read()` / `write()` 约 **16 处**（含 `add_ref` / `remove_ref` / `get_refs` / `clear` 等）。
- **路径**：Secret/Service/跨命名空间引用在 reconcile 中高频更新与查询；`clear_registry` 会调用 `get_service_ref_manager().clear()` 等。
- **Poison**：任一把锁在**写锁**持有时 panic → 该索引失效；与 `ProcessorRegistry` **无自动联动恢复**。

### 4.3 Conf Center `Mutex`（示例：`kubernetes/center.rs`、`file_system/center.rs`）

- `config_sync_server`、`shutdown_handle`、`reload_tx` 等使用 `Mutex`，多处 `.lock().unwrap()`。
- **Poison 条件比 `RwLock` 更宽**：持锁线程 panic 即中毒。
- **就绪检查组合**：例如 `PROCESSOR_REGISTRY.is_all_ready() && self.config_sync_server.read().unwrap().is_some()`（K8s center 侧为 `RwLock` 包 `Option`）——**任一侧 poison 或 unwrap panic 都会导致该路径失败或进程退出**。

### 4.4 `conf_mgr` 内其它 `RwLock` + `unwrap`（grep 摘要）

以下模块与全局协调、索引或处理器状态相关，**同样存在「锁中毒 + unwrap」组合风险**（不完全统计，便于排期）：

- `resource_processor/processor.rs`：`raw_store`、`metadata_filter`、`namespace_filter` 的 `RwLock`。
- `gateway_route_index.rs`、`attached_route_tracker.rs`、`listener_port_manager.rs`。
- `ref_grant/store.rs`、`namespace_store.rs`、`configmap_store.rs`。
- `secret` handler：`init_accumulator` 的 `RwLock`。

**与本文主题关系**：它们与 `PROCESSOR_REGISTRY` **共享同一进程、同一类故障模型**；`clear_registry` 会串联清理其中多项，但**各自锁的 poison 仍需单独处理**（除非统一换用不中毒锁或统一恢复策略）。

---

## 5. 可行处理策略（技术对比）

### 5.1 `parking_lot::RwLock` / `parking_lot::Mutex`

- **特点**：**不提供 poison 语义**；持有者 panic 后数据可能处于不一致状态，但**后续加锁不会仅因「曾 panic」而失败**。
- **工程含义**：避免「一次 panic → 永久 unwrap 死」；**不替代**「业务上必须保证临界区内不 panic」的规范，也不自动修复已损坏的结构。
- **本项目**：根 `Cargo.toml` 已依赖 `parking_lot`（便于渐进迁移）。

### 5.2 保留 `std` 锁，对 `PoisonError` 显式恢复

- **`into_inner()`**：在 `Result::Err(PoisonError)` 上取回内层数据，**选择是否继续服务**（例如记录致命指标后 `process::exit`，或尽力清空并重建）。
- **适用**：必须保留 std 语义或与 FFI/审计要求兼容时。

### 5.3 `expect("...")` 替代 `unwrap()`

- **作用**：仅改善排障信息；**不降低** poison 或 panic 频率。
- **建议**：若短期不改锁类型，至少对 **写路径** 使用 `expect` 标明组件与操作，便于日志聚合。

### 5.4 API 层「优雅降级」

- 将 `get` / `is_all_ready` 等改为返回 `Result<_, RegistryError>` 或使用内部 `Option` 表示「registry 不可用」，由上层决定：拒绝新连接、进入只读、触发进程级重启等。
- **代价**：调用面大，需与 `async`/trait 边界统一错误类型。

### 5.5 架构层减负（与 01-global-singletons 一致）

- 减少在**持全局锁**时执行的逻辑；保证临界区极小、无用户可控的 `unwrap`/断言爆炸。
- 中长期将「注册表 + 索引」纳入可注入、可替换的生命周期（测试与故障域隔离）。

---

## 6. 推荐方案与优先级

| 优先级 | 范围 | 建议 | 理由 |
|--------|------|------|------|
| **P0** | `PROCESSOR_REGISTRY` 的 `RwLock` | **优先迁移至 `parking_lot::RwLock`**（或等价：std + poison 恢复 + 明确退出策略） | 单例、调用面最广；写路径 poison 会切断所有 Kind 的调度与就绪信号。 |
| **P0** | `KubernetesCenter` / `FileSystemCenter` 中与就绪强相关的 `RwLock`/`Mutex` + `unwrap` | 与 P0 同步评估：**同一「就绪探测」链路上任一 poison 都会放大为不可用** | 避免只修 registry 仍被 center 锁卡住。 |
| **P1** | `BidirectionalRefManager`、`FsRawStore`、`GatewayRouteIndex`、`AttachedRouteTracker`、`ReferenceGrantStore` 等 | 同上策略：**`parking_lot` 或 poison 处理二选一** | reconcile 热路径与 `clear_registry` 联动，故障模式与 registry 同类。 |
| **P2** | 各 handler 内与**业务不变量**相关的 `unwrap()`（如 `last_mut().unwrap()`、`find(...).unwrap()`） | 分路径改为 `if let` / 记录 error 状态，**避免非锁原因 panic 拖垮任务** | 不直接 poison 全局锁，但会导致 worker panic、任务停滞；与整体可用性相关。 |
| **P3** | 测试、配置序列化中的 `unwrap` | 可保持或逐步收紧为 `expect` | 通常不在生产热路径。 |

**不推荐**作为「主修复」：仅把 `unwrap` 改成 `expect` 而不改锁类型或 poison 策略——**运维价值有限**。

---

## 7. 结论

1. **`processor_registry.rs` 中共有 12 处**对 `processors` 的 `RwLock` 加锁 `unwrap`，其中 **2 处写、10 处读**；读方法大量处于 **requeue、就绪门控、ConfigSync 注册** 等生产路径。
2. **`std::sync::RwLock` 仅在写锁持有者 panic 时 poison**，但一旦 poison，**所有读写在 `unwrap` 上失败**，与「全局单例 + 多线程异步」结合后，表现为**单点永久性故障**，除非进程重启或显式 `into_inner` 恢复。
3. **`BidirectionalRefManager`、`FsRawStore` 及多处全局索引**重复同一模式；**`Mutex` 中心状态** poison 条件更宽，应与 registry **一并**纳入改造或恢复策略。
4. **综合推荐**：在已有 `parking_lot` 依赖前提下，**以 P0 全局注册表与 Center 关键锁为试点迁移到 `parking_lot`**，并收紧写临界区；若必须保留 std，则**必须**定义 poison 后的明确行为（恢复数据并继续 vs 记录并退出），避免静默反复 panic。

---

## 8. 参考代码位置（便于跳转）

- `src/core/controller/conf_mgr/processor_registry.rs` — 本文主分析对象  
- `src/core/controller/conf_mgr/sync_runtime/resource_processor/raw_store.rs` — `FsRawStore`  
- `src/core/controller/conf_mgr/sync_runtime/resource_processor/ref_manager.rs` — `BidirectionalRefManager`  
- `src/core/controller/conf_mgr/conf_center/kubernetes/center.rs`、`file_system/center.rs` — `PROCESSOR_REGISTRY` 与 `config_sync_server` 组合就绪逻辑  

**相关既有审查**：`tasks/review/conf_center/01-global-singletons.md`（全局单例与 `clear_registry` 范围）、`tasks/review/conf_center/04-sync-locks-in-async-context.md`（异步上下文中的锁使用）。
