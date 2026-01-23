# ConfigServer 与 ResourceProcessor 重构规格文档

## 1. 概述

### 1.1 重构目标

将 `ServerCache<T>` 从 `conf_server` 移到 `ResourceProcessor` 中，实现：

1. **类型安全**：所有 apply/list/delete 操作都通过 typed `ResourceProcessor<T>` 完成
2. **职责清晰**：Processor 负责资源的完整生命周期管理（包括缓存）
3. **接口简化**：`conf_server` 只提供 gRPC watch/list 接口（本身就需要序列化）
4. **消除冗余**：删除 `conf_change_apply.rs` 和重复的 typed fields

### 1.2 重构策略

采用**渐进式重构**：
- **Phase 1**：重构 `conf_server`，简化为只提供 watch/list 的 gRPC 服务
- **Phase 2**：重构 `conf_mgr`，新建 `conf_mgr_new` 目录，逐步迁移功能

---

## 2. 目标架构

### 2.1 整体架构图

```
┌─────────────────────────────────────────────────────────────────────┐
│                        Controller Runtime                            │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  ┌────────────────────────────────────────────────────────────────┐ │
│  │                    ProcessorRegistry (全局)                      │ │
│  │           RwLock<HashMap<&'static str, Arc<dyn ProcessorObj>>>  │ │
│  │                                                                  │ │
│  │  ┌─────────────────────┐  ┌─────────────────────┐               │ │
│  │  │ HttpRouteProcessor  │  │ GatewayProcessor    │  ...          │ │
│  │  │  ├─ ServerCache<T>  │  │  ├─ ServerCache<T>  │               │ │
│  │  │  ├─ Store (K8s)     │  │  ├─ Store (K8s)     │               │ │
│  │  │  ├─ Workqueue       │  │  ├─ Workqueue       │               │ │
│  │  │  └─ ProcessConfig   │  │  └─ ProcessConfig   │               │ │
│  │  └─────────────────────┘  └─────────────────────┘               │ │
│  └────────────────────────────────────────────────────────────────┘ │
│         │                              │                             │
│         │ (typed access)               │ (as WatchObj)              │
│         ▼                              ▼                             │
│  ┌─────────────────┐          ┌────────────────────────────────┐   │
│  │   Admin API     │          │       ConfigSyncServer         │   │
│  │   (CRUD ops)    │          │  (gRPC list/watch only)        │   │
│  └─────────────────┘          │  HashMap<kind, Arc<dyn WatchObj>>│   │
│                               └────────────────────────────────┘   │
│                                          │                          │
│                                          ▼                          │
│                               ┌──────────────────────┐              │
│                               │    Gateway Nodes     │              │
│                               │  (config consumers)  │              │
│                               └──────────────────────┘              │
└─────────────────────────────────────────────────────────────────────┘
```

### 2.2 核心组件职责

| 组件 | 职责 |
|------|------|
| `ProcessorRegistry` | 全局管理所有 Processor 实例，提供 typed 和 dynamic 访问 |
| `ResourceProcessor<T>` | 完整的资源生命周期管理：filter, validate, parse, save, delete, cache |
| `ConfigSyncServer` | 只做 gRPC list/watch，通过 `WatchObj` trait 访问 |
| `WatchObj` trait | 极简接口：`list_json()`, `watch()`, `state()`, `kind_name()` |

---

## 3. Phase 1: 重构 conf_server

### 3.1 目标

将 `conf_server` 简化为纯粹的 gRPC 服务层，只负责序列化输出。

### 3.2 变更内容

#### 3.2.1 新增 `WatchObj` trait

```rust
// src/core/conf_sync/conf_server/traits.rs

/// 极简的 Watch 接口，只用于 gRPC list/watch
/// 所有方法都涉及序列化，所以用 trait object 是合理的
pub trait WatchObj: Send + Sync {
    /// 资源类型名称
    fn kind_name(&self) -> &'static str;
    
    /// List 所有资源 (JSON 序列化)
    fn list_json(&self) -> Result<(String, u64), String>;
    
    /// Watch 资源变更
    fn watch_json(
        &self,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> mpsc::Receiver<WatchResponseSimple>;
    
    /// 缓存状态
    fn state(&self) -> CacheState;
    
    /// 是否就绪
    fn is_ready(&self) -> bool;
    
    /// 设置就绪状态
    fn set_ready(&self);
}
```

#### 3.2.2 简化 `ConfigServer` → `ConfigSyncServer`

```rust
// src/core/conf_sync/conf_server/config_server.rs

/// 简化的配置同步服务器
/// 只负责 gRPC list/watch，不再持有 typed caches
pub struct ConfigSyncServer {
    /// Server instance ID
    server_id: RwLock<String>,
    
    /// Endpoint mode
    endpoint_mode: RwLock<Option<EndpointMode>>,
    
    /// Watch objects by kind (从 ProcessorRegistry 注册)
    watch_objects: RwLock<HashMap<String, Arc<dyn WatchObj>>>,
}

impl ConfigSyncServer {
    /// 注册一个 WatchObj (由 Processor 初始化时调用)
    pub fn register_watch_obj(&self, kind: &str, obj: Arc<dyn WatchObj>);
    
    /// List 资源
    pub fn list(&self, kind: &str) -> Result<(String, u64), String>;
    
    /// Watch 资源
    pub fn watch(&self, kind: &str, ...) -> Option<mpsc::Receiver<WatchResponseSimple>>;
    
    /// 检查是否所有资源都就绪
    pub fn is_all_ready(&self) -> bool;
}
```

#### 3.2.3 删除的文件/代码

| 删除项 | 说明 |
|--------|------|
| `conf_change_apply.rs` | 所有 `apply_*_change` 方法移到 Processor |
| `factory.rs` 中的 typed fields | 不再需要，cache 由 Processor 持有 |
| `config_server.rs` 中的 backward compatibility fields | 同上 |

#### 3.2.4 保留的功能

- `grpc_server.rs` 基本不变，但使用新的 `ConfigSyncServer`
- `ServerCache<T>` 实现保留，但移交给 Processor 管理

---

## 4. Phase 2: 重构 conf_mgr

### 4.1 策略

新建 `conf_mgr_new` 目录，逐步迁移和重构功能，最终替换原有 `conf_mgr`。

### 4.2 目录结构

