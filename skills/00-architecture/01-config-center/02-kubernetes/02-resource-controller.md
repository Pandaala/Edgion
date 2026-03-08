# ResourceController 与 Status 写入

> 单资源类型的完整生命周期：watcher → reflector → init/runtime → workqueue → status persist。

## 架构概览

每种 K8s 资源类型由一个独立的 `ResourceController<K>` 管理：

```
KubernetesController.run()
├── spawn::<HTTPRoute, _>(handler)     → ResourceController<HTTPRoute>
├── spawn::<Gateway, _>(handler)       → ResourceController<Gateway>
├── spawn::<Service, _>(handler)       → ResourceController<Service>
└── ... (~20 resource types, all independent)
```

每个 ResourceController 独立运行：
1. 创建 `ResourceProcessor<K>`（含 `ServerCache<K>` + `Workqueue`）
2. 注册到 `PROCESSOR_REGISTRY`
3. 启动 K8s watcher stream
4. Init 阶段 → Runtime 阶段

## 生命周期

```
ResourceController<K>::run_with_api()
│
├── Step 1: Create reflector store + watcher stream
│
├── Step 2-6: Init Phase (LIST)
│   ├── Event::Init         → processor.on_init()
│   ├── Event::InitApply(obj) → processor.on_init_apply(obj)
│   │                          → persist_k8s_status() if leader
│   └── Event::InitDone    → processor.on_init_done()
│                              → spawn_worker()
│
└── Step 7-8: Runtime Phase (WATCH)
    ├── Event::Apply(obj)  → processor.on_apply(obj) → enqueue key
    ├── Event::Delete(obj) → processor.on_delete(obj) → enqueue key
    └── Worker loop:
        ├── dequeue work item
        ├── get obj from store
        ├── processor.process_work_item()
        └── persist_k8s_status() if leader && status_changed
```

## ResourceProcessor Pipeline

每个资源经过完整的处理管线：

```
process_resource(obj):
├── 1. Namespace filter check
├── 2. Handler filter (e.g. gateway_class match)
├── 3. Clean metadata (remove managedFields, blocked annotations)
├── 4. Validate (schema + semantic, collect errors)
├── 5. Preparse (build runtime structures)
├── 6. Parse (resolve refs, update caches)
├── 7. Extract old status (for comparison)
├── 8. Update status (Gateway API conditions)
├── 9. Check if status changed
├── 10. on_change (notify dependents via cross-resource requeue)
└── 11. Save to ServerCache
```

## Status 写入与 Leader Guard

### persist_k8s_status

Status 通过 K8s API 的 status subresource 回写：

```rust
async fn persist_k8s_status<K>(
    client: &Client,
    api_scope: &ApiScope,
    namespace: Option<&str>,
    name: &str,
    status_value: &serde_json::Value,
) -> Result<(), kube::Error>
```

使用 `DynamicObject` + JSON Merge Patch 写入 `/status` subresource，
避免 Scope 泛型约束问题。

### Leader Guard

所有 `persist_k8s_status()` 调用均受 `leader_handle` 守卫：

```rust
let can_write_status = leader_handle.as_ref().is_none_or(|h| h.is_leader());
if status_changed && can_write_status {
    persist_k8s_status::<K>(...).await;
}
```

三个检查点一致地应用了此逻辑：
1. **Init phase** — `Event::InitApply` 处理中
2. **Init phase (edge case)** — `Event::Apply` 在 init_done 之前到达时
3. **Runtime worker** — `spawn_worker` 中的 work item 处理

当 `leader_handle` 为 `None`（FileSystem 模式或无 HA）时，`is_none_or` 返回 `true`，
始终允许写入。

### Status 变更检测

通过比较序列化后的 JSON 字符串判断 status 是否变化：

```rust
enum StatusExtractResult {
    Present(String),     // status 字段存在且有值
    Empty,               // status 为 null 或空
    SerializationError,  // 序列化失败
}
```

只有 status 实际变化时才触发写入，避免不必要的 K8s API 调用。

## Workqueue 机制

### 结构

```rust
pub struct Workqueue {
    kind: &'static str,
    queue: Mutex<VecDeque<WorkItem>>,
    pending: Mutex<HashMap<String, PendingItem>>,
    notify: Notify,
    config: WorkqueueConfig,
}
```

### WorkqueueConfig

```rust
pub struct WorkqueueConfig {
    pub max_retries: u32,              // 最大重试次数
    pub initial_backoff: Duration,     // 初始退避时间
    pub max_backoff: Duration,         // 最大退避时间
    pub default_requeue_delay: Duration, // 跨资源 requeue 延迟
    pub max_trigger_cycles: usize,     // 触发链最大深度
}
```

### 去重机制

```
enqueue("default/my-route"):
├── if "default/my-route" in pending set → skip (already queued)
├── else → add to pending + push to queue + notify worker
```

### Dirty Requeue

```
dequeue():
├── pop from queue
├── remove from pending     ← key released here
└── return WorkItem

# During processing of "default/my-route":
# if K8s sends another event for same key → enqueue succeeds
# because key was already removed from pending
```

### Trigger Chain（级联追踪）

跨资源 requeue 携带 `TriggerChain` 以检测循环：

```rust
pub struct TriggerChain {
    entries: Vec<TriggerEntry>,  // [(kind, key), ...]
}
```

每次跨资源 requeue 时扩展链，检测是否超过 `max_trigger_cycles` 或形成循环。

## ProcessorRegistry

全局单例 `PROCESSOR_REGISTRY` 提供统一的处理器访问：

```rust
pub static PROCESSOR_REGISTRY: LazyLock<ProcessorRegistry> = LazyLock::new(ProcessorRegistry::new);
```

### 核心方法

| 方法 | 用途 |
|------|------|
| `register(processor)` | 注册处理器（启动时） |
| `get(kind)` | 按种类名获取 |
| `requeue(kind, key)` | 跨资源即时 requeue |
| `requeue_with_chain(kind, key, chain)` | 带延迟的跨资源 requeue |
| `requeue_all()` | 全量 requeue（leader 切换时） |
| `is_all_ready()` | 检查所有缓存是否就绪 |
| `all_watch_objs(no_sync)` | 收集 WatchObj 给 ConfigSyncServer |
| `clear_registry()` | 清空所有处理器（重启/失去 leadership） |

### requeue_all()

Leader 切换时调用，触发全量 status 重新协调：

```rust
pub fn requeue_all(&self) {
    for (kind, processor) in processors.iter() {
        let count = processor.requeue_all_keys();
        // logs count per kind
    }
}
```

`requeue_all_keys()` 从 `ServerCache` 获取所有 key，异步 enqueue 到 workqueue。
workqueue 的去重机制确保不会产生重复处理。

## 410 Gone 处理

当 K8s API server 的 watch bookmark 过期时，watcher 收到 410 Gone 错误。
ResourceController 检测到 `Event::Init` 在 `init_done` 之后再次出现，
判定为 watcher 重连，发送 `RelinkReason::WatcherReconnected` 信号，
触发整个 `KubernetesController` 重建。

## 关键文件

- `conf_center/kubernetes/resource_controller.rs` — `ResourceController<K>`
- `sync_runtime/resource_processor/processor.rs` — `ResourceProcessor<K>`, `ProcessorObj` trait
- `sync_runtime/workqueue.rs` — `Workqueue`, `WorkItem`
- `conf_mgr/processor_registry.rs` — `PROCESSOR_REGISTRY`
