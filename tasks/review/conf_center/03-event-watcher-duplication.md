# KubernetesCenter 事件监视器重复代码审查

## 问题概述

`KubernetesCenter` 在 `center.rs` 中为 **leader-only** 与 **all-serve** 两种 HA 模式分别实现了两套几乎相同的「启动事件监视器」与「事件循环」逻辑：

| 能力 | leader-only | all-serve |
|------|-------------|-----------|
| 启动监视器 | `start_event_watchers`（约 444–616 行） | `start_event_watchers_all_serve`（约 809–955 行） |
| 主事件循环 | `run_main_flow`（约 227 行起，内含重试/backoff） | `run_serving_flow`（约 706–805 行） |

两套路径在 **端点模式解析、Gateway CRD 预检、ConfigSyncServer、Controller 任务、caches 就绪轮询、跨命名空间重验证、reload channel** 等步骤上高度一致；差异集中在 **ACME 启动时机**、**Leadership watcher 语义** 以及 **外层生命周期（重试、leader 服务启停）**。

当前结构使「修一处漏一处」的概率显著升高；下文按代码块对比共享与差异，并给出重构方向与实施顺序建议。

---

## 重复代码对比

### 1. `start_event_watchers` vs `start_event_watchers_all_serve`

以下按执行顺序对照（行号以当前 `center.rs` 为准）。

#### 1.1 端点模式解析

| 步骤 | leader-only (`start_event_watchers`) | all-serve (`start_event_watchers_all_serve`) |
|------|----------------------------------------|---------------------------------------------|
| test mode | `is_test_mode()` → `EndpointMode::Both`，并打 `tracing::info`（forcing Both） | 同逻辑，**无** test mode 专用 info 日志 |
| 非 test | `resolve_endpoint_mode(client, self.config.endpoint_mode())` | 相同 |
| 全局初始化 | `init_global_endpoint_mode(resolved_mode)` | 相同 |
| 解析后日志 | 有 `resolved_mode` / `test_mode` 的 info | **无** 对等的「Endpoint mode resolved」日志 |

**结论**：逻辑等价，但 **可观测性不一致**（all-serve 缺少解析结果与 test 分支日志）。

#### 1.2 Gateway API CRD 预检

两边均为：

- `validate_no_watch_kinds(&self.config.no_watch_kinds)`
- `HashSet` 拷贝 `no_watch_kinds`
- `preflight_check_gateway_api_crds(client, &no_watch_kinds).await?`

**结论**：**逐行等价**。

#### 1.3 ConfigSyncServer

两边均为：`Arc::new(ConfigSyncServer::new())` + `set_endpoint_mode(resolved_mode)`。

**结论**：**完全重复**。

#### 1.4 Controller 创建与 `controller.run` 任务

| 项目 | leader-only | all-serve |
|------|-------------|-----------|
| 构造 | `create_k8s_controller(...)?` 链式 `.with_leader_handle(...)` | `let mut controller` 再 `with_leader_handle`（语义相同） |
| `expected_processor_kinds` | `controller.all_processor_kinds()` | 相同 |
| spawn 内错误处理 | `unwrap_or_else` → `AllControllersStopped`，日志含 `mode = "kubernetes"` | 同，日志文案为 `Controller run error (all-serve)`，**无** `mode` 字段 |

**结论**：行为一致，**日志字段/文案不统一**。

#### 1.5 Caches ready 任务（最大重复块）

以下在两边 **结构相同**（仅下列差异）：

- 共用：`CACHE_READY_TIMEOUT_SECS = 30`、`no_sync_kinds` / `expected_sync_kinds` 计算、`pending_sync_kinds` 闭包（missing registration → not ready）、200ms poll、`register_all`、`trigger_full_cross_ns_revalidation`、`trigger_gateway_secret_revalidation`、`trigger_gateway_route_revalidation`、成功发 `CachesReady` / 超时发 `CachesTimeout`。

| 差异点 | leader-only | all-serve |
|--------|-------------|-----------|
| ACME | `acme_client = client.clone()`，就绪后 `start_acme_service(acme_client)` | **不** 启动 ACME（注释说明移至 `start_leader_services`） |
| 超时 `warn!` | 含 `mode = "kubernetes"`、`timeout_secs = CACHE_READY_TIMEOUT_SECS`、`not_ready` | 含 `ha_mode = "all-serve"`、`not_ready`，**缺少 `timeout_secs`** |

**结论**：**`pending_sync_kinds` + 轮询 + 三条 revalidation 为字节级重复**；已存在一处 **日志字段不一致**（timeout 秒数仅在 leader-only 中记录）。

#### 1.6 Leadership watcher

| 项目 | leader-only | all-serve |
|------|-------------|-----------|
| 语义 | 仅检测 **失去** leader：`while lh.is_leader() { sleep 200ms }` 后发 `LeadershipLost` | **双向**：初始若已是 leader 则先发 `LeadershipAcquired`，之后 100ms 轮询检测翻转，发 `LeadershipAcquired` / `LeadershipLost` |
| 轮询间隔 | 200ms | 内层循环 100ms |
| 发送失败 | 未显式处理 `send` 错误（与 all-serve 不同） | `send` 失败则 `return` / `break` |

