# Revalidation & Requeue 全景图

> 梳理 Controller 中所有 requeue 路径：post-init 一次性重校验 + 运行时级联 requeue。
> 源码基准：`src/core/controller/conf_mgr/sync_runtime/resource_processor/`

---

## 一、Post-Init 重校验（CachesReady 阶段）

所有 Processor 完成 `on_init_done()` 后，在 `pending_sync_kinds()` 清空时执行。
调用位置（两处，逻辑相同）：
- `conf_center/kubernetes/center.rs` — 主初始化 + HA all-serve 重载
- `conf_center/file_system/center.rs` — 文件系统后端初始化

### 执行顺序

| # | 函数 | 源码位置 | Requeue 目标 | 解决什么问题 |
|---|------|---------|-------------|-------------|
| 1 | `trigger_full_cross_ns_revalidation()` | `ref_grant/revalidation_listener.rs:14` | CrossNsRefManager 中所有已注册资源（去重） | Route 在 ReferenceGrant 到达前处理，跨 ns backendRef 被错误拒绝 |
| 2 | `trigger_gateway_secret_revalidation()` | `ref_grant/revalidation_listener.rs:89` | **所有 Gateway**（通过 `PROCESSOR_REGISTRY.get("Gateway").list_keys()`） | Gateway 在 Secret 到达前处理，TLS 证书未解析，ResolvedRefs=False |
| 3 | `trigger_gateway_route_revalidation()` | `ref_grant/revalidation_listener.rs:66` | GatewayRouteIndex 中所有已注册 Route（去重） | Route 的 on_change 在 Gateway 的 on_change 之前跑，Gateway 的 hostname/port 缓存还没建立 |

### 数据流

```
所有 Processor on_init_done()
    │
    ▼  pending_sync_kinds() == empty
    │
    ├─ 1. trigger_full_cross_ns_revalidation()
    │     CrossNsRefManager.all_source_keys() → 每个 ns 的 get_refs() → requeue 资源
    │
    ├─ 2. trigger_gateway_secret_revalidation()
    │     PROCESSOR_REGISTRY.get("Gateway").list_keys() → requeue 每个 Gateway
    │
    └─ 3. trigger_gateway_route_revalidation()
          GatewayRouteIndex.all_routes() → requeue 每条 Route
```

### 覆盖范围分析

| 资源类型 | 是否被 post-init 覆盖 | 通过哪条路径 |
|---------|---------------------|-------------|
| Gateway | ✅ | #2 直接 requeue 所有 |
| HTTPRoute / GRPCRoute | ✅ | #3 通过 GatewayRouteIndex；#1 如有跨 ns 引用 |
| TLSRoute | ✅ | #3 通过 GatewayRouteIndex |
| TCPRoute / UDPRoute | ❌ | 不在 GatewayRouteIndex 中（无 hostname 解析需求），也不在 #1 中（除非有跨 ns backendRef） |
| EdgionTls | ✅ | #3 通过 GatewayRouteIndex |
| BackendTLSPolicy | ❌ | 不在任何 post-init requeue 路径中 |
| EdgionPlugins | ❌ | 无（通过 SecretRefManager 运行时级联覆盖） |
| Secret / ConfigMap | ❌ | 无需（它们是数据源，不依赖其他资源） |
| Service / EndpointSlice | ❌ | 无需（Service 是被引用方；EndpointSlice 直接存储） |
| ReferenceGrant | ❌ | 无需（它触发别人的 requeue，自身不依赖外部资源） |

### 潜在缺口

1. **BackendTLSPolicy**：如果 BTP 先于 Route 处理，其 ancestor 解析依赖的 ServiceRefManager + GatewayRouteIndex 还未填充
2. **TCPRoute / UDPRoute**：如果有跨 ns backendRef 且 ReferenceGrant 后到达，仅 #1 覆盖；如果无跨 ns 引用，init 乱序可能导致 status 暂时不准（但这两种 Route 不用 GatewayRouteIndex，不受 Gateway hostname 影响）

---

## 二、运行时级联 Requeue

资源变更时通过 on_change / on_delete 触发的 requeue。

### 2.1 SecretRefManager 级联