```
src/core/conf_mgr_new/
├── mod.rs                      # 模块入口
├── processor_registry.rs       # ProcessorRegistry 全局管理
├── sync_runtime/
│   ├── mod.rs
│   ├── workqueue.rs            # 复用现有实现
│   ├── shutdown.rs             # 复用现有实现
│   ├── metrics.rs              # 复用现有实现
│   └── resource_processor/
│       ├── mod.rs              # 增强的 ResourceProcessor trait
│       ├── processor_obj.rs    # ProcessorObj trait (object-safe)
│       ├── base_processor.rs   # 通用处理逻辑
│       ├── gateway.rs
│       ├── http_route.rs
│       ├── secret.rs
│       ├── edgion_tls.rs
│       └── ... (其他 processors)
├── conf_center/
│   ├── mod.rs
│   ├── kubernetes/
│   │   ├── mod.rs
│   │   └── controller.rs       # 简化的 Controller
│   └── file_system/
│       └── ...
└── schema_validator.rs         # 复用
```

### 4.3 核心设计

#### 4.3.1 增强的 `ResourceProcessor<T>`

```rust
// src/core/conf_mgr_new/sync_runtime/resource_processor/mod.rs

/// 增强的 ResourceProcessor，包含完整的资源管理能力
pub struct ResourceProcessor<K>
where
    K: Resource + Clone + Send + Sync + Debug + DeserializeOwned + Serialize + 'static,
{
    /// 资源类型名称
    kind: &'static str,
    
    /// 资源缓存 (之前在 ConfigServer)
    cache: Arc<ServerCache<K>>,
    
    /// K8s Store (reflector store)
    store: Option<Arc<Store<K>>>,
    
    /// 工作队列
    workqueue: Arc<Workqueue>,
    
    /// 处理配置
    process_config: Option<Arc<ProcessConfig>>,
    
    /// 命名空间过滤
    namespace_filter: Option<Arc<Vec<String>>>,
    
    /// 跨资源 Requeue 注册表
    requeue_registry: Arc<RequeueRegistry>,
    
    /// Secret 引用管理器 (共享)
    secret_ref_manager: Arc<SecretRefManager>,
    
    /// 处理逻辑 trait object
    handler: Arc<dyn ProcessorHandler<K>>,
}

impl<K> ResourceProcessor<K> {
    // ==================== 生命周期方法 ====================
    
    /// 处理 Init 事件（LIST 开始）
    pub fn on_init(&self);
    
    /// 处理 InitApply 事件（LIST 返回的对象）
    pub fn on_init_apply(&self, obj: K) -> bool;
    
    /// 处理 InitDone 事件（LIST 完成）
    pub fn on_init_done(&self);
    
    /// 处理 Apply 事件（运行时 WATCH 到的对象）
    pub fn on_apply(&self, obj: K);
    
    /// 处理 Delete 事件
    pub fn on_delete(&self, obj: K);
    
    // ==================== 缓存操作 ====================
    
    /// 获取资源
    pub fn get(&self, key: &str) -> Option<K>;
    
    /// 列出所有资源
    pub fn list(&self) -> Vec<K>;
    
    /// 保存资源到缓存
    pub fn save(&self, obj: K);
    
    /// 从缓存删除资源
    pub fn remove(&self, key: &str);
    
    // ==================== Requeue 接口 ====================
    
    /// 入队 key（供外部调用，如 Secret 变更触发级联）
    pub fn requeue(&self, key: String);
    
    /// 入队 key 并延迟
    pub fn requeue_after(&self, key: String, duration: Duration);
}
```

#### 4.3.2 `ProcessorHandler<T>` trait（处理逻辑）

```rust
/// 资源处理逻辑（由各资源类型实现）
pub trait ProcessorHandler<K>: Send + Sync
where
    K: Resource + Clone + Send + Sync + 'static,
{
    /// 过滤资源
    fn filter(&self, obj: &K) -> bool { true }
    
    /// 清理元数据
    fn clean_metadata(&self, obj: &mut K, config: &MetadataFilterConfig);
    
    /// 验证资源
    fn validate(&self, obj: &K, ctx: &HandlerContext) -> Vec<String> { vec![] }
    
    /// 解析资源（如 Secret 引用解析）
    fn parse(&self, obj: K, ctx: &HandlerContext) -> ProcessResult<K>;
    
    /// 删除时的清理
    fn on_delete(&self, obj: &K, ctx: &HandlerContext) {}
    
    /// 变更后的处理（如级联 requeue）
    fn on_change(&self, obj: &K, ctx: &HandlerContext) {}
}
```

#### 4.3.3 `ProcessorObj` trait（object-safe）

```rust
/// Object-safe 的 Processor 接口，用于统一管理
pub trait ProcessorObj: Send + Sync {
    /// 资源类型名称
    fn kind(&self) -> &'static str;
    
    /// 获取 WatchObj 用于 gRPC
    fn as_watch_obj(&self) -> Arc<dyn WatchObj>;
    
    /// Requeue 接口
    fn requeue(&self, key: String);
    
    /// 是否就绪
    fn is_ready(&self) -> bool;
    
    /// 设置就绪
    fn set_ready(&self);
}

// ResourceProcessor<K> 实现 ProcessorObj
impl<K> ProcessorObj for ResourceProcessor<K>
where
    K: Resource + Clone + Send + Sync + Debug + DeserializeOwned + Serialize + 'static,
{
    fn kind(&self) -> &'static str { self.kind }
    
    fn as_watch_obj(&self) -> Arc<dyn WatchObj> {
        // cache 实现了 WatchObj
        self.cache.clone()
    }
    
    fn requeue(&self, key: String) {
        self.workqueue.enqueue(key);
    }
    
    fn is_ready(&self) -> bool {
        self.cache.is_ready()
    }
    
    fn set_ready(&self) {
        self.cache.set_ready();
    }
}
```

#### 4.3.4 `ProcessorRegistry`（全局注册表）

```rust
// src/core/conf_mgr_new/processor_registry.rs

use once_cell::sync::Lazy;

/// 全局 Processor 注册表
pub static PROCESSOR_REGISTRY: Lazy<ProcessorRegistry> = Lazy::new(ProcessorRegistry::new);

pub struct ProcessorRegistry {
    /// 所有 processor（按 kind 索引）
    processors: RwLock<HashMap<&'static str, Arc<dyn ProcessorObj>>>,
}

impl ProcessorRegistry {
    /// 注册 processor
    pub fn register<K>(&self, processor: Arc<ResourceProcessor<K>>)
    where
        K: Resource + Clone + Send + Sync + Debug + DeserializeOwned + Serialize + 'static,
    {
        let mut map = self.processors.write().unwrap();
        map.insert(processor.kind(), processor);
    }
    
    /// 获取 processor（typed）
    pub fn get<K>(&self, kind: &str) -> Option<Arc<ResourceProcessor<K>>>
    where
        K: Resource + Clone + Send + Sync + Debug + DeserializeOwned + Serialize + 'static,
    {
        // 通过 downcast 获取 typed processor
        // 注意：这里需要一些技巧来实现类型安全的获取
    }
    
    /// 获取 processor（dynamic）
    pub fn get_dynamic(&self, kind: &str) -> Option<Arc<dyn ProcessorObj>> {
        self.processors.read().unwrap().get(kind).cloned()
    }
    
    /// 获取所有 WatchObj（供 ConfigSyncServer 使用）
    pub fn all_watch_objs(&self) -> HashMap<String, Arc<dyn WatchObj>> {
        self.processors
            .read()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.to_string(), v.as_watch_obj()))
            .collect()
    }
    
    /// 跨资源 requeue
    pub fn requeue(&self, kind: &str, key: String) {
        if let Some(processor) = self.get_dynamic(kind) {
            processor.requeue(key);
        }
    }
}
```

