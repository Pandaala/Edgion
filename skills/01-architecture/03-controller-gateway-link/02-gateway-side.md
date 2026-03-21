---
name: link-gateway-side
description: Gateway 侧 gRPC 实现：ConfigSyncClient、ClientCache、CacheEventDispatch、ConfHandler、ArcSwap 无锁读取、重连机制。
---

# Gateway 侧实现

Gateway 通过 `ConfigSyncClient` 连接 Controller 的 gRPC 服务，接收资源配置并写入本地 `ClientCache<T>`。每种资源类型通过 `ConfHandler` 将缓存数据转化为运行时代理行为，使用 ArcSwap 实现无锁读取。

## ConfigSyncClient

管理与 Controller 的 gRPC 连接，负责 List/Watch 调用、版本跟踪、断线重连。

```rust
pub struct ConfigSyncClient {
    config_client: Arc<ConfigClient>,
    conf_client_handle: ConfigSyncClientService<Channel>,
    client_id: String,
    client_name: String,
}
```

### 启动同步序列

1. **连接建立**：通过 tonic 连接 Controller 的 gRPC 端口（默认 :50051）
2. **GetServerInfo**：获取 `server_id`、`endpoint_mode`、`supported_kinds`
3. **逐 kind List**：对 `supported_kinds` 中的每种资源执行 List 调用，获取全量快照
4. **逐 kind Watch**：使用 List 返回的 `sync_version` 作为 `from_version`，启动 Watch 流
5. **WatchServerMeta**：注册到 Controller 的 ClientRegistry，接收 `gateway_instance_count` 变更

### 重连机制

ConfigSyncClient 检测到以下情况时触发重连：

| 触发条件 | 行为 |
|---------|------|
| Watch 流收到 `WATCH_ERR_SERVER_RELOAD` | Controller 执行了 reload，全量 relist |
| Watch 流收到 `WATCH_ERR_SERVER_ID_MISMATCH` | Controller 重启，重新 GetServerInfo + 全量 relist |
| gRPC 连接断开 | 指数退避重连，重新建立连接并全量同步 |
| `server_id` 变化 | 全量 relist 所有 kind |

重连后 Gateway 重新执行完整的同步序列（GetServerInfo → List → Watch），确保配置与 Controller 完全一致。

## ClientCache\<T\>

每种资源类型一个缓存实例，存储从 Controller 同步的资源数据。

```rust
pub struct ClientCache<T> where T: kube::Resource {
    pub(crate) cache_data: Arc<RwLock<CacheData<T>>>,
    pub(crate) grpc_client: Arc<AsyncRwLock<Option<ConfigSyncClientService<Channel>>>>,
    pub(crate) client_id: Arc<String>,
    pub(crate) client_name: Arc<String>,
}
```

### CacheData\<T\>

ClientCache 内部持有 `CacheData<T>`（RwLock 保护），包含：
- 全量资源快照（`HashMap<String, T>`）
- 当前 `sync_version`
- `server_id`（用于检测 Controller 重启）

### DynClientCache

类型擦除的统一调度接口，使 ConfigSyncClient 能以统一方式管理不同类型的 ClientCache：

```
ConfigSyncClient
  └── Vec<Arc<dyn DynClientCache>>   // 按 kind 索引
       ├── ClientCache<Gateway>
       ├── ClientCache<HTTPRoute>
       ├── ClientCache<GRPCRoute>
       └── ...
```

## 事件分发（CacheEventDispatch）

`CacheEventDispatch<T>` trait 定义了缓存事件的处理接口：

```rust
pub trait CacheEventDispatch<T> {
    fn apply_change(&self, change: ResourceChange, resource: T) where T: Send + 'static;
    fn set_ready(&self);
}
```

ClientCache 实现此 trait，将 gRPC 事件（来自 Watch 流的 WatchResponse）映射到 ResourceChange 操作：

| Watch 事件 | ResourceChange | 处理逻辑 |
|-----------|---------------|---------|
| 初始化开始 | `InitStart` | 记录日志，准备接收初始化数据 |
| 初始化数据 | `InitAdd` | 写入 CacheData 快照（不触发 ConfHandler） |
| 初始化完成 | `InitDone` | 标记缓存就绪，调用 ConfHandler.full_set() |
| 运行时新增 | `EventAdd` | 更新 CacheData + 调用 ConfHandler.partial_update(add) |
| 运行时更新 | `EventUpdate` | 更新 CacheData + 调用 ConfHandler.partial_update(update) |
| 运行时删除 | `EventDelete` | 从 CacheData 移除 + 调用 ConfHandler.partial_update(remove) |

