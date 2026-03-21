---
name: kubernetes-resource-controller
description: Kubernetes ResourceController 生命周期：Reflector 监听、Init/Runtime 阶段、ResourceProcessor 11 步流水线、Status 回写、410 Gone 处理。
---

# ResourceController 生命周期

每种 K8s 资源类型由一个独立的 `ResourceController<K>` 管理，所有实例完全独立并行运行。

## 架构概览

```
KubernetesController.run()
├── spawn::<HTTPRoute, _>(handler)      → ResourceController<HTTPRoute>
├── spawn::<Gateway, _>(handler)        → ResourceController<Gateway>
├── spawn::<Service, _>(handler)        → ResourceController<Service>
└── ... (~20 种资源类型，全部独立)
```

每个 ResourceController 的内部结构：

```
ResourceController<K>
├── kind: &'static str
├── client: kube::Client
├── watcher_config: watcher::Config
├── api_scope: ApiScope                  # Namespaced(watch_mode) | ClusterScoped
├── processor: Arc<ResourceProcessor<K>> # 持有 ServerCache + Workqueue + Handler
├── namespace_filter: Option<Vec<String>>
├── shutdown_signal: Option<ShutdownSignal>
├── relink_signal: Option<RelinkSignalSender>
└── leader_handle: Option<LeaderHandle>
```

启动步骤：
1. 创建 `ResourceProcessor<K>`（含 `ServerCache<K>` + `Workqueue`）
2. 注册到 `PROCESSOR_REGISTRY`
3. 调用 `run_namespaced()` 或 `run_cluster_scoped()` → `run_with_api(api)`
4. 进入 Init 阶段 → Runtime 阶段

## Init 阶段（Steps 1-6）

```
run_with_api(api)
│
├── Step 1: 创建 Reflector store + watcher stream
│   store, writer = reflector::store()
│   watcher_stream = watcher(api, watcher_config)
│   stream = reflector(writer, watcher_stream)  # 合并后的事件流
│
├── Step 2: Event::Init
│   └── processor.on_init()                     # 标记初始化开始
│       └── init_timer = InitSyncTimer::start()
│
├── Step 3-5: Event::InitApply(obj)  (重复 N 次，每个 LIST 结果一次)
│   ├── namespace_filter 检查
│   ├── processor.on_init_apply(obj, None)
│   │   └── 完整的 ResourceProcessor 11 步流水线
│   ├── init_count += 1
│   └── if status_changed && can_write_status:
│       └── persist_k8s_status()                # Leader 守卫
│
└── Step 6: Event::InitDone
    ├── processor.on_init_done()                # 标记 ready
    └── spawn_worker()                          # 启动 Workqueue Worker
```

Init 阶段处理 K8s API 的 LIST 结果，每个对象触发 `InitApply`。
此阶段的 status 回写是同步的（在事件循环中直接 await），因为 worker 尚未启动。

## Runtime 阶段（Steps 7-8）

```
Step 7-8: Runtime Phase (WATCH)
├── Event::Apply(obj)
│   ├── namespace_filter 检查
│   ├── processor.on_apply(obj)       → enqueue key
│   └── (特殊情况：init_done 之前到达的 Apply 直接处理 + status 写入)
│
├── Event::Delete(obj)
│   └── processor.on_delete(obj)      → enqueue key
│
└── Worker loop (spawn_worker):
    ├── dequeue work item             # 阻塞等待，key 从 pending 释放
    ├── get obj from reflector store
    ├── processor.process_work_item(obj, store)
    └── if status_changed && can_write_status:
        └── persist_k8s_status()      # Leader 守卫
```

Runtime 阶段使用 Go operator 风格的 workqueue：所有事件（Apply/Delete）先 enqueue key，
由 worker 统一 dequeue 处理。worker 从 reflector store 获取最新对象状态，
因此即使多个事件在短时间内到达，也只会处理一次最新状态。

## ResourceProcessor 11 步流水线

每个资源经过完整的处理管线（`process_resource`）：

```
process_resource(obj):
├── Step 1.  Namespace filter check      — 多命名空间模式下过滤不相关命名空间
├── Step 2.  Handler filter              — 业务过滤（如 gateway_class 匹配）
├── Step 3.  Clean metadata              — 移除 managedFields、blocked annotations
├── Step 4.  Validate                    — Schema + 语义校验，收集错误
├── Step 5.  Preparse                    — 构建运行时结构（如解析 host/path 匹配规则）
├── Step 6.  Parse                       — 解析引用、更新缓存（Secret/Service ref 注册）
├── Step 7.  Extract old status          — 从当前对象提取旧 status（用于变更比较）
├── Step 8.  Update status               — 计算新 status（Gateway API conditions）
├── Step 9.  Check if status changed     — 比较序列化 JSON 字符串
├── Step 10. on_change                   — 通知依赖资源（跨资源 requeue）
└── Step 11. Save to ServerCache         — 存入 gRPC 同步缓存
```

Step 4-6 由 `ProcessorHandler<K>` trait 的具体实现负责，每种资源类型有独立的 Handler。
Step 7-9 使用统一的 status 提取和比较逻辑。
Step 10 的跨资源 requeue 通过 `PROCESSOR_REGISTRY.requeue()` 实现。
Step 11 写入 `ServerCache` 后，gRPC Watch 流会自动推送变更给 Gateway。

## persist_k8s_status — Status 回写