### 4.4 Processor 初始化流程

```rust
// 在 run_with_api 中创建 Processor

async fn run_with_api<K, H>(
    kind: &'static str,
    api: Api<K>,
    handler: Arc<H>,
    config_sync_server: Arc<ConfigSyncServer>,
    // ... other params
) -> Result<()>
where
    K: Resource + Clone + Send + Sync + Debug + DeserializeOwned + Serialize + 'static,
    H: ProcessorHandler<K> + 'static,
{
    // 1. 创建 K8s Store
    let (store, writer) = reflector::store();
    
    // 2. 创建 Workqueue
    let workqueue = Arc::new(Workqueue::new(kind));
    
    // 3. 创建 ServerCache
    let cache = Arc::new(ServerCache::new(capacity));
    
    // 4. 创建 ResourceProcessor
    let processor = Arc::new(ResourceProcessor {
        kind,
        cache: cache.clone(),
        store: Some(Arc::new(store)),
        workqueue: workqueue.clone(),
        process_config,
        namespace_filter,
        requeue_registry,
        secret_ref_manager,
        handler,
    });
    
    // 5. 注册到全局 Registry
    PROCESSOR_REGISTRY.register(processor.clone());
    
    // 6. 注册 WatchObj 到 ConfigSyncServer
    config_sync_server.register_watch_obj(kind, cache.clone());
    
    // 7. 启动 reflector stream 处理
    let stream = Box::pin(reflector(writer, watcher_stream));
    
    while let Some(event) = stream.next().await {
        match event {
            Event::Init => processor.on_init(),
            Event::InitApply(obj) => { processor.on_init_apply(obj); }
            Event::InitDone => processor.on_init_done(),
            Event::Apply(obj) => processor.on_apply(obj),
            Event::Delete(obj) => processor.on_delete(obj),
        }
    }
}
```

---

## 5. 详细重构步骤

### 5.1 重构策略

采用**并行开发**策略，保留旧代码，新建独立目录：

```
src/core/
├── conf_sync/
│   ├── conf_server/          # 旧代码（保留）
│   └── conf_server_new/      # 新代码（Phase 1）
├── conf_mgr/                 # 旧代码（保留）
└── conf_mgr_new/             # 新代码（Phase 2）
```

最终切换时：
1. 删除 `conf_server`，重命名 `conf_server_new` → `conf_server`
2. 删除 `conf_mgr`，重命名 `conf_mgr_new` → `conf_mgr`

---

### 5.2 Phase 1: conf_server_new

#### 目标
创建精简的 `conf_server_new`，只负责 gRPC list/watch。

#### 目录结构

```
src/core/conf_sync/conf_server_new/
├── mod.rs                    # 模块入口
├── traits.rs                 # WatchObj trait 定义
├── config_sync_server.rs     # ConfigSyncServer 实现
└── grpc_server.rs            # gRPC 服务实现
```

#### 步骤详解

##### Step 1.1: 创建目录和 mod.rs

```rust
// src/core/conf_sync/conf_server_new/mod.rs

mod traits;
mod config_sync_server;
mod grpc_server;

pub use traits::{WatchObj, WatchResponseSimple};
pub use config_sync_server::ConfigSyncServer;
pub use grpc_server::ConfigSyncGrpcServer;
```

##### Step 1.2: 实现 WatchObj trait

```rust
// src/core/conf_sync/conf_server_new/traits.rs

use tokio::sync::mpsc;

/// Watch 响应（简化版）
#[derive(Debug, Clone)]
pub struct WatchResponseSimple {
    pub data: String,
    pub sync_version: u64,
    pub err: Option<String>,
}

/// 极简的 Watch 接口
/// 只用于 gRPC list/watch，所有方法都涉及序列化
pub trait WatchObj: Send + Sync {
    /// 资源类型名称
    fn kind_name(&self) -> &'static str;
    
    /// List 所有资源 (JSON)
    fn list_json(&self) -> Result<(String, u64), String>;
    
    /// Watch 资源变更
    fn watch_json(
        &self,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> mpsc::Receiver<WatchResponseSimple>;
    
    /// 是否就绪
    fn is_ready(&self) -> bool;
    
    /// 设置就绪状态
    fn set_ready(&self);
    
    /// 设置未就绪状态  
    fn set_not_ready(&self);
    
    /// 清空缓存
    fn clear(&self);
}
```

##### Step 1.3: 实现 ConfigSyncServer

