---
name: controller-cache-server
description: ServerCache + EventStore：泛型内存缓存、环形事件缓冲区、单调递增 sync_version、WatchObj trait、ConfigSyncServer 架构。
---

# CacheServer（内存缓存与配置同步）

ServerCache 和 EventStore 构成 Controller 的内存缓存层，ResourceProcessor 将处理后的资源写入 ServerCache，ServerCache 通过事件通知机制推送变更给 gRPC watcher 客户端（即 Gateway 实例）。

## ServerCache\<T\>

泛型内存缓存，每种资源类型一个实例，由对应的 `ResourceProcessor<K>` 创建和持有。

```rust
pub struct ServerCache<T: ResourceMeta + Resource + Send + Sync> {
    ready: RwLock<bool>,                          // 初始化完成标志
    store: Arc<RwLock<EventStore<T>>>,             // 事件存储
    watchers: RwLock<Vec<WatchClient<T>>>,          // 待处理的 watch 请求
    notify: Arc<Notify>,                            // 广播通知（唤醒所有 watcher 任务）
}
```

### 生命周期

1. **创建**: `ServerCache::new(capacity)` — 初始化环形缓冲区，状态为未就绪
2. **初始化写入**: `apply_change(InitAdd, resource)` — 同步写入 EventStore（不触发 watcher 通知）
3. **就绪标记**: `set_ready()` — 标记缓存已就绪，允许 gRPC 客户端连接
4. **运行时写入**: `apply_change(EventUpdate/EventDelete, resource)` — 写入事件并通知所有 watcher
5. **重链接**: `clear()` + `set_not_ready()` — 清空数据，等待新的初始化

### 数据操作

| 方法 | 说明 |
|---|---|
| `list()` / `list_owned()` | 返回所有资源的快照和当前 sync_version |
| `get_by_key(key)` | 按 "namespace/name" 键获取单个资源 |
| `watch(client_id, client_name, from_version)` | 启动 watcher 任务，返回 mpsc::Receiver 持续接收变更 |
| `apply_change(change, resource)` | 根据 ResourceChange 类型执行对应操作 |

### sync_version

sync_version 是全局单调递增的版本号（通过 `utils::next_resource_version()` 生成），独立于 K8s resourceVersion。每次写入操作分配新版本号，用于：

- Watcher 增量同步：客户端报告 `from_version`，服务端返回该版本之后的所有事件
- 过期检测：客户端版本太旧（事件已被环形缓冲区覆盖）时返回 `TooOldVersion` 错误
- 乱序防护：低于当前 sync_version 的事件直接丢弃（Stage A 保护）

### Watcher 任务

`start_watcher_task()` 为每个 watch 请求启动独立的 tokio 任务：

```
loop {
    1. 从 EventStore 获取 from_version 之后的事件
    2. 若有新事件 → 发送给客户端，更新 from_version
    3. 若版本跳跃但无事件 → 事件丢失，发送 WATCH_ERR_EVENTS_LOST 错误
    4. 若无新事件 → 等待 notify.notified() 或 sender.closed()
}
```

watcher 任务退出条件：接收端断开（gRPC 连接关闭）、EventStore 返回错误、或事件丢失。

## EventStore\<T\>

环形事件缓冲区，存储最近的资源变更事件，同时维护全量资源快照。

```rust
pub struct EventStore<T> {
    capacity: usize,                    // 环形缓冲区容量（最小 10，默认 200）
    cache: Vec<Option<WatcherEvent<T>>>, // 环形缓冲区
    start_index: usize,                 // 有效事件起始位置
    end_index: usize,                   // 下一个写入位置
    sync_version: u64,                  // 当前最大版本号
    expire_sync_version: u64,           // 已过期的最小版本号
    data: HashMap<String, T>,           // 全量资源快照（key → 最新资源）
}
```

### 事件类型

`WatcherEvent<T>` 包含三种类型：`Add`、`Update`、`Delete`，每个事件携带 `sync_version` 和资源数据。

### 环形缓冲区机制

- 写入时 `end_index` 递增，当缓冲区满时 `start_index` 递增（覆盖最旧事件）
- 被覆盖事件的 `sync_version` 记录到 `expire_sync_version`
- 客户端请求 `from_version < expire_sync_version` 时返回 `TooOldVersion` 错误，触发客户端全量重新同步

### 乱序防护（Stage A）

