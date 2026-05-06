---
name: controller-resource-processor
description: ResourceProcessor 流水线：ProcessorHandler trait、HandlerContext、11 步处理流程、21 种 Handler 实现、引用管理器。
---

# ResourceProcessor 流水线

ResourceProcessor 是 Controller 核心处理引擎。每种资源类型拥有独立的 `ResourceProcessor<K>` 实例，内部持有 `ServerCache<K>`、`Workqueue` 和 `ProcessorHandler<K>` 实现，负责将原始资源从接收到校验、解析、状态更新、最终写入缓存的完整生命周期。

## ProcessorHandler\<K\> trait

每种资源类型实现该 trait 以定义自身处理逻辑。Handler 是无状态的——所有状态由 ResourceProcessor 管理。

```rust
#[async_trait]
pub trait ProcessorHandler<K>: Send + Sync
where
    K: Resource + Clone + Send + Sync + 'static,
{
    /// 资源过滤，返回 false 则跳过。默认：接受所有
    fn filter(&self, obj: &K) -> bool { true }

    /// 清理元数据（移除 managedFields、无关注解等）。默认：无操作
    fn clean_metadata(&self, obj: &mut K, ctx: &HandlerContext) {}

    /// 资源校验，返回警告列表（处理不中断）。默认：无校验
    fn validate(&self, obj: &K, ctx: &HandlerContext) -> Vec<String> { vec![] }

    /// 预解析：构建运行时结构、校验插件配置等。错误自动合并到 validate 错误列表
    fn preparse(&self, obj: &mut K, ctx: &HandlerContext) -> Vec<String> { vec![] }

    /// 解析/预处理：注册 Secret 引用、解析外部引用等。返回 Continue 或 Skip
    async fn parse(&self, obj: K, ctx: &HandlerContext) -> ProcessResult<K> { ProcessResult::Continue(obj) }

    /// 删除清理：清除 SecretRefManager 注册等
    async fn on_delete(&self, obj: &K, ctx: &HandlerContext) {}

    /// 变更后处理：保存到缓存后调用，用于级联 requeue
    async fn on_change(&self, obj: &K, ctx: &HandlerContext) {}

    /// init LIST 阶段完成后调用，用于 replace_all 等全量同步操作
    fn on_init_done(&self, ctx: &HandlerContext) {}

    /// 更新资源状态字段（Accepted、ResolvedRefs 等 Gateway API 条件）
    fn update_status(&self, obj: &mut K, ctx: &HandlerContext, validation_errors: &[String]) {}
}
```

`ProcessResult<K>` 枚举包含 `Continue(K)`（继续处理）和 `Skip { reason }`（丢弃该资源）两个变体。

## HandlerContext

Handler 方法的上下文对象，提供以下共享资源：

| 字段 | 类型 | 用途 |
|---|---|---|
| `secret_ref_manager` | `Arc<SecretRefManager>` | Secret 依赖追踪（哪些资源依赖哪些 Secret） |
| `metadata_filter` | `Option<Arc<MetadataFilterConfig>>` | 元数据清理规则配置 |
| `namespace_filter` | `Option<Arc<Vec<String>>>` | 命名空间白名单过滤 |
| `trigger_chain` | `TriggerChain` | 级联触发链，用于 requeue 环检测 |
| `max_trigger_cycles` | `usize` | 同一 (kind, key) 在触发链中允许的最大重复次数（默认 5） |

关键方法：

- `requeue(kind, key)` — 跨资源 requeue，内置环检测。检查 trigger_chain 中目标 (kind, key) 是否已超过 `max_trigger_cycles` 次，若超过则丢弃并记录错误。通过 `PROCESSOR_REGISTRY.requeue_with_chain()` 分发到目标 Processor 的 Workqueue。
- `clean_metadata(obj)` — 根据 `metadata_filter` 配置清理资源元数据。

## 11 步处理流程

`process_resource()` 方法实现完整的处理流水线：

