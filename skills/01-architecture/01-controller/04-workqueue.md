---
name: controller-workqueue
description: Workqueue 内部机制：去重、dirty requeue、指数退避、延迟入队、TriggerChain 环检测。
---

# Workqueue 机制

## 设计理念

Workqueue 采用 Go controller-runtime 风格设计，每种资源类型拥有独立的队列实例，提供：

- **去重**：同一 key 在队列中仅存在一次
- **Dirty requeue**：处理期间允许重新入队，不丢失更新
- **指数退避**：失败重试时延迟递增
- **延迟入队**：跨资源 requeue 的合并调度
- **TriggerChain**：级联因果追踪与环检测

源文件：`src/core/controller/conf_mgr/sync_runtime/workqueue.rs`

## 数据结构

### Workqueue 核心字段

```text
Workqueue
├── name: String                          // 队列名称（日志和指标使用）
├── tx: mpsc::Sender<WorkItem>            // 就绪队列发送端
├── rx: Mutex<mpsc::Receiver<WorkItem>>   // 就绪队列接收端（Mutex 保证单消费者）
├── pending: Arc<DashSet<String>>         // 就绪队列中的 key 集合（去重用）
├── scheduled: Arc<DashSet<String>>       // 延迟队列中的 key 集合（去重用）
├── delay_tx: mpsc::Sender<DelayedItem>   // 延迟队列发送端
├── config: WorkqueueConfig               // 配置
└── metrics: Arc<WorkqueueMetrics>        // 指标
```

就绪队列使用 `mpsc::channel`（有界，容量由 `config.capacity` 控制）。延迟队列通过独立的 `mpsc::channel` 接收延迟项，在后台任务中使用 `BinaryHeap`（最小堆，按 `ready_at` 排序）管理定时。

### WorkItem

```rust
pub struct WorkItem {
    pub key: String,                // "namespace/name"（命名空间级）或 "name"（集群级）
    pub retry_count: u32,           // 已重试次数
    pub enqueue_time: Instant,      // 入队时间
    pub trigger_chain: TriggerChain, // 级联追踪链
}
```

三种构造方式：
- `WorkItem::new(key)` — 原始事件，空链
- `WorkItem::for_retry(key, retry_count)` — 失败重试
- `WorkItem::with_chain(key, chain)` — 跨资源 requeue，携带因果链

## WorkqueueConfig

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `capacity` | `usize` | 5000 | 有界通道容量 |
| `max_retries` | `u32` | 5 | 最大重试次数，超过后放弃 |
| `initial_backoff` | `Duration` | 100ms | 退避初始延迟 |
| `max_backoff` | `Duration` | 30s | 退避最大延迟 |
| `default_requeue_delay` | `Duration` | 100ms | 跨资源 requeue 默认延迟 |
| `max_trigger_cycles` | `usize` | 5 | 同一 (kind, key) 在触发链中最大出现次数 |
| `max_trigger_depth` | `usize` | 20 | 触发链最大总深度（安全网） |

## 去重机制

`pending` 集合（`DashSet<String>`）记录当前在就绪队列中的所有 key。`enqueue()` 入队前检查：

```text
enqueue("ns/my-route"):
  pending 已包含 "ns/my-route"?
    ├─ 是 → 跳过，返回 false
    └─ 否 → 插入 pending → 发送到 mpsc → 返回 true
```

`enqueue_after()` 额外检查 `scheduled` 集合（延迟队列中的 key），两个集合任一包含该 key 则跳过。

当队列深度达到容量 80% 时打印告警日志（每 500 项一次）。

## Dirty Requeue 模式

关键设计：**不追踪 "processing" 状态**。key 在 `dequeue()` 时立即从 `pending` 移除，而非在处理完成后移除。

```text
时间线：
  t1: enqueue("ns/a")    → pending = {"ns/a"}
  t2: dequeue()           → pending = {}          // 立即移除
  t3: (处理中) enqueue("ns/a") → pending = {"ns/a"}  // 允许重新入队
  t4: done("ns/a")        → (no-op)
  t5: dequeue()           → pending = {}          // 处理最新版本
```

这保证了处理期间发生的更新不会丢失：即使当前正在处理某个 key，新的变更仍能入队并在之后被处理。

`done()` 方法当前是 no-op（仅记录日志），保留 API 兼容性。

## 指数退避

`requeue_with_backoff()` 在处理失败时调用，计算退避延迟后 spawn 异步任务延迟重新入队：

```text
backoff = initial_backoff * 2^retry_count
backoff = min(backoff, max_backoff)
```

具体流程：

1. `retry_count + 1` 超过 `max_retries` → 放弃，记录 warn 日志
2. 计算退避：`initial_backoff.saturating_mul(2^retry_count).min(max_backoff)`
3. spawn 异步任务：`sleep(backoff)` → 检查 `pending` 去重 → 插入 `pending` → 发送 `WorkItem::for_retry`

以默认配置为例：