事件分发还会压缩事件批次——多个连续的增量事件可合并为一次 partial_update 调用，减少运行时结构的重建频率。

## ConfHandler

每种资源类型的配置处理器，定义在 `src/core/common/conf_sync/traits.rs`：

```rust
pub trait ConfHandler<T>: Send + Sync {
    fn full_set(&self, data: &HashMap<String, T>);
    fn partial_update(&self, add: HashMap<String, T>, update: HashMap<String, T>, remove: HashSet<String>);
}
```

### full_set

初始化同步完成时调用（InitDone 之后），接收全量资源快照。ConfHandler 据此构建完整的运行时结构并通过 ArcSwap 原子替换。

### partial_update

运行时增量变更时调用，接收三组数据：
- `add`：新增的资源
- `update`：更新的资源
- `remove`：删除的资源键集合

ConfHandler 据此增量更新运行时结构。对于复杂资源（如路由），partial_update 可能触发部分或全量重建，取决于变更的影响范围。

### Preparse（预解析）

ConfHandler 在接收到变更后会触发 preparse 操作，将 Kubernetes 风格的声明式资源转换为高效的运行时数据结构。例如：
- HTTPRoute → 路由匹配树
- EdgionPlugins → 插件执行链
- EdgionTls → TLS 证书和密钥配置
- Gateway → 监听器绑定和端口映射

## ArcSwap 无锁读取

Gateway 使用 ArcSwap 实现配置数据的无锁读取：

```
写入路径（Tokio 运行时）:
  gRPC Watch → CacheEventDispatch → ConfHandler → ArcSwap.store()

读取路径（Pingora 代理层）:
  请求处理 → ArcSwap.load() → 读取最新配置
```

这种设计确保：
- **写入不阻塞读取**：配置更新通过原子指针替换完成，读取方始终看到一致的快照
- **零拷贝读取**：Pingora 线程通过 `Arc` 引用计数共享数据，无需克隆
- **无锁**：读取路径不涉及任何锁竞争，适合高并发代理场景

## WatchServerMeta

Gateway 通过 WatchServerMeta RPC 接收集群级元数据：

```rust
message ServerMetaEvent {
    string server_id = 1;
    uint32 gateway_instance_count = 2;
    uint64 timestamp = 10;
}
```

`gateway_instance_count` 反映当前连接到 Controller 的 Gateway 实例总数。Gateway 使用此信息进行集群级感知，典型用途是集群级限流——单实例配额 = 集群总配额 / `gateway_instance_count`。

当有新的 Gateway 实例上线或下线时，Controller 的 ClientRegistry 会推送更新的 count 给所有已连接实例。

## ConfigClient

聚合所有资源类型的 ClientCache，提供类型安全的查询接口：

```
ConfigClient
  ├── ClientCache<Gateway>
  ├── ClientCache<GatewayClass>
  ├── ClientCache<HTTPRoute>
  ├── ClientCache<GRPCRoute>
  ├── ClientCache<TCPRoute>
  ├── ClientCache<TLSRoute>
  ├── ClientCache<UDPRoute>
  ├── ClientCache<EdgionTls>
  ├── ClientCache<EdgionPlugins>
  ├── ClientCache<EdgionStreamPlugins>
  ├── ClientCache<PluginMetaData>
  ├── ClientCache<BackendTLSPolicy>
  ├── ClientCache<EdgionGatewayConfig>
  ├── ClientCache<LinkSys>
  ├── ClientCache<EdgionAcme>
  └── ClientCache<Service/EndpointSlice/Endpoint>
```

ConfigClient 还提供 `is_ready()` 方法，检查所有必需资源是否已完成初始化同步。Gateway 只有在 ConfigClient 就绪后才开始接受流量。

## 模块结构

```
src/core/gateway/conf_sync/
├── conf_client/
│   ├── mod.rs              # 模块导出
│   ├── config_client.rs    # ConfigClient 聚合层
│   └── grpc_client.rs      # ConfigSyncClient gRPC 客户端
└── cache_client/
    ├── mod.rs              # 模块导出
    ├── cache.rs            # ClientCache<T>、DynClientCache
    ├── cache_data.rs       # CacheData<T> 数据存储
    └── event_dispatch.rs   # CacheEventDispatch 实现
```