通过 K8s API 的 status subresource 回写 status：

```rust
async fn persist_k8s_status<K>(
    client: &Client,
    api_scope: &ApiScope,
    namespace: Option<&str>,
    name: &str,
    status_value: &serde_json::Value,
) -> Result<(), kube::Error>
```

使用 `DynamicObject` + JSON Merge Patch 写入 `/status` subresource。
选择 `DynamicObject` 而非泛型 `Api<K>` 是为了避免 Scope 泛型约束问题
（同一函数需要处理 Namespaced 和 ClusterScoped 资源）。

### Leader 守卫

所有 `persist_k8s_status()` 调用均受 `leader_handle` 守卫：

```rust
let can_write_status = leader_handle.as_ref().is_none_or(|h| h.is_leader());
if status_changed && can_write_status {
    persist_k8s_status::<K>(...).await;
}
```

三个检查点一致应用此逻辑：

| 检查点 | 位置 | 说明 |
|--------|------|------|
| Init phase | `Event::InitApply` 处理 | LIST 结果的 status 回写 |
| Init edge case | `Event::Apply` 在 init_done 前到达 | 竞态条件下的 status 回写 |
| Runtime worker | `spawn_worker` work item 处理 | 常规运行时 status 回写 |

当 `leader_handle` 为 `None`（FileSystem 模式）时，`is_none_or` 返回 `true`，始终允许写入。

### Status 变更检测

通过比较序列化后的 JSON 字符串判断 status 是否变化：

```rust
enum StatusExtractResult {
    Present(String),       // status 字段存在且有值，包含序列化 JSON
    Empty,                 // status 为 null 或空
    SerializationError,    // 序列化失败
}
```

只有 status 实际变化时才触发 K8s API 调用，避免不必要的 patch 请求。
比较使用 JSON 字符串而非结构体 PartialEq，因为不同资源类型的 status 结构不同，
统一使用 JSON 序列化后的字符串比较是最通用的方案。

## Workqueue 机制

### 结构

```rust
pub struct Workqueue {
    kind: &'static str,
    queue: Mutex<VecDeque<WorkItem>>,       // FIFO 队列
    pending: Mutex<HashMap<String, PendingItem>>,  // 去重 set
    notify: Notify,                         // 唤醒 worker
    config: WorkqueueConfig,
}
```

### 去重

```
enqueue("default/my-route"):
├── if "default/my-route" in pending → skip（已在队列中）
└── else → add to pending + push to queue + notify worker
```

### Dirty Requeue

```
dequeue():
├── pop from queue
├── remove from pending    ← key 在此处释放
└── return WorkItem

# 处理 "default/my-route" 期间：
# 如果 K8s 发送该 key 的新事件 → enqueue 成功
# 因为 key 已从 pending 中移除
```

### WorkqueueConfig

```rust
pub struct WorkqueueConfig {
    pub max_retries: u32,                // 最大重试次数
    pub initial_backoff: Duration,       // 初始退避时间
    pub max_backoff: Duration,           // 最大退避时间
    pub default_requeue_delay: Duration, // 跨资源 requeue 延迟
    pub max_trigger_cycles: usize,       // 触发链最大深度
}
```

### TriggerChain（级联追踪）

跨资源 requeue 携带 `TriggerChain` 以检测循环和控制深度：

```rust
pub struct TriggerChain {
    entries: Vec<TriggerEntry>,  // [(kind, key), ...]
}
```

每次跨资源 requeue 时扩展链，检测是否超过 `max_trigger_cycles` 或形成循环。
超过限制时停止传播并记录警告日志。

## 410 Gone 处理

当 K8s API server 的 watch bookmark 过期时，watcher 收到 410 Gone 错误，
导致 Reflector 自动重连并重新 LIST。

`ResourceController` 检测到 `Event::Init` 在 `init_done = true` 之后再次出现，
判定为 watcher 重连：

```rust
Event::Init => {
    if init_done {
        // Watcher reconnecting (possible 410 Gone)
        relink_signal.try_send(RelinkReason::WatcherReconnected);
        break;  // 退出当前 ResourceController
    }
}
```

发送 `RelinkReason::WatcherReconnected` 信号后，上层 `KubernetesController` 收到
`ControllerExit(Relink)` 事件，触发整个 Controller 的完整重建：
清空 `PROCESSOR_REGISTRY` → 重新创建所有 `ResourceController` → 重新 LIST + WATCH。

这是一个保守但安全的策略：任何一个资源的 watcher 重连都触发全量重建，
避免部分 cache 与 K8s API 不一致的风险。

## 关键文件

- `src/core/controller/conf_mgr/conf_center/kubernetes/resource_controller.rs` — `ResourceController<K>`, `ApiScope`, `RelinkReason`
- `src/core/controller/conf_mgr/sync_runtime/resource_processor/processor.rs` — `ResourceProcessor<K>`, `ProcessorObj` trait
- `src/core/controller/conf_mgr/sync_runtime/resource_processor/handler.rs` — `ProcessorHandler<K>` trait
- `src/core/controller/conf_mgr/sync_runtime/workqueue.rs` — `Workqueue`, `WorkItem`, `WorkqueueConfig`
- `src/core/controller/conf_mgr/processor_registry.rs` — `PROCESSOR_REGISTRY`
- `src/core/controller/conf_mgr/conf_center/kubernetes/status.rs` — `persist_k8s_status`