`apply_event()` 在写入前检查：若 `incoming_sync_version < current_sync_version`，则丢弃该事件并记录警告。这防止 tokio 调度器重排序导致旧数据覆盖新数据。

### 查询接口

| 方法 | 说明 |
|---|---|
| `get_events_from_sync_version(from)` | 返回 from 之后的所有事件 + 当前版本号 |
| `snapshot_owned()` | 返回全量资源快照（克隆） |
| `get_by_key(key)` | 按键获取单个资源 |
| `init_add(version, resource)` | 初始化阶段写入（不进入环形缓冲区） |

## WatchObj trait

对象安全（object-safe）的 list/watch 接口，由 `ServerCache<T>` 实现，使 `ConfigSyncServer` 能以统一方式管理不同类型的缓存。

```rust
pub trait WatchObj: Send + Sync {
    fn kind_name(&self) -> &'static str;
    fn list_json(&self) -> Result<(String, u64), String>;
    fn watch_json(&self, client_id: String, client_name: String, from_version: u64)
        -> mpsc::Receiver<WatchResponseSimple>;
    fn is_ready(&self) -> bool;
    fn set_ready(&self);
    fn set_not_ready(&self);
    fn clear(&self);
}
```

所有方法涉及序列化（JSON），因此使用 trait object 是合理的——序列化开销远大于虚调用开销。`ServerCache<T>` 通过 `list_json()` 返回 JSON 字符串 + sync_version，通过 `watch_json()` 返回 `WatchResponseSimple`（JSON 字符串形式的事件流）。

## ConfigSyncServer

gRPC 配置同步服务器，持有所有 WatchObj 并提供统一的 list/watch API。

```rust
pub struct ConfigSyncServer {
    server_id: RwLock<String>,                              // 服务器实例 ID（毫秒时间戳）
    endpoint_mode: RwLock<Option<EndpointMode>>,             // 端点发现模式
    watch_objects: RwLock<HashMap<String, Arc<dyn WatchObj>>>, // 按 kind 注册的 WatchObj
    client_registry: Arc<ClientRegistry>,                    // 已连接 Gateway 实例注册表
}
```

### server_id

启动时生成的毫秒时间戳字符串，用于客户端检测服务器重启。当 Gateway 收到的 `server_id` 与之前不同时，触发全量 relist。重链接（relink）时调用 `regenerate_server_id()` 生成新 ID。

### WatchObj 注册

ResourceProcessor 初始化时通过 `register_watch_obj(kind, Arc<ServerCache<T>>)` 向 ConfigSyncServer 注册。所有注册完成且就绪后（`is_all_ready()` 返回 true），gRPC 服务开始对外提供数据。

### list/watch 操作

| 操作 | 流程 |
|---|---|
| `list(kind)` | 查找 WatchObj → 调用 `list_json()` → 返回 `ListDataSimple { data, sync_version, server_id }` |
| `watch(kind, client_id, ...)` | 查找 WatchObj → 调用 `watch_json()` → 启动转换任务附加 server_id → 返回 `mpsc::Receiver<EventDataSimple>` |

watch 操作会启动一个 tokio 任务，将 `WatchResponseSimple` 转换为 `EventDataSimple`（附加 server_id），确保客户端能检测服务器重启。

### 状态管理

| 方法 | 说明 |
|---|---|
| `is_all_ready()` | 所有 WatchObj 均已就绪 |
| `not_ready_kinds()` | 返回尚未就绪的 kind 列表 |
| `set_all_not_ready()` | 重链接时标记所有缓存未就绪 |
| `clear_all()` | 清空所有缓存数据 |
| `reset_for_relink()` | 完整重置：set_all_not_ready + clear_all + regenerate_server_id |

## 完整数据流

```
K8s API / FileSystem
        │
        ▼
  ConfCenter (watcher)
        │
        ▼
  ResourceProcessor.process_resource()    ← 11 步处理流水线
        │
        ▼
  ServerCache.apply_change()              ← 分配 sync_version，写入 EventStore
        │
        ▼
  Notify.notify_waiters()                 ← 唤醒所有 watcher 任务
        │
        ▼
  Watcher Task → mpsc::channel            ← 发送增量事件
        │
        ▼
  ConfigSyncServer.watch()                ← 附加 server_id
        │
        ▼
  gRPC Stream → Gateway                  ← Gateway 实例接收配置变更
```