| 重试次数 | 退避 |
|----------|------|
| 0 → 1 | 100ms |
| 1 → 2 | 200ms |
| 2 → 3 | 400ms |
| 3 → 4 | 800ms |
| 4 → 5 | 1.6s |
| 5 → 放弃 | — |

退避重试与延迟入队（`enqueue_after`）是独立子系统，避免长时间重试退避阻塞短延迟的跨资源 requeue。

## 延迟入队（enqueue_after）

用于跨资源 requeue 的合并调度。当资源 A 变更需要触发资源 B 重新处理时，通过延迟入队合并短时间内的多次触发：

```text
enqueue_after("ns/gateway-1", 100ms, chain):
  pending 或 scheduled 已包含?
    ├─ 是 → 跳过
    └─ 否 → 插入 scheduled → 发送 DelayedItem 到 delay_tx
```

后台 delay loop 任务管理 `BinaryHeap<DelayedItem>`（按 `ready_at` 最小堆排序）：

```text
loop {
    if heap 为空 → 阻塞等待 delay_rx.recv()
    else {
        tokio::select! {
            sleep(下一个到期时间) => {
                弹出所有已到期项 → 检查 pending 去重 → 发送到就绪队列
            }
            delay_rx.recv() => {
                新延迟项推入 heap
            }
        }
    }
}
```

延迟项到期后，从 `scheduled` 移除、检查 `pending` 避免重复、插入 `pending` 后发送到就绪队列。

## TriggerChain：级联因果追踪

TriggerChain 类似 HTTP 的 `X-Forwarded-For`，记录引发级联 requeue 的资源路径：

```rust
pub struct TriggerChain {
    pub sources: SmallVec<[TriggerSource; 4]>,  // 栈上优化，<=4 跳无堆分配
}

pub struct TriggerSource {
    pub kind: &'static str,   // 例如 "HTTPRoute"、"Gateway"
    pub key: String,           // 例如 "default/my-route"
}
```

### 链的构建

每个 ResourceProcessor 处理完成后，在 requeue 下游资源前调用 `chain.extend(kind, key)` 追加自身信息：

```text
原始事件: Secret/default/tls-cert 变更
  → 触发 Gateway/default/gw-1 requeue，chain = [Secret/default/tls-cert]
    → 触发 HTTPRoute/default/route-1 requeue，chain = [Secret/default/tls-cert → Gateway/default/gw-1]
```

### 环检测

入队前通过 `would_exceed_cycle_limit()` 检查目标 (kind, key) 在链中的出现次数：

```text
chain.occurrence_count(target_kind, target_key) >= max_trigger_cycles?
  ├─ 是 → 终止级联，不入队
  └─ 否 → 正常入队
```

`max_trigger_cycles` 默认为 5，`max_trigger_depth` 默认为 20（总深度安全网）。

示例：如果 `HTTPRoute/default/route-1` 在链中已出现 5 次，再次尝试 requeue 该资源时将被阻止，防止无限循环。

## Worker 生命周期

Worker 是消费 Workqueue 的异步任务，每个 `ResourceController` 拥有一个 worker：

```text
ResourceController 事件循环:
  Event::Applied(obj) → processor.process_init_item() (初始化阶段)
  Event::InitDone     → processor.on_init_done()
                        worker_handle = spawn_worker(processor, ...)  // 此时才启动 worker
  Event::Applied(obj) → processor.process_work_item() (运行时阶段，由 worker 驱动)
```

**worker 仅在 `InitDone` 之后启动**。初始化阶段（LIST 回放）期间，事件由事件循环同步处理；`InitDone` 标志 LIST 完成后，spawn worker 开始从 Workqueue 消费并异步处理增量事件。

这一设计确保：

- 初始化阶段按序处理全量数据，不会因并发导致中间状态
- 运行时阶段通过 Workqueue 解耦事件接收与处理，支持去重和退避

## 指标（WorkqueueMetrics）

| 指标 | 类型 | 说明 |
|------|------|------|
| `adds_total` | AtomicU64 | 入队总次数 |
| `retries_total` | AtomicU64 | 重试总次数 |
| `depth` | AtomicU64 | 当前队列深度 |
| `delayed_total` | AtomicU64 | 延迟入队总次数 |

所有指标使用 `Ordering::Relaxed` 原子操作，适合监控但不提供严格一致性。

## 关键源文件

| 文件 | 职责 |
|------|------|
| `src/core/controller/conf_mgr/sync_runtime/workqueue.rs` | Workqueue 完整实现 |
| `src/core/controller/conf_mgr/sync_runtime/resource_processor/processor.rs` | ResourceProcessor，使用 Workqueue |
| `src/core/controller/conf_mgr/conf_center/kubernetes/resource_controller.rs` | K8s ResourceController，spawn_worker |
| `src/core/controller/conf_mgr/conf_center/file_system/resource_controller.rs` | 文件系统 ResourceController，spawn_worker |