```rust
// src/core/conf_sync/conf_server_new/config_sync_server.rs

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use super::traits::{WatchObj, WatchResponseSimple};
use tokio::sync::mpsc;

/// 精简的配置同步服务器
/// 只负责 gRPC list/watch，不持有 typed caches
pub struct ConfigSyncServer {
    /// Server instance ID
    server_id: RwLock<String>,
    
    /// Endpoint mode
    endpoint_mode: RwLock<Option<EndpointMode>>,
    
    /// Watch objects by kind (由 Processor 注册)
    watch_objects: RwLock<HashMap<String, Arc<dyn WatchObj>>>,
}

impl ConfigSyncServer {
    pub fn new() -> Self {
        Self {
            server_id: RwLock::new(uuid::Uuid::new_v4().to_string()),
            endpoint_mode: RwLock::new(None),
            watch_objects: RwLock::new(HashMap::new()),
        }
    }
    
    /// 注册 WatchObj（由 Processor 初始化时调用）
    pub fn register_watch_obj(&self, kind: &str, obj: Arc<dyn WatchObj>) {
        let mut map = self.watch_objects.write().unwrap();
        map.insert(kind.to_string(), obj);
    }
    
    /// 批量注册
    pub fn register_all(&self, objs: HashMap<String, Arc<dyn WatchObj>>) {
        let mut map = self.watch_objects.write().unwrap();
        map.extend(objs);
    }
    
    /// List 资源
    pub fn list(&self, kind: &str) -> Result<(String, u64), String> {
        let map = self.watch_objects.read().unwrap();
        match map.get(kind) {
            Some(obj) => obj.list_json(),
            None => Err(format!("Unknown kind: {}", kind)),
        }
    }
    
    /// Watch 资源
    pub fn watch(
        &self,
        kind: &str,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> Option<mpsc::Receiver<WatchResponseSimple>> {
        let map = self.watch_objects.read().unwrap();
        map.get(kind).map(|obj| obj.watch_json(client_id, client_name, from_version))
    }
    
    /// 检查所有资源是否就绪
    pub fn is_all_ready(&self) -> bool {
        let map = self.watch_objects.read().unwrap();
        map.values().all(|obj| obj.is_ready())
    }
    
    /// 获取未就绪的 kinds
    pub fn not_ready_kinds(&self) -> Vec<String> {
        let map = self.watch_objects.read().unwrap();
        map.iter()
            .filter(|(_, obj)| !obj.is_ready())
            .map(|(k, _)| k.clone())
            .collect()
    }
    
    /// 获取所有注册的 kinds
    pub fn all_kinds(&self) -> Vec<String> {
        let map = self.watch_objects.read().unwrap();
        map.keys().cloned().collect()
    }
    
    /// 获取 server_id
    pub fn server_id(&self) -> String {
        self.server_id.read().unwrap().clone()
    }
    
    /// 设置 endpoint_mode
    pub fn set_endpoint_mode(&self, mode: EndpointMode) {
        *self.endpoint_mode.write().unwrap() = Some(mode);
    }
    
    /// 获取 endpoint_mode
    pub fn endpoint_mode(&self) -> Option<EndpointMode> {
        self.endpoint_mode.read().unwrap().clone()
    }
}
```

##### Step 1.4: 实现 gRPC Server

```rust
// src/core/conf_sync/conf_server_new/grpc_server.rs

use std::sync::Arc;
use tonic::{Request, Response, Status};
use super::ConfigSyncServer;

/// gRPC ConfigSync 服务实现
pub struct ConfigSyncGrpcServer {
    server: Arc<ConfigSyncServer>,
}

impl ConfigSyncGrpcServer {
    pub fn new(server: Arc<ConfigSyncServer>) -> Self {
        Self { server }
    }
}

#[tonic::async_trait]
impl ConfigSync for ConfigSyncGrpcServer {
    async fn list(&self, request: Request<ListRequest>) -> Result<Response<ListResponse>, Status> {
        let req = request.get_ref();
        
        // 验证 server_id
        if req.server_id != self.server.server_id() {
            return Err(Status::invalid_argument("Server ID mismatch"));
        }
        
        match self.server.list(&req.kind) {
            Ok((data, version)) => Ok(Response::new(ListResponse {
                data,
                sync_version: version,
                err: String::new(),
            })),
            Err(e) => Err(Status::not_found(e)),
        }
    }
    
    type WatchStream = /* ... */;
    
    async fn watch(&self, request: Request<WatchRequest>) -> Result<Response<Self::WatchStream>, Status> {
        let req = request.get_ref();
        
        // 验证 server_id
        if req.server_id != self.server.server_id() {
            return Err(Status::invalid_argument("Server ID mismatch"));
        }
        
        match self.server.watch(&req.kind, req.client_id.clone(), req.client_name.clone(), req.from_version) {
            Some(receiver) => {
                // 转换为 gRPC stream
                // ...
            }
            None => Err(Status::not_found(format!("Unknown kind: {}", req.kind))),
        }
    }
}
```

##### Step 1.5: ServerCache<T> 实现 WatchObj

在 `src/core/conf_sync/cache_server/cache.rs` 中为 `ServerCache<T>` 实现 `WatchObj`：

```rust
// 添加到现有的 cache.rs

use crate::core::conf_sync::conf_server_new::WatchObj;

impl<T> WatchObj for ServerCache<T>
where
    T: Clone + Send + Sync + Serialize + ResourceMeta + Resource + 'static,
{
    fn kind_name(&self) -> &'static str {
        T::kind(&())
    }
    
    fn list_json(&self) -> Result<(String, u64), String> {
        // 复用现有实现
        self.list_json_internal()
    }
    
    fn watch_json(...) -> mpsc::Receiver<WatchResponseSimple> {
        // 复用现有实现
    }
    
    fn is_ready(&self) -> bool {
        self.state.read().unwrap().is_ready
    }
    
    fn set_ready(&self) {
        self.state.write().unwrap().is_ready = true;
    }
    
    fn set_not_ready(&self) {
        self.state.write().unwrap().is_ready = false;
    }
    
    fn clear(&self) {
        self.data.write().unwrap().clear();
    }
}
```

##### Step 1.6: 更新 conf_sync/mod.rs

```rust
// src/core/conf_sync/mod.rs

pub mod cache_server;
pub mod conf_server;      // 旧代码（暂时保留）
pub mod conf_server_new;  // 新代码

// 切换时只需改这里
pub use conf_server_new::{ConfigSyncServer, WatchObj, ConfigSyncGrpcServer};
```

---

### 5.3 Phase 2: conf_mgr_new

#### 目标
创建增强的 `conf_mgr_new`，`ResourceProcessor` 持有 `ServerCache<T>`。

#### 目录结构（按创建顺序）

```
src/core/conf_mgr_new/
├── mod.rs                          # Step 2.1
├── processor_registry.rs           # Step 2.2
├── sync_runtime/
│   ├── mod.rs                      # Step 2.3
│   ├── workqueue.rs                # Step 2.4 (复用)
│   ├── shutdown.rs                 # Step 2.4 (复用)
│   ├── metrics.rs                  # Step 2.4 (复用)
│   └── resource_processor/
│       ├── mod.rs                  # Step 2.5
│       ├── processor.rs            # Step 2.6 (核心)
│       ├── handler.rs              # Step 2.7
│       ├── context.rs              # Step 2.8
│       ├── secret_utils/           # Step 2.9 (复用)
│       │   ├── mod.rs
│       │   ├── secret_ref.rs
│       │   └── secret_store.rs
│       ├── validation.rs           # Step 2.10 (复用)
│       └── handlers/               # Step 2.11 (各资源实现)
│           ├── mod.rs
│           ├── gateway.rs
│           ├── http_route.rs
│           ├── secret.rs
│           ├── edgion_tls.rs
│           └── ... (其他)
├── conf_center/                    # Step 2.12 (最后添加)
│   ├── mod.rs
│   ├── config.rs
│   ├── traits.rs
│   ├── kubernetes/
│   │   ├── mod.rs
│   │   ├── controller.rs
│   │   └── resource_runner.rs      # 替代 resource_controller.rs
│   └── file_system/
│       └── ...
└── schema_validator.rs             # Step 2.13 (复用)
```