```
输入: 原始资源 obj
  │
  ├─ 1. namespace filter    检查命名空间白名单，不在允许列表则 Skip
  ├─ 2. handler filter      调用 handler.filter()，返回 false 则 Skip
  ├─ 3. clean metadata      先执行 ctx.clean_metadata()（全局配置），再执行 handler.clean_metadata()（自定义清理）
  ├─ 4. validate            调用 handler.validate()，收集校验错误（记录警告但不中断）
  ├─ 5. preparse            调用 handler.preparse()，合并到校验错误列表
  ├─ 6. parse               调用 handler.parse()，返回 Continue(obj) 或 Skip
  ├─ 7. extract old status  提取旧状态用于后续比较
  │                          - K8s 模式：从对象本身提取
  │                          - FileSystem 模式：使用传入的 existing_status_json
  ├─ 8. update status       调用 handler.update_status()，设置 Gateway API 条件
  ├─ 9. check change        语义比较新旧状态，判断 status 是否真正变更
  ├─ 10. on_change          调用 handler.on_change()，执行级联操作（requeue 等）
  │                          init 阶段也会调用，requeue 项被 Workqueue 缓冲到 InitDone 后处理
  └─ 11. save to cache      init 阶段使用 InitAdd（同步），运行时使用 EventUpdate
                             保存后 ServerCache 通知 gRPC watchers

输出: WorkItemResult::Processed { obj, status_changed }
      或 WorkItemResult::Skipped
```

删除流程（`process_delete`）更简单：调用 `handler.on_delete()` 清理引用关系，然后从缓存移除。

## Handler 实现列表

共 21 种 Handler 实现，覆盖所有资源类型：

### Gateway API 路由（5 种）

| Handler | 资源 | 关键处理 |
|---|---|---|
| `HttpRouteHandler` | HTTPRoute | 主机名解析、Service 后端引用注册、跨命名空间校验、gateway_route_index 注册 |
| `GrpcRouteHandler` | GRPCRoute | 同 HTTPRoute，增加 gRPC 方法匹配 |
| `TcpRouteHandler` | TCPRoute | Service 后端引用注册、attached_route_tracker |
| `TlsRouteHandler` | TLSRoute | 同 TCPRoute，增加 gateway_route_index 注册 |
| `UdpRouteHandler` | UDPRoute | Service 后端引用注册、attached_route_tracker |

### Gateway API 核心（4 种）

| Handler | 资源 | 关键处理 |
|---|---|---|
| `GatewayHandler` | Gateway | 监听器端口冲突检测、hostname/port 缓存更新、route requeue |
| `GatewayClassHandler` | GatewayClass | 控制器名称校验、Accepted/SupportedVersion 条件 |
| `BackendTlsPolicyHandler` | BackendTLSPolicy | 后端 TLS 策略校验 |
| `ReferenceGrantHandler` | ReferenceGrant | 跨命名空间授权变更通知、CrossNsRevalidationListener |

### K8s 核心资源（4 种）

| Handler | 资源 | 关键处理 |
|---|---|---|
| `ServiceHandler` | Service | 变更时 requeue 依赖的路由 |
| `EndpointSliceHandler` | EndpointSlice | 直接存储，Gateway 按需消费 |
| `SecretHandler` | Secret | 变更时级联 requeue 依赖资源 |
| `ConfigmapHandler` | ConfigMap | 直接存储，无 requeue 依赖 |

### Edgion 自定义资源（6 种）

| Handler | 资源 | 关键处理 |
|---|---|---|
| `EdgionGatewayConfigHandler` | EdgionGatewayConfig | Gateway 关联配置 |
| `EdgionPluginsHandler` | EdgionPlugins | 插件配置校验、Secret 引用注册 |
| `EdgionStreamPluginsHandler` | EdgionStreamPlugins | 流式插件配置校验 |
| `EdgionTlsHandler` | EdgionTls | TLS 配置、Secret 引用注册、gateway_route_index |
| `EdgionAcmeHandler` | EdgionAcme | ACME 证书配置、Secret 引用 |
| `PluginMetadataHandler` | PluginMetadata | 插件元数据 |
| `LinkSysHandler` | LinkSys | 系统链接配置 |

### 共享工具模块

| 模块 | 用途 |
|---|---|
| `route_utils` | 路由类公共逻辑：Service 后端引用注册 (`register_service_backend_refs`)、parentRef 解析 |
| `hostname_resolution` | 主机名解析：Route 与 Gateway Listener 的 hostname 交集计算 |

## 引用管理器

所有引用管理器基于统一的 `BidirectionalRefManager<V>` 泛型实现，维护双向多对多索引：

- **正向索引**: `source_key → HashSet<V>` — 哪些资源引用了该源
- **反向索引**: `value_key → HashSet<source_key>` — 某个资源依赖哪些源

核心操作：`add_ref`、`remove_ref`、`clear_value_refs`（删除/更新时先清后加）、`get_refs`（查询依赖方）。

### 具体管理器

