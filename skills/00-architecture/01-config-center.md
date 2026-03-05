# 配置中心

> Controller 核心架构：ConfCenter 多配置源抽象、Workqueue 处理队列、ResourceProcessor 资源处理、
> 跨资源 Requeue 机制、BidirectionalRefManager 引用追踪、Secret 管理、Admin API。

## ConfCenter — Multi Config Center Support

The controller abstracts its config source behind the `ConfCenter` trait:

```
ConfMgr (facade, in manager.rs)
└── Arc<dyn ConfCenter>
    ├── FileSystemCenter   — watches local YAML directory, file events
    └── KubernetesCenter   — K8s API watchers, leader election
```

**Traits:**
- `CenterApi` — CRUD: `set_one`, `create_one`, `update_one`, `delete_one`, `get_one`, `list_all`
- `CenterLifeCycle` — `start`, `is_ready`, `config_sync_server`, `request_reload`
- `ConfCenter = CenterApi + CenterLifeCycle`

**Key files:**
- `src/core/conf_mgr/conf_center/traits.rs` — trait definitions
- `src/core/conf_mgr/conf_center/file_system/center.rs` — `FileSystemCenter`
- `src/core/conf_mgr/conf_center/kubernetes/center.rs` — `KubernetesCenter`
- `src/core/conf_mgr/manager.rs` — `ConfMgr` facade

## Workqueue — Per-Resource Processing

Each resource kind gets its own `Workqueue` + `ResourceProcessor`:

```
Event (file change / K8s watch)
  → ResourceController.on_apply(key) / on_delete(key)
    → Workqueue.enqueue(key)        # Deduplicated by pending set
      → Worker loop:
        item = dequeue()            # Key released from pending (allows dirty requeue)
        obj = store.get(key)
        handler.validate(obj)       # Schema + semantic validation
        handler.preparse(obj)       # Build runtime structures
        handler.parse(obj)          # Update caches, resolve refs
        handler.on_change(obj)      # Notify dependents
        handler.update_status(obj)  # Write status back
```

**Requeue with backoff:** `initial_backoff * 2^retry_count`, capped by `max_backoff`. Items dropped after `max_retries`.

**Dirty requeue:** Key is removed from `pending` on dequeue, so new events for the same key can be enqueued while processing. This ensures no events are lost.

**Key files:**
- `src/core/conf_mgr/sync_runtime/workqueue.rs` — `Workqueue`, `WorkItem`, `WorkqueueConfig`
- `src/core/conf_mgr/sync_runtime/resource_processor/processor.rs` — `ResourceProcessor<K>`
- `src/core/conf_mgr/sync_runtime/resource_processor/handler.rs` — `ProcessorHandler` trait
- `src/core/conf_mgr/sync_runtime/resource_processor/handlers/` — per-kind handlers
- `src/core/conf_mgr/processor_registry.rs` — `PROCESSOR_REGISTRY` (global, for cross-kind requeue)

## Cross-Resource Requeue

When one resource changes, dependent resources are requeued automatically.

**Secret → dependent resources:**

```
SecretHandler.on_change()
  → SecretRefManager.get_refs(secret_key)     # Returns Set<ResourceRef>
    → for each ref: PROCESSOR_REGISTRY.requeue(kind, key)
      → target kind's workqueue.enqueue(key)
```

**Service → dependent routes:**

```
ServiceHandler.on_change()
  → ServiceRefManager.get_refs(service_key)   # Returns Set<ResourceRef>
    → for each ref: PROCESSOR_REGISTRY.requeue(kind, key)
```

**ReferenceGrant → cross-namespace resources:**

```
ReferenceGrant change
  → CrossNsRevalidationListener
    → requeue all resources with cross-namespace refs
      (HTTPRoute, GRPCRoute, TCPRoute, TLSRoute, UDPRoute, Gateway)
```

## BidirectionalRefManager — Generic Reference Tracking