#### 步骤详解

##### Step 2.1: 创建 mod.rs 入口

```rust
// src/core/conf_mgr_new/mod.rs

pub mod processor_registry;
pub mod sync_runtime;
// pub mod conf_center;  // 最后添加

pub use processor_registry::{ProcessorRegistry, PROCESSOR_REGISTRY};
pub use sync_runtime::resource_processor::{
    ResourceProcessor, ProcessorHandler, ProcessorObj, HandlerContext,
};
```

##### Step 2.2: 实现 ProcessorRegistry

```rust
// src/core/conf_mgr_new/processor_registry.rs

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use once_cell::sync::Lazy;

use super::sync_runtime::resource_processor::ProcessorObj;
use crate::core::conf_sync::conf_server_new::WatchObj;

/// 全局 Processor 注册表
pub static PROCESSOR_REGISTRY: Lazy<ProcessorRegistry> = Lazy::new(ProcessorRegistry::new);

pub struct ProcessorRegistry {
    /// 所有 processor（按 kind 索引）
    processors: RwLock<HashMap<&'static str, Arc<dyn ProcessorObj>>>,
}

impl ProcessorRegistry {
    pub fn new() -> Self {
        Self {
            processors: RwLock::new(HashMap::new()),
        }
    }
    
    /// 注册 processor（内部使用）
    pub fn register(&self, processor: Arc<dyn ProcessorObj>) {
        let kind = processor.kind();
        let mut map = self.processors.write().unwrap();
        tracing::info!(kind = kind, "Registering processor");
        map.insert(kind, processor);
    }
    
    /// 获取 processor（dynamic）
    pub fn get(&self, kind: &str) -> Option<Arc<dyn ProcessorObj>> {
        self.processors.read().unwrap().get(kind).cloned()
    }
    
    /// 获取所有 WatchObj（供 ConfigSyncServer 使用）
    pub fn all_watch_objs(&self) -> HashMap<String, Arc<dyn WatchObj>> {
        self.processors
            .read()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.to_string(), v.as_watch_obj()))
            .collect()
    }
    
    /// 跨资源 requeue
    pub fn requeue(&self, kind: &str, key: String) {
        if let Some(processor) = self.get(kind) {
            processor.requeue(key);
        } else {
            tracing::warn!(kind = kind, key = %key, "Requeue failed: processor not found");
        }
    }
    
    /// 获取所有注册的 kinds
    pub fn all_kinds(&self) -> Vec<&'static str> {
        self.processors.read().unwrap().keys().copied().collect()
    }
    
    /// 检查是否所有 processor 都就绪
    pub fn is_all_ready(&self) -> bool {
        self.processors.read().unwrap().values().all(|p| p.is_ready())
    }
    
    /// 清空所有注册（用于测试）
    #[cfg(test)]
    pub fn clear(&self) {
        self.processors.write().unwrap().clear();
    }
}
```

##### Step 2.3: 创建 sync_runtime/mod.rs

```rust
// src/core/conf_mgr_new/sync_runtime/mod.rs

pub mod workqueue;
pub mod shutdown;
pub mod metrics;
pub mod resource_processor;

pub use workqueue::{Workqueue, WorkItem};
pub use shutdown::ShutdownSignal;
pub use resource_processor::{
    ResourceProcessor, ProcessorHandler, ProcessorObj, HandlerContext,
};
```

##### Step 2.4: 复用 workqueue/shutdown/metrics

直接从旧代码复制或 `pub use` 引用：

```rust
// 方案 A: 直接复制文件
// 方案 B: 使用 pub use 引用旧代码
pub use crate::core::conf_mgr::conf_center::sync_runtime::workqueue;
```

##### Step 2.5: 创建 resource_processor/mod.rs

```rust
// src/core/conf_mgr_new/sync_runtime/resource_processor/mod.rs

mod processor;
mod handler;
mod context;
pub mod secret_utils;
pub mod validation;
pub mod handlers;

pub use processor::{ResourceProcessor, ProcessorObj};
pub use handler::ProcessorHandler;
pub use context::HandlerContext;

// Re-export secret_utils
pub use secret_utils::{SecretRefManager, SecretStore, get_secret, update_secrets};
```

##### Step 2.6: 实现核心 ResourceProcessor