**触发方**：SecretHandler / ConfigMapHandler 的 `on_change` / `on_delete`
**机制**：`trigger_cascading_requeue(secret_key, event, ctx)`
**查询**：`ctx.secret_ref_manager().get_refs(secret_key)` → 返回所有注册了该 Secret/ConfigMap 的 `ResourceRef`
**Requeue 目标**：逐个 `ctx.requeue(ref.kind_str(), ref.resource_key())`

| 触发资源 | 注册方（parse 时调用 add_ref） | 级联 requeue 目标 |
|---------|---------------------------|------------------|
| Secret 变更 | Gateway, EdgionTls, EdgionPlugins, BackendTLSPolicy, EdgionAcme | 所有引用该 Secret 的资源 |
| ConfigMap 变更 | Gateway（frontend CA） | 所有引用该 ConfigMap 的 Gateway |

**源码**：
- `handlers/secret.rs:38` — `trigger_cascading_requeue`
- `handlers/configmap.rs:22` — `trigger_cascading_requeue`

### 2.2 ServiceRefManager 级联

**触发方**：ServiceHandler 的 `on_change` / `on_delete`
**机制**：`requeue_dependent_routes(svc, event, ctx)`
**查询**：`get_service_ref_manager().get_refs(service_key)` → 返回所有引用该 Service 的 `ResourceRef`
**Requeue 目标**：逐个 `ctx.requeue(ref.kind_str(), ref.resource_key())`

| 触发资源 | 注册方（parse 时调用 register_service_backend_refs） | 级联 requeue 目标 |
|---------|---------------------------------------------------|------------------|
| Service 变更 | HTTPRoute, GRPCRoute, TLSRoute, TCPRoute, UDPRoute | 所有 backendRef 引用该 Service 的 Route |

**注意**：BackendTLSPolicy 当前 **未注册** 在 ServiceRefManager 中，所以 Service 变更不会 requeue BTP。

**源码**：
- `handlers/service.rs:22` — `requeue_dependent_routes`
- `handlers/route_utils.rs:395` — `register_service_backend_refs`

### 2.3 GatewayRouteIndex 级联

**触发方**：GatewayHandler 的 `on_change`
**条件**：仅当 Gateway 的 listener hostname 或 port **实际变更**时触发（通过 `update_gateway_hostnames()` / `update_gateway_ports()` 的返回值判断）
**查询**：`get_gateway_route_index().get_routes_for_gateway(gw_key)` → 返回关联的 Route
**Requeue 目标**：每条 Route

**反向**：Route 的 `on_change` → `update_gateway_route_index()` 更新正向/反向索引

**源码**：
- `handlers/gateway.rs` — on_change 中 hostname/port 变更检测 + requeue
- `handlers/mod.rs:89` — `update_gateway_route_index`
- `gateway_route_index.rs` — 正向/反向索引

### 2.4 CrossNsRefManager 级联

**触发方**：ReferenceGrantHandler 的 `on_change` / `on_delete`
**机制**：`CrossNsRevalidationListener::on_reference_grant_changed(event)`
**查询**：`get_global_cross_ns_ref_manager().get_refs(namespace)` → 返回受影响的 ResourceRef
**Requeue 目标**：所有在受影响 namespace 有跨 ns 引用的资源

| 注册方（parse 时注册跨 ns 引用） |
|-------------------------------|
| Gateway（跨 ns Secret/CA）、各类 Route（跨 ns backendRef） |

**源码**：
- `ref_grant/revalidation_listener.rs:114` — `CrossNsRevalidationListener`
- `ref_grant/cross_ns_ref_manager.rs` — 命名空间级引用管理

### 2.5 AttachedRouteTracker → Gateway

**触发方**：Route 的 `on_change`
**条件**：Route 的 Gateway attachment 列表**实际变化**时（通过 `update_route()` 返回值判断）
**Requeue 目标**：Route 的 parentRef 指向的 Gateway
**用途**：更新 Gateway status 中的 `attachedRoutes` 计数

**源码**：
- `attached_route_tracker.rs` — 正向/反向追踪
- `handlers/mod.rs:60` — `requeue_parent_gateways`

### 2.6 Route → BackendTLSPolicy