| 管理器 | 类型别名 | 用途 |
|---|---|---|
| `SecretRefManager` | `BidirectionalRefManager<ResourceRef>` | Secret 依赖追踪（Secret → Gateway, EdgionTls, EdgionPlugins, EdgionAcme） |
| `ServiceRefManager` | `BidirectionalRefManager<ResourceRef>` | Service 后端依赖（Service → 所有路由类型） |
| `CrossNamespaceRefManager` | `BidirectionalRefManager<ResourceRef>` | 跨命名空间引用（ReferenceGrant → 引用方资源） |
| `GatewayRouteIndex` | 自定义双向索引 | Gateway ↔ Route 映射，含 hostname/port 变更检测缓存 |
| `AttachedRouteTracker` | 自定义结构 | Route → 父 Gateway 的附着状态追踪（用于 Gateway status 中的 attachedRoutes） |

### ReferenceGrant 体系

- `ReferenceGrantStore` — 存储所有 ReferenceGrant 资源
- `CrossNamespaceValidator` — 校验跨命名空间引用是否被授权
- `CrossNsRevalidationListener` — 监听 ReferenceGrant 变更并 requeue 受影响资源

## Status 工具

### status_utils

提供 Gateway API 标准条件的构建工具：

- `accepted_condition(generation)` — 构建 Accepted=True 条件
- `resolved_refs_condition(generation)` — 构建 ResolvedRefs=True 条件
- `condition_true/condition_false` — 构建任意条件
- `update_condition(conditions, new_condition)` — 更新条件列表（合并同类型）
- `set_parent_conditions` / `set_parent_conditions_full` — 设置路由的 parentRef 状态

错误类型：`AcceptedError`、`ResolvedRefsError` 用于构建失败条件。

### status_diff

语义变更检测：比较新旧 status 时忽略 `lastTransitionTime` 等非语义字段，避免不必要的 K8s API 写入。通过 `status_semantically_changed(kind, old_json, new_json)` 判断是否需要持久化。

## RawStore — 跨资源按 key 直接查询

`ServerCache` 只存放经过 `handler.filter()` 的资源（如 `GatewayHandler` 按 `gatewayClassName` 过滤）。当 Route 需要查找不属于本 controller 的 Gateway 时（parentRef 可以指向任意 Gateway），`ServerCache` 查不到，导致 Route 状态计算错误。

`RawStore<K>` 绕过 handler filter，提供按 key 的 O(1) 单条查询：

```rust
pub trait RawStore<K>: Send + Sync {
    fn get(&self, key: &str) -> Option<Arc<K>>;  // key = "namespace/name" 或 "name"
}
```

### 两种后端实现

| 后端 | 类型 | 数据来源 | 注入时机 |
|---|---|---|---|
| `KubeRawStore<K>` | 包装 `reflector::Store<K>` | reflector 自动维护，与 K8s API 实时同步 | `run_with_api` 中 `reflector::store()` 之后立即注入 |
| `FsRawStore<K>` | `RwLock<HashMap<String, Arc<K>>>` | `FileSystemResourceController` 在 InitApply/Apply/Delete 及 Worker 处理时手动维护 | `FileSystemResourceController::new()` 时注入 |

### ProcessorObj 暴露接口

`ProcessorObj` trait（object-safe）新增：

```rust
fn get_raw_any(&self, key: &str) -> Option<Arc<dyn Any + Send + Sync>>;
```

实现：从 `raw_store` 取 `Arc<K>`，直接 coerce 为 `Arc<dyn Any + Send + Sync>` 返回，**零序列化开销**。  
调用方通过 `downcast::<ConcreteType>()` 还原具体类型：

```rust
let arc: Arc<Gateway> = processor.get_raw_any(&key)?.downcast::<Gateway>().ok()?;
```

`raw_store` 未注入时（init 阶段之前）返回 `None`，行为与之前一致，不退化。

### 调用方

| 函数 | 位置 | 查询目标 |
|---|---|---|
| `lookup_gateway` | `route_utils.rs` | `PROCESSOR_REGISTRY.get("Gateway")?.get_raw_any("ns/name")?.downcast::<Gateway>()` |
| `lookup_service_cluster_ip` | `gateway.rs` | `PROCESSOR_REGISTRY.get("Service")?.get_raw_any("ns/name")?.downcast::<Service>()` |

### 关键文件

- `src/core/controller/conf_mgr/sync_runtime/resource_processor/raw_store.rs` — `RawStore<K>` trait、`KubeRawStore<K>`、`FsRawStore<K>`