```rust
// src/core/conf_mgr_new/sync_runtime/resource_processor/processor.rs

use std::sync::Arc;
use std::time::Duration;
use kube::runtime::reflector::Store;
use kube::Resource;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;

use crate::core::conf_sync::cache_server::ServerCache;
use crate::core::conf_sync::conf_server_new::WatchObj;
use crate::core::conf_sync::traits::ResourceChange;
use super::{ProcessorHandler, HandlerContext, Workqueue, SecretRefManager};
use crate::core::conf_mgr_new::ProcessorRegistry;

/// Object-safe 的 Processor 接口
pub trait ProcessorObj: Send + Sync {
    fn kind(&self) -> &'static str;
    fn as_watch_obj(&self) -> Arc<dyn WatchObj>;
    fn requeue(&self, key: String);
    fn requeue_after(&self, key: String, duration: Duration);
    fn is_ready(&self) -> bool;
    fn set_ready(&self);
    fn set_not_ready(&self);
    fn clear(&self);
}

/// 增强的 ResourceProcessor，持有 ServerCache
pub struct ResourceProcessor<K>
where
    K: Resource + Clone + Send + Sync + Debug + Serialize + for<'de> Deserialize<'de> + 'static,
{
    /// 资源类型名称
    kind: &'static str,
    
    /// 资源缓存（核心变更：由 Processor 持有）
    cache: Arc<ServerCache<K>>,
    
    /// K8s Store (reflector store，运行时设置)
    store: RwLock<Option<Arc<Store<K>>>>,
    
    /// 工作队列
    workqueue: Arc<Workqueue>,
    
    /// 处理配置
    process_config: RwLock<Option<Arc<ProcessConfig>>>,
    
    /// 命名空间过滤
    namespace_filter: RwLock<Option<Arc<Vec<String>>>>,
    
    /// Secret 引用管理器（共享）
    secret_ref_manager: Arc<SecretRefManager>,
    
    /// 处理逻辑
    handler: Arc<dyn ProcessorHandler<K>>,
}

impl<K> ResourceProcessor<K>
where
    K: Resource + Clone + Send + Sync + Debug + Serialize + for<'de> Deserialize<'de> + 'static,
{
    pub fn new(
        kind: &'static str,
        capacity: usize,
        handler: Arc<dyn ProcessorHandler<K>>,
        secret_ref_manager: Arc<SecretRefManager>,
    ) -> Self {
        Self {
            kind,
            cache: Arc::new(ServerCache::new(capacity)),
            store: RwLock::new(None),
            workqueue: Arc::new(Workqueue::new(kind)),
            process_config: RwLock::new(None),
            namespace_filter: RwLock::new(None),
            secret_ref_manager,
            handler,
        }
    }
    
    /// 设置 K8s Store（运行时调用）
    pub fn set_store(&self, store: Arc<Store<K>>) {
        *self.store.write().unwrap() = Some(store);
    }
    
    /// 设置处理配置
    pub fn set_process_config(&self, config: Arc<ProcessConfig>) {
        *self.process_config.write().unwrap() = Some(config);
    }
    
    /// 设置命名空间过滤
    pub fn set_namespace_filter(&self, filter: Vec<String>) {
        *self.namespace_filter.write().unwrap() = Some(Arc::new(filter));
    }
    
    /// 获取 workqueue（供 runner 使用）
    pub fn workqueue(&self) -> Arc<Workqueue> {
        self.workqueue.clone()
    }
    
    /// 获取 cache（供 runner 使用）
    pub fn cache(&self) -> Arc<ServerCache<K>> {
        self.cache.clone()
    }
    
    // ==================== 生命周期方法 ====================
    
    /// 处理 Init 事件
    pub fn on_init(&self) {
        tracing::info!(kind = self.kind, "Init started");
        self.cache.set_not_ready();
    }
    
    /// 处理 InitApply 事件（直接处理，不入队）
    pub fn on_init_apply(&self, obj: K) -> bool {
        let ctx = self.create_context();
        self.process_resource(obj, &ctx, true)
    }
    
    /// 处理 InitDone 事件
    pub fn on_init_done(&self) {
        self.cache.set_ready();
        tracing::info!(kind = self.kind, "Init done, cache ready");
    }
    
    /// 处理 Apply 事件（入队）
    pub fn on_apply(&self, obj: &K) {
        let key = make_resource_key(obj);
        self.workqueue.enqueue(key);
    }
    
    /// 处理 Delete 事件（入队）
    pub fn on_delete(&self, obj: &K) {
        let key = make_resource_key(obj);
        self.workqueue.enqueue(key);
    }
    
    /// Worker 处理单个 work item
    pub fn process_work_item(&self, key: &str) {
        let store = self.store.read().unwrap();
        let ctx = self.create_context();
        
        // 对比 store vs cache
        let store_obj = store.as_ref().and_then(|s| {
            let obj_ref = parse_key_to_obj_ref(key);
            s.get(&obj_ref)
        });
        let cache_obj = self.cache.get(key);
        
        match (store_obj, cache_obj) {
            (Some(obj), _) => {
                // 存在于 store → 处理
                self.process_resource(obj, &ctx, false);
            }
            (None, Some(cached)) => {
                // 不在 store 但在 cache → 删除
                self.process_delete(&cached, &ctx);
            }
            (None, None) => {
                // 都不存在 → 已处理，跳过
                tracing::debug!(kind = self.kind, key = key, "Already processed, skipping");
            }
        }
    }
    
    // ==================== 缓存操作 ====================
    
    /// 获取资源
    pub fn get(&self, key: &str) -> Option<K> {
        self.cache.get(key)
    }
    
    /// 列出所有资源
    pub fn list(&self) -> Vec<K> {
        self.cache.list_all()
    }
    
    /// 保存资源到缓存
    pub fn save(&self, obj: K) {
        self.cache.apply_change(ResourceChange::EventUpdate, obj);
    }
    
    /// 从缓存删除资源
    pub fn remove(&self, key: &str) {
        self.cache.remove(key);
    }
    
    // ==================== 内部方法 ====================
    
    fn create_context(&self) -> HandlerContext {
        HandlerContext {
            process_config: self.process_config.read().unwrap().clone(),
            namespace_filter: self.namespace_filter.read().unwrap().clone(),
            secret_ref_manager: self.secret_ref_manager.clone(),
            requeue_fn: Box::new(|kind, key| {
                PROCESSOR_REGISTRY.requeue(kind, key);
            }),
        }
    }
    
    fn process_resource(&self, mut obj: K, ctx: &HandlerContext, is_init: bool) -> bool {
        // 1. 命名空间过滤
        if !self.filter_namespace(&obj, ctx) {
            return false;
        }
        
        // 2. Handler 过滤
        if !self.handler.filter(&obj) {
            return false;
        }
        
        // 3. 清理元数据
        self.handler.clean_metadata(&mut obj, ctx);
        
        // 4. 验证（仅警告）
        let warnings = self.handler.validate(&obj, ctx);
        for w in warnings {
            tracing::warn!(kind = self.kind, warning = %w, "Validation warning");
        }
        
        // 5. 解析（如 Secret 引用）
        let processed = match self.handler.parse(obj, ctx) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!(kind = self.kind, error = %e, "Parse failed");
                return false;
            }
        };
        
        // 6. 保存到缓存
        self.save(processed.clone());
        
        // 7. 调用 on_change
        if !is_init {
            self.handler.on_change(&processed, ctx);
        }
        
        true
    }
    
    fn process_delete(&self, obj: &K, ctx: &HandlerContext) {
        let key = make_resource_key(obj);
        
        // 调用 handler 的删除处理
        self.handler.on_delete(obj, ctx);
        
        // 从缓存删除
        self.remove(&key);
        
        tracing::info!(kind = self.kind, key = %key, "Resource deleted");
    }
    
    fn filter_namespace(&self, obj: &K, ctx: &HandlerContext) -> bool {
        if let Some(filter) = &ctx.namespace_filter {
            if let Some(ns) = obj.meta().namespace.as_ref() {
                return filter.contains(ns);
            }
        }
        true
    }
}

// 实现 ProcessorObj
impl<K> ProcessorObj for ResourceProcessor<K>
where
    K: Resource + Clone + Send + Sync + Debug + Serialize + for<'de> Deserialize<'de> + 'static,
{
    fn kind(&self) -> &'static str {
        self.kind
    }
    
    fn as_watch_obj(&self) -> Arc<dyn WatchObj> {
        self.cache.clone()
    }
    
    fn requeue(&self, key: String) {
        self.workqueue.enqueue(key);
    }
    
    fn requeue_after(&self, key: String, duration: Duration) {
        self.workqueue.enqueue_after(key, duration);
    }
    
    fn is_ready(&self) -> bool {
        self.cache.is_ready()
    }
    
    fn set_ready(&self) {
        self.cache.set_ready();
    }
    
    fn set_not_ready(&self) {
        self.cache.set_not_ready();
    }
    
    fn clear(&self) {
        self.cache.clear();
    }
}
```