**结论**：这是 **设计差异**，不宜简单删其一，但可抽成两种策略实现同一 trait/枚举。

#### 1.7 Reload channel

两边均为：`mpsc::channel::<()>(1)` → `set_reload_tx` → spawn 中 `recv` 后 `ReloadRequested`。

**结论**：**完全重复**。

---

### 2. `run_main_flow` vs `run_serving_flow`

二者 **不** 是同一函数的两份拷贝，但存在明显的 **事件分发 + 清理** 模式重复。

#### 2.1 相同骨架

- 创建 `mpsc::channel::<LifecycleEvent>(32)`。
- 调用对应的 `start_event_watchers*`；失败时打 error、sleep、退出当前轮次（但 **重试策略不同**，见下）。
- `loop { match event_rx.recv().await { ... } }` 处理：
  - `CachesReady`：置 `caches_ready`、`set_config_sync_server(Some(...))`、打 info。
  - `CachesTimeout` / `ControllerExit` / `ReloadRequested` / `None`：以不同方式 `break` 出内层循环。
- 内层循环结束后：`watchers.abort_and_wait().await`。
- 若 `caches_ready`：`set_config_sync_server(None)`。

#### 2.2 主要差异

| 维度 | `run_main_flow` | `run_serving_flow` |
|------|-----------------|---------------------|
| 外层结构 | **自带** `loop` + `consecutive_failures` + backoff + reload 指标 + `STABLE_RUN_DURATION` | 单次流程；外层重试在 `run_all_serve_lifecycle`（`RelinkRequested` 时 clear + 1s sleep） |
| 迭代前 | 检查 `is_leader()`，非 leader 则清 CSS 并 `LostLeadership` | 无（所有副本都跑 watchers） |
| `CachesReady` 后 | 仅设置 CSS；ACME 已在 caches 任务里启动 | 若当前是 leader 且 leader 服务未起，调用 `start_leader_services`（内含 ACME + `requeue_all`） |
| `LeadershipAcquired` / `Lost` | `Acquired` **忽略**；`Lost` → `ControllerExitReason::LostLeadership` | 双向：`Acquired` 可能 `start_leader_services`；`Lost` → `stop_leader_services` |
| `CachesTimeout` | `tracing::error!` 后视为 controller failure | **无** error 日志，直接 `break AllControllersStopped` |
| 清理后 | **总是** `stop_acme_service()` + `clear_registry()`（为重连/retry） | `stop_leader_services` 条件调用；**总是** `clear_registry()` |
| 退出映射 | `Shutdown` / `LostLeadership` / 继续 loop（Relink / backoff） | `Shutdown` vs `RelinkRequested` |

**结论**：事件 `match` 分支若继续分裂，**语义漂移**（例如 timeout 是否算 error、是否打日志）会更难对齐。

---

## 维护风险

1. **Bug 修复遗漏**  
   对 `pending_sync_kinds`、超时阈值、revalidation 顺序、或 `no_sync_kinds` 过滤逻辑的修改，必须在两个 spawn 块中各改一次；极易漏改 all-serve 或 leader-only。

2. **行为不一致**  
   已观察到：**caches 超时** 时 leader-only 打 `error` 且带 `timeout_secs`，all-serve 的 `warn` **缺少 `timeout_secs`**；**端点模式解析** 后 leader-only 有结构化日志，all-serve 没有。后续排障会误以为「all-serve 没走到解析」或「超时时间不同」。

3. **Leadership 轮询间隔不同（200ms vs 100ms）**  
   若未来在「检测 leadership」侧修竞态或负载问题，可能只改一侧，导致 **延迟与行为** 不一致。

4. **`run_main_flow` / `run_serving_flow` 事件语义分叉**  
   新增 `LifecycleEvent` 变体时，两处 `match` 都要补全；若一侧 `ignore`、一侧处理，容易产生 **只在某种 HA 模式下生效** 的功能或 bug。

5. **ACME 与 caches 就绪的耦合点分散**  
   leader-only：ACME 在 caches 任务末尾启动；all-serve：ACME 在 `start_leader_services`。若 ACME 启动前置条件变化（例如必须等某 CR 就绪），两处都要理解并可能都要改。

---

## 重构方案

### 方案 A：抽取「公共启动」函数（推荐为第一步）

增加内部类型或枚举，将 **无分支或极少分支** 的逻辑集中：

```text
enum EventWatcherMode {
    LeaderOnly,
    AllServe,
}
```

抽取例如：

- `async fn resolve_and_init_endpoint_mode(client, config) -> Result<EndpointMode>`  
  统一 test / resolve / `init_global_endpoint_mode` / **统一日志**。
