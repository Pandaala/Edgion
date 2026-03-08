# 配置中心通用架构

> ConfCenter trait 抽象、Workqueue 处理队列、ResourceProcessor 资源处理、
> 跨资源 Requeue 机制、BidirectionalRefManager 引用追踪、Secret 管理、Admin API。

## ConfCenter Trait 体系

Controller 将配置源抽象为 trait，允许不同后端（文件系统 / Kubernetes）提供统一接口：

```
ConfMgr (facade, in manager.rs)
└── Arc<dyn ConfCenter>
    ├── FileSystemCenter   — watches local YAML directory, file events
    └── KubernetesCenter   — K8s API watchers, leader election
```

**Traits：**

| Trait | 职责 | 关键方法 |
|-------|------|---------|
| `CenterApi` | CRUD 操作 | `set_one`, `create_one`, `update_one`, `delete_one`, `get_one`, `list_all` |
| `CenterLifeCycle` | 生命周期管理 | `start`, `is_ready`, `config_sync_server`, `request_reload` |
| `ConfCenter` | 完整接口 | `= CenterApi + CenterLifeCycle`（blanket impl） |

**关键文件：**
- `src/core/controller/conf_mgr/conf_center/traits.rs` — trait 定义
- `src/core/controller/conf_mgr/conf_center/file_system/center.rs` — `FileSystemCenter`
- `src/core/controller/conf_mgr/conf_center/kubernetes/center.rs` — `KubernetesCenter`
- `src/core/controller/conf_mgr/manager.rs` — `ConfMgr` facade

## Workqueue — 逐资源处理队列

每种资源类型拥有独立的 `Workqueue` + `ResourceProcessor`：

```
Event (file change / K8s watch)
  → ResourceController.on_apply(key) / on_delete(key)
    → Workqueue.enqueue(key)        # 按 key 去重（pending set）
      → Worker loop:
        item = dequeue()            # Key 从 pending 释放（支持 dirty requeue）
        obj = store.get(key)
        handler.validate(obj)       # Schema + 语义校验
        handler.preparse(obj)       # 构建运行时结构
        handler.parse(obj)          # 更新缓存、解析引用
        handler.on_change(obj)      # 通知依赖资源
        handler.update_status(obj)  # 回写 status
```

### Requeue with Backoff

失败的 work item 按指数退避重试：`initial_backoff * 2^retry_count`，
上限为 `max_backoff`。超过 `max_retries` 后放弃。

### Dirty Requeue

Key 在 dequeue 时从 `pending` set 中移除，因此同一 key 在处理期间可以被重新 enqueue。
这确保了不会丢失任何事件——即使在处理 key A 时 A 再次变更，新事件也会被捕获。

### 关键文件

- `src/core/controller/conf_mgr/sync_runtime/workqueue.rs` — `Workqueue`, `WorkItem`, `WorkqueueConfig`
- `src/core/controller/conf_mgr/sync_runtime/resource_processor/processor.rs` — `ResourceProcessor<K>`
- `src/core/controller/conf_mgr/sync_runtime/resource_processor/handler.rs` — `ProcessorHandler` trait
- `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/` — per-kind handlers
- `src/core/controller/conf_mgr/processor_registry.rs` — `PROCESSOR_REGISTRY`（全局注册表）

## Cross-Resource Requeue 机制

当一个资源变更时，依赖它的资源会被自动 requeue：

**Secret → 依赖资源：**
```
SecretHandler.on_change()
  → SecretRefManager.get_refs(secret_key)     # Returns Set<ResourceRef>
    → for each ref: PROCESSOR_REGISTRY.requeue(kind, key)
      → target kind's workqueue.enqueue(key)
```

**Service → 依赖路由：**
```
ServiceHandler.on_change()
  → ServiceRefManager.get_refs(service_key)
    → for each ref: PROCESSOR_REGISTRY.requeue(kind, key)
```

**ReferenceGrant → 跨命名空间资源：**
```
ReferenceGrant change
  → CrossNsRevalidationListener
    → requeue all resources with cross-namespace refs
      (HTTPRoute, GRPCRoute, TCPRoute, TLSRoute, UDPRoute, Gateway)
```

## BidirectionalRefManager — 通用引用追踪

三种引用管理器共享同一个泛型实现：

```rust
BidirectionalRefManager<V: RefValue>
├── Forward:  source_key → HashSet<V>    // 哪些值引用了这个 source
├── Reverse:  value_key  → HashSet<source_key>  // 一个值依赖哪些 source
└── Methods:  add_ref, get_refs, get_dependencies, clear_value_refs, all_source_keys, stats, clear
```

| 类型别名 | Source Key | 用途 |
|----------|-----------|------|
| `SecretRefManager` | Secret key (`"ns/name"`) | Secret → 依赖资源 |
| `ServiceRefManager` | Service key (`"ns/name"`) | Service → 依赖路由 |
| `CrossNamespaceRefManager` | Target namespace (`"ns"`) | ReferenceGrant → 跨命名空间资源 |

Handler 在 `parse()` 时注册引用，在 `on_delete()` 时清理。
Source handler 的 `on_change()` 查询 `get_refs()` 找到并 requeue 依赖者。

**关键文件：**
- `src/core/controller/conf_mgr/sync_runtime/resource_processor/ref_manager.rs`
- `src/core/controller/conf_mgr/sync_runtime/resource_processor/secret_utils/secret_ref.rs`
- `src/core/controller/conf_mgr/sync_runtime/resource_processor/service_ref.rs`
- `src/core/controller/conf_mgr/sync_runtime/resource_processor/ref_grant/cross_ns_ref_manager.rs`

## 架构约束：禁止循环触发

**规则：跨资源 requeue 必须形成 DAG（有向无环图），禁止循环触发链。**

当前触发流：

```
Secret ──────► {EdgionTls, Gateway, EdgionPlugins, BackendTLSPolicy, ...}
Service ─────► {HTTPRoute, GRPCRoute, TCPRoute, TLSRoute, UDPRoute}
ReferenceGrant ► {HTTPRoute, GRPCRoute, TCPRoute, TLSRoute, UDPRoute, Gateway}
Route (any) ─► Gateway   (requeue parent gateways for status update)
```

**关键约束：** Gateway **不可以**触发 Route requeue。否则 `Route → Gateway → Route` 会形成无限循环。

## Secret — 内置机制

```
GLOBAL_SECRET_STORE (LazyLock<SecretStore>)
├── Map: "namespace/name" → Secret
├── get_secret(namespace, name) → Option<Secret>
├── update_secrets(upsert, remove)
└── replace_all_secrets()

SecretHandler
├── parse: updates SecretStore
├── on_change: triggers cascading requeue for dependents
└── on_delete: removes from store + triggers requeue
```

## Controller Admin API

HTTP 端口 `:5800`（Axum）：

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/health` | Liveness |
| GET | `/ready` | Readiness（ConfigServer ready） |
| GET | `/api/v1/server-info` | Server ID、endpoint mode、supported kinds |
| POST | `/api/v1/reload` | 重新加载所有资源 |
| GET/POST/PUT/DELETE | `/api/v1/namespaced/{kind}/{namespace}[/{name}]` | Namespaced resource CRUD |
| GET/POST/PUT/DELETE | `/api/v1/cluster/{kind}[/{name}]` | Cluster-scoped resource CRUD |
| GET | `/configserver/{kind}/list` | List from ConfigServer cache |