##### Step 2.7: 实现 ProcessorHandler trait

```rust
// src/core/conf_mgr_new/sync_runtime/resource_processor/handler.rs

use kube::Resource;
use super::HandlerContext;

/// 处理结果
pub type ProcessResult<T> = Result<T, String>;

/// 资源处理逻辑（由各资源类型实现）
pub trait ProcessorHandler<K>: Send + Sync
where
    K: Resource + Clone + Send + Sync + 'static,
{
    /// 过滤资源
    fn filter(&self, _obj: &K) -> bool { true }
    
    /// 清理元数据
    fn clean_metadata(&self, _obj: &mut K, _ctx: &HandlerContext) {}
    
    /// 验证资源（返回警告列表）
    fn validate(&self, _obj: &K, _ctx: &HandlerContext) -> Vec<String> { vec![] }
    
    /// 解析资源（如 Secret 引用解析、注册到 RefManager）
    fn parse(&self, obj: K, _ctx: &HandlerContext) -> ProcessResult<K> { Ok(obj) }
    
    /// 删除时的清理（如取消 RefManager 注册）
    fn on_delete(&self, _obj: &K, _ctx: &HandlerContext) {}
    
    /// 变更后的处理（如 Secret 级联 requeue）
    fn on_change(&self, _obj: &K, _ctx: &HandlerContext) {}
}
```

##### Step 2.8: 实现 HandlerContext

```rust
// src/core/conf_mgr_new/sync_runtime/resource_processor/context.rs

use std::sync::Arc;
use super::secret_utils::SecretRefManager;

pub type RequeueFn = Box<dyn Fn(&str, String) + Send + Sync>;

/// Handler 上下文
pub struct HandlerContext {
    pub process_config: Option<Arc<ProcessConfig>>,
    pub namespace_filter: Option<Arc<Vec<String>>>,
    pub secret_ref_manager: Arc<SecretRefManager>,
    pub requeue_fn: RequeueFn,
}

impl HandlerContext {
    /// 跨资源 requeue
    pub fn requeue(&self, kind: &str, key: String) {
        (self.requeue_fn)(kind, key);
    }
    
    /// 获取 SecretRefManager
    pub fn secret_ref_manager(&self) -> &SecretRefManager {
        &self.secret_ref_manager
    }
}
```

##### Step 2.9 ~ 2.10: 复用 secret_utils 和 validation

从旧代码复制或引用。

##### Step 2.11: 实现各资源 Handler

```rust
// src/core/conf_mgr_new/sync_runtime/resource_processor/handlers/http_route.rs

use super::super::{ProcessorHandler, HandlerContext, ProcessResult};
use crate::types::prelude_resources::HTTPRoute;

pub struct HttpRouteHandler;

impl ProcessorHandler<HTTPRoute> for HttpRouteHandler {
    fn filter(&self, route: &HTTPRoute) -> bool {
        // 复用现有逻辑
        true
    }
    
    fn validate(&self, route: &HTTPRoute, ctx: &HandlerContext) -> Vec<String> {
        // 复用现有逻辑
        vec![]
    }
    
    fn parse(&self, route: HTTPRoute, ctx: &HandlerContext) -> ProcessResult<HTTPRoute> {
        // 复用现有逻辑
        Ok(route)
    }
}
```

##### Step 2.12: 添加 conf_center

最后添加 `conf_center`，创建简化的 Controller：

```rust
// src/core/conf_mgr_new/conf_center/kubernetes/resource_runner.rs

use std::sync::Arc;
use kube::{Api, Client};
use kube::runtime::{reflector, watcher};
use futures::StreamExt;

use crate::core::conf_mgr_new::{ResourceProcessor, ProcessorHandler, PROCESSOR_REGISTRY};
use crate::core::conf_sync::conf_server_new::ConfigSyncServer;

/// 运行单个资源的 Controller
pub async fn run_resource<K, H>(
    kind: &'static str,
    api: Api<K>,
    handler: Arc<H>,
    config_sync_server: Arc<ConfigSyncServer>,
    secret_ref_manager: Arc<SecretRefManager>,
    capacity: usize,
) -> Result<(), Error>
where
    K: Resource + Clone + Send + Sync + Debug + Serialize + DeserializeOwned + 'static,
    H: ProcessorHandler<K> + 'static,
{
    // 1. 创建 Processor
    let processor = Arc::new(ResourceProcessor::new(
        kind,
        capacity,
        handler,
        secret_ref_manager,
    ));
    
    // 2. 注册到全局 Registry
    PROCESSOR_REGISTRY.register(processor.clone());
    
    // 3. 注册 WatchObj 到 ConfigSyncServer
    config_sync_server.register_watch_obj(kind, processor.as_watch_obj());
    
    // 4. 创建 K8s reflector
    let (store, writer) = reflector::store();
    processor.set_store(Arc::new(store.clone()));
    
    // 5. 启动 Worker
    let worker_processor = processor.clone();
    let worker_handle = tokio::spawn(async move {
        loop {
            let item = worker_processor.workqueue().dequeue().await;
            worker_processor.process_work_item(&item.key);
            worker_processor.workqueue().done(&item.key);
        }
    });
    
    // 6. 处理 reflector stream
    let watcher_config = watcher::Config::default();
    let stream = Box::pin(reflector(writer, watcher(watcher_config, api)));
    
    while let Some(event) = stream.next().await {
        match event {
            Ok(Event::Init) => processor.on_init(),
            Ok(Event::InitApply(obj)) => { processor.on_init_apply(obj); }
            Ok(Event::InitDone) => processor.on_init_done(),
            Ok(Event::Apply(obj)) => processor.on_apply(&obj),
            Ok(Event::Delete(obj)) => processor.on_delete(&obj),
            Err(e) => {
                tracing::error!(kind = kind, error = %e, "Watcher error");
            }
        }
    }
    
    Ok(())
}
```

##### Step 2.13: 更新 core/mod.rs

```rust
// src/core/mod.rs

pub mod conf_sync;
pub mod conf_mgr;      // 旧代码（暂时保留）
pub mod conf_mgr_new;  // 新代码

// 切换时改这里
pub use conf_mgr_new::{ProcessorRegistry, PROCESSOR_REGISTRY};
```

---

### 5.4 Phase 3: 切换与清理

#### Step 3.1: 更新调用方

1. **Admin API**：改用 `PROCESSOR_REGISTRY` 获取资源
2. **CLI**：改用新的 Controller

#### Step 3.2: 集成测试

确保所有集成测试通过。

#### Step 3.3: 删除旧代码