- `fn gateway_preflight(&self, client) -> impl Future` 或保持现有两行但 **单函数** `validate_and_preflight_gateway(...)`.
- `fn spawn_controller_run_task(controller, shutdown, tx, log_ctx: &str)` 或传入 `tracing::Span`。
- `fn spawn_caches_ready_task(..., OnCachesReadyHooks)`  
  其中 `OnCachesReadyHooks` 可为：
  - `start_acme: bool` 或 `Option<Client>`（有则 `start_acme_service`）；
  - 或闭包 `impl FnOnce() + Send`（测试友好，但类型可能略重）。

`spawn_leadership_watcher` 按 `EventWatcherMode` 分两支，或拆成 `spawn_leadership_loss_only` / `spawn_leadership_bidirectional` 两个小函数，**避免** 在单函数内长 `if mode`。

**优点**：改动面可控、可渐进提交；**缺点**：若 hooks 过多，需要克制避免变成「上帝参数」。

### 方案 B：配置驱动（结构体字段）

定义 `EventWatcherStartConfig`：

- `caches_ready_extra: enum None | StartAcme { client }`
- `leadership_watcher: enum LossOnly | Bidirectional { poll_ms }`
- `log_labels: HashMap` 或固定 `tracing` 字段

`start_event_watchers` 与 `start_event_watchers_all_serve` 退化为 **一行** `start_event_watchers_unified(client, ..., config)`。

**优点**：差异显式、可单测配置组合；**缺点**：需要仔细命名，避免配置组合出非法状态（可用枚举嵌套压缩非法组合）。

### 方案 C：策略 trait（适合继续膨胀时）

`trait CachesReadyPolicy`、`trait LeadershipPolicy`，由两种模式各实现一个小类型。

**优点**：扩展新 HA 变体时新增 impl；**缺点**：对当前仅两模式可能偏重，可等 A/B 落地后若仍臃肿再引入。

### 方案 D：`run_main_flow` / `run_serving_flow` 的收敛

不建议强行合并为一个巨型 loop（leader-only 的 backoff/reload 与 all-serve 的 leader 服务生命周期差异大）。更可取：

- 抽取 **`async fn drain_lifecycle_events(rx, &mut EventLoopState, Policy)`** 或按事件类型拆成小函数：`on_caches_ready`, `on_leadership_lost`，由 `Policy` 决定副作用（是否启停 ACME、是否 break 原因）。
- 或共享 **`LifecycleEvent`** 处理用 **`enum MainVsServing`** 携带「CachesTimeout 是否打 error」等微小差异。

优先保证：**CachesReady / CachesTimeout / ControllerExit / Reload / channel closed** 的日志级别与字段 **对齐**。

---

## 实施建议

1. **先对齐可观测性（低成本）**  
   为 all-serve 补上与 leader-only 对等的 endpoint 解析日志；在 caches 超时 `warn!` 中补上 `timeout_secs`（与常量一致）。减少「文档式重构」前的排障噪音。

2. **抽取 caches ready 任务（高收益）**  
   将 `pending_sync_kinds` 与 poll 循环、`register_all`、三条 `trigger_*` 移入单一 `fn spawn_caches_ready_watcher(..., start_acme: bool, client: Option<Client>)` 或 hooks。这是重复行数最多、最易漏改的部分。

3. **统一 leadership watcher 的辅助函数**  
   `loss_only_watcher` / `bidirectional_watcher` 两个 `tokio::spawn` 闭包，明确注释语义与轮询间隔；若产品允许，评估是否统一 poll 间隔（否则在注释中写明 **为何** all-serve 用 100ms）。

4. **再合并「前半段」启动流水线**  
   endpoint + preflight + `ConfigSyncServer` + controller spawn + reload，合并为 `start_event_watchers_unified`。

5. **最后考虑 `run_*_flow` 的事件处理表**  
   在 watcher 启动已统一、行为稳定后，再抽事件处理，避免同时改两层导致回归难查。

6. **回归验证建议**  
   - leader-only：leadership 丢失、reload、caches 超时、正常就绪。  
   - all-serve：非 leader →  leader 时 `start_leader_services`、丢 leader 时 `stop_leader_services`、gRPC 与 CSS 可用性。  
   - 若有无单测，至少对 `pending_sync_kinds` 逻辑做 **纯函数单元测试**（传入伪造 registry 或可注入接口），避免双份闭包再次分叉。

---

## 附录：涉及符号速查

| 符号 | 说明 |
|------|------|
| `resolve_endpoint_mode` | 端点模式解析 |
| `validate_no_watch_kinds` / `preflight_check_gateway_api_crds` | Gateway 相关预检 |
| `ConfigSyncServer` | caches 就绪后注册 watch 对象 |
| `create_k8s_controller` | 构造 `KubernetesController` |
| `trigger_full_cross_ns_revalidation` 等 | caches 就绪后的重验证 |
| `start_acme_service` / `stop_acme_service` | ACME 生命周期 |
| `start_leader_services` / `stop_leader_services` | all-serve 下 leader 专属服务 |
| `WatcherHandles` | controller / caches / leader / reload 四类 join/abort 句柄 |

---

*文档基于 `Edgion/src/core/controller/conf_mgr/conf_center/kubernetes/center.rs` 当前实现整理。*