**触发方**：所有 Route Handler（HTTP/GRPC/TLS/TCP/UDP）的 `on_change` / `on_delete`
**机制**：`requeue_backend_tls_policies_for_route(route_kind, route_ns, route_name, ctx)`
**查询**：`ServiceRefManager.get_dependencies(route_ref_key)` → 反向查 Route 依赖的 Service keys → `BackendTLSPolicyTargetManager.get_refs(service_key)` → 找到绑定的 BTP
**Requeue 目标**：关联的 BackendTLSPolicy
**用途**：BTP 需要重新解析 ancestor Gateways（Route 的 parentRef 或 backendRef 可能变更）

**关键时序**：on_delete 中必须在 `clear_service_backend_refs` **之前**调用，否则 ServiceRefManager 反向索引已清除。

**源码**：
- `handlers/mod.rs:113` — `requeue_backend_tls_policies_for_route`

---

## 三、环安全

### TriggerChain

每次 `ctx.requeue(kind, key)` 检查：
1. 链深度 < `max_trigger_depth`（安全上限）
2. 目标 `(kind, key)` 在链中出现次数 < `max_trigger_cycles`（默认 5）

### 已知环路

唯一的双向 requeue：**Route ↔ Gateway**
- Gateway on_change → requeue Routes（仅 hostname/port 实际变更时）
- Route on_change → requeue Gateway（仅 attachment 实际变化时）

这两个"仅当实际变更"条件确保绝大多数情况下不形成循环。TriggerChain 兜底。

---

## 四、索引与管理器总览

| 组件 | 类型 | 正向索引 | 反向索引 | 公开查询 API |
|------|------|---------|---------|-------------|
| **SecretRefManager** | `BidirectionalRefManager<ResourceRef>` | `secret_key → Set<ResourceRef>` | `resource_ref_key → Set<secret_key>` | `get_refs(source)`, `get_dependencies(value)`, `clear_resource_refs(value)` |
| **ServiceRefManager** | `BidirectionalRefManager<ResourceRef>` | `service_key → Set<ResourceRef>` | `resource_ref_key → Set<service_key>` | 同上 |
| **CrossNsRefManager** | `BidirectionalRefManager<ResourceRef>` | `namespace → Set<ResourceRef>` | `resource_ref_key → Set<namespace>` | 同上 |
| **GatewayRouteIndex** | 自定义双向索引 | `gateway_key → Set<RouteEntry>` | `RouteEntry → Set<gateway_key>` | `get_routes_for_gateway(gw_key)`, `get_gateways_for_route(kind, key)`, `all_routes()` |
| **AttachedRouteTracker** | 自定义双向索引 | `gateway_key → ListenerMap` | `RouteRef → Set<Attachment>` | `count_for_listener(gw_key, listener)`, `update_route()`, `remove_route()` |

### ~~GatewayRouteIndex 缺口~~ ✅ 已修复

反向索引已通过 `get_gateways_for_route(kind, route_key)` 公开。用于 BackendTLSPolicy 的 ancestor 解析链路。

---

## 五、完整 Requeue 触发矩阵

| 当…变更时 | 谁被 Requeue | 通过什么机制 | 条件 |
|----------|-------------|-------------|------|
| Secret | Gateway, EdgionTls, EdgionPlugins, BTP, EdgionAcme | SecretRefManager 级联 | 注册了该 Secret 的资源 |
| ConfigMap | Gateway | SecretRefManager 级联（共用） | 注册了该 ConfigMap 的 Gateway |
| Service | HTTPRoute, GRPCRoute, TLSRoute, TCPRoute, UDPRoute | ServiceRefManager 级联 | 注册了该 Service 的 Route |
| Gateway（hostname/port 变更） | 关联 Route | GatewayRouteIndex 正向查询 | hostname 或 port 实际变更 |
| Route（attachment 变更） | 父 Gateway | AttachedRouteTracker | attachment 列表实际变化 |
| ReferenceGrant | 跨 ns 引用方 | CrossNsRefManager | 受影响 namespace |
| **init 完成** | 跨 ns 资源 + 所有 Gateway + 索引中所有 Route | 3 个 post-init 函数 | 一次性 |