```bash
# 删除旧代码
rm -rf src/core/conf_sync/conf_server
rm -rf src/core/conf_mgr

# 重命名新代码
mv src/core/conf_sync/conf_server_new src/core/conf_sync/conf_server
mv src/core/conf_mgr_new src/core/conf_mgr
```

#### Step 3.4: 更新 mod.rs 导出

```rust
// src/core/conf_sync/mod.rs
pub mod conf_server;  // 现在指向原 conf_server_new

// src/core/mod.rs
pub mod conf_mgr;     // 现在指向原 conf_mgr_new
```

---

### 5.5 步骤检查清单

#### Phase 1: conf_server_new

- [ ] Step 1.1: 创建 `conf_server_new/mod.rs`
- [ ] Step 1.2: 实现 `WatchObj` trait
- [ ] Step 1.3: 实现 `ConfigSyncServer`
- [ ] Step 1.4: 实现 `ConfigSyncGrpcServer`
- [ ] Step 1.5: `ServerCache<T>` 实现 `WatchObj`
- [ ] Step 1.6: 更新 `conf_sync/mod.rs` 导出
- [ ] 编译通过

#### Phase 2: conf_mgr_new

- [ ] Step 2.1: 创建 `conf_mgr_new/mod.rs`
- [ ] Step 2.2: 实现 `ProcessorRegistry`
- [ ] Step 2.3: 创建 `sync_runtime/mod.rs`
- [ ] Step 2.4: 复用 workqueue/shutdown/metrics
- [ ] Step 2.5: 创建 `resource_processor/mod.rs`
- [ ] Step 2.6: 实现 `ResourceProcessor<T>` 核心
- [ ] Step 2.7: 实现 `ProcessorHandler` trait
- [ ] Step 2.8: 实现 `HandlerContext`
- [ ] Step 2.9: 复用 secret_utils
- [ ] Step 2.10: 复用 validation
- [ ] Step 2.11: 实现各资源 Handler（逐个迁移）
  - [ ] HttpRouteHandler
  - [ ] GrpcRouteHandler
  - [ ] TcpRouteHandler
  - [ ] UdpRouteHandler
  - [ ] TlsRouteHandler
  - [ ] GatewayHandler
  - [ ] GatewayClassHandler
  - [ ] SecretHandler
  - [ ] EdgionTlsHandler
  - [ ] ServiceHandler
  - [ ] EndpointSliceHandler
  - [ ] ... (其他)
- [ ] Step 2.12: 添加 conf_center
- [ ] Step 2.13: 更新 core/mod.rs
- [ ] 编译通过

#### Phase 3: 切换与清理

- [ ] Step 3.1: 更新 Admin API
- [ ] Step 3.2: 更新 CLI
- [ ] Step 3.3: 集成测试通过
- [ ] Step 3.4: 删除旧代码
- [ ] Step 3.5: 重命名新代码
- [ ] Step 3.6: 最终测试

---

## 6. API 调用示例

### 6.1 Admin API 获取资源

```rust
// 之前
fn get_http_routes(cs: &ConfigServer) -> Vec<HTTPRoute> {
    cs.routes.list_all()
}

// 之后
fn get_http_routes() -> Vec<HTTPRoute> {
    // 方法 1: 通过 Registry typed 获取
    if let Some(processor) = PROCESSOR_REGISTRY.get::<HTTPRoute>("HTTPRoute") {
        processor.list()
    } else {
        vec![]
    }
}
```

### 6.2 Apply 资源变更

```rust
// 之前
fn apply_route(cs: &ConfigServer, route: HTTPRoute) {
    cs.apply_http_route_change(ResourceChange::EventUpdate, route);
}

// 之后
fn apply_route(route: HTTPRoute) {
    if let Some(processor) = PROCESSOR_REGISTRY.get::<HTTPRoute>("HTTPRoute") {
        processor.save(route);
    }
}
```

### 6.3 gRPC list/watch

```rust
// 不变，通过 ConfigSyncServer 访问
impl ConfigSync for ConfigSyncServer {
    async fn list(&self, request: Request<ListRequest>) -> Result<Response<ListResponse>, Status> {
        let kind = request.get_ref().kind.as_str();
        let (json, version) = self.list(kind)?;
        // ...
    }
}
```

---

## 7. 兼容性考虑

### 7.1 Admin API

需要更新所有通过 `ConfigServer` 直接访问 cache 的代码，改为通过 `ProcessorRegistry`。

### 7.2 集成测试

现有集成测试使用 `ConfigServer` 的地方需要更新。

### 7.3 回滚方案

在 Phase 2 完成前，保留原有 `conf_mgr`，可随时切回。

---

## 8. 风险与缓解

| 风险 | 缓解措施 |
|------|----------|
| 全局状态（`PROCESSOR_REGISTRY`）可能导致测试困难 | 提供 `with_registry` 方法用于测试注入 |
| Typed 获取需要 downcast | 使用 `TypeId` + `Any` 但限制在内部使用 |
| 迁移过程中功能中断 | 渐进式迁移，保持两套代码并行运行 |

---

## 9. 预期收益

1. **类型安全**：所有资源操作都是 typed，无需 JSON 序列化/反序列化
2. **职责清晰**：Processor 管理完整生命周期，`ConfigSyncServer` 只做 gRPC
3. **代码简化**：删除 `conf_change_apply.rs`、重复的 typed fields
4. **扩展性**：新增资源类型只需实现 `ProcessorHandler`
5. **可测试性**：Processor 可独立测试，不依赖完整的 ConfigServer

---

## 附录 A: 现有代码引用

### A.1 需要修改的文件

#### conf_server 相关
- `src/core/conf_sync/conf_server/mod.rs`
- `src/core/conf_sync/conf_server/config_server.rs`
- `src/core/conf_sync/conf_server/factory.rs`
- `src/core/conf_sync/conf_server/traits.rs`
- `src/core/conf_sync/conf_server/conf_change_apply.rs` (删除)
- `src/core/conf_sync/conf_server/grpc_server.rs`

#### conf_mgr 相关
- `src/core/conf_mgr/conf_center/kubernetes/resource_controller.rs`
- `src/core/conf_mgr/conf_center/sync_runtime/resource_processor/*.rs`

#### Admin API 相关
- `src/core/api/controller/namespaced_handlers.rs`
- `src/core/api/controller/cluster_handlers.rs`

### A.2 复用的代码

- `src/core/conf_sync/cache_server/cache.rs` (ServerCache<T>)
- `src/core/conf_mgr/conf_center/sync_runtime/workqueue.rs`
- `src/core/conf_mgr/conf_center/sync_runtime/shutdown.rs`
- `src/core/conf_mgr/conf_center/sync_runtime/resource_processor/secret_utils/`