All three ref managers above (`SecretRefManager`, `ServiceRefManager`, `CrossNamespaceRefManager`) share the same pattern and are implemented as type aliases over a single generic:

```rust
BidirectionalRefManager<V: RefValue>
├── Forward:  source_key → HashSet<V>    // which values reference this source
├── Reverse:  value_key  → HashSet<source_key>  // which sources a value depends on
└── Methods:  add_ref, get_refs, get_dependencies, clear_value_refs, all_source_keys, stats, clear
```

`ResourceRef { kind, namespace, name }` implements `RefValue` and is the common value type. Concrete managers:

| Type Alias | Source Key | Usage |
|------------|-----------|-------|
| `SecretRefManager` | Secret key (`"ns/name"`) | Secret → dependent resources |
| `ServiceRefManager` | Service key (`"ns/name"`) | Service → dependent routes |
| `CrossNamespaceRefManager` | Target namespace (`"ns"`) | ReferenceGrant → cross-ns resources |

Handlers register refs during `parse()` and clear them in `on_delete()`. The source handler's `on_change()` queries `get_refs()` to find and requeue dependents.

**Key files:**
- `src/core/conf_mgr/sync_runtime/resource_processor/ref_manager.rs` — `BidirectionalRefManager<V>`, `RefValue`, `ResourceRef`, `RefManagerStats`
- `src/core/conf_mgr/sync_runtime/resource_processor/secret_utils/secret_ref.rs` — `SecretRefManager` (type alias)
- `src/core/conf_mgr/sync_runtime/resource_processor/service_ref.rs` — `ServiceRefManager` (type alias)
- `src/core/conf_mgr/sync_runtime/resource_processor/ref_grant/cross_ns_ref_manager.rs` — `CrossNamespaceRefManager` (type alias)
- `src/core/conf_mgr/sync_runtime/resource_processor/secret_utils/secret_store.rs` — `GLOBAL_SECRET_STORE`

## Architecture Constraint: No Circular Triggers

**Rule: Cross-resource requeue must form a DAG (Directed Acyclic Graph). Circular trigger chains are forbidden.**

Current trigger flow (unidirectional only):

```
Secret ──────► {EdgionTls, Gateway, EdgionPlugins, BackendTLSPolicy, ...}
Service ─────► {HTTPRoute, GRPCRoute, TCPRoute, TLSRoute, UDPRoute}
ReferenceGrant ► {HTTPRoute, GRPCRoute, TCPRoute, TLSRoute, UDPRoute, Gateway}
Route (any) ─► Gateway   (requeue parent gateways for status update)
```

**Critical constraint:** Gateway MUST NOT trigger Route requeue. If it did, `Route → Gateway → Route` would form an infinite loop.

When adding new cross-resource triggers:
1. Draw the trigger edge on the DAG above
2. Verify no cycle is introduced
3. If the new edge would create a cycle, redesign the dependency (e.g., use
   a separate status-only path that doesn't trigger full reprocessing)

## Secret — Built-in Mechanism

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

Plugins access secrets at runtime: `get_secret(Some(namespace), &secret_ref.name)`.

Controller-side handlers resolve secrets during parse phase and populate `resolved_*` fields in configs (e.g., `resolved_client_secret` in plugin configs).

## Controller Admin API

HTTP on `:5800` via Axum:

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/health` | Liveness |
| GET | `/ready` | Readiness (ConfigServer ready) |
| GET | `/api/v1/server-info` | Server ID, endpoint mode, supported kinds |
| POST | `/api/v1/reload` | Reload all resources from storage |
| GET/POST/PUT/DELETE | `/api/v1/namespaced/{kind}/{namespace}[/{name}]` | Namespaced resource CRUD |
| GET/POST/PUT/DELETE | `/api/v1/cluster/{kind}[/{name}]` | Cluster-scoped resource CRUD |
| GET | `/configserver/{kind}/list` | List from ConfigServer cache |
