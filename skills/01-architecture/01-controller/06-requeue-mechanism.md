---
name: controller-requeue
description: 跨资源 Requeue 机制：6 条触发路径、环检测、post-init 重校验、handler on_change/on_delete 清单。
---

# 跨资源 Requeue 机制

## 概述

Controller 使用基于 requeue 的调和模式处理资源间依赖关系。当资源 A 发生变更时，依赖它的资源 B 被重新入队（re-process），以获取最新状态。这种松耦合设计支持任意资源到达顺序——无论是初始化阶段还是运行时都能正确工作。

核心组件：

- **Workqueue**（每种资源类型一个）— 就绪队列 + 延迟堆，支持去重和指数退避
- **PROCESSOR_REGISTRY**（全局单例）— 注册所有 Processor，提供 `requeue(kind, key)` 和 `requeue_with_chain(kind, key, chain)` 跨资源分发
- **TriggerChain** — 因果追踪链，记录 requeue 级联路径，防止无限循环

## 6 条触发路径

### 1. GatewayRouteIndex

**用途**: 解析 Route ↔ Gateway 依赖关系（主机名、端口、sectionName）

| 方向 | 触发条件 | 目标资源 | 机制 |
|---|---|---|---|
| Gateway → Routes | Listener hostname 或 port 发生变更 | HTTPRoute, GRPCRoute, TLSRoute, EdgionTls | `gateway_route_index.update_gateway_hostnames/ports()` 返回 true 时 requeue 索引中的所有关联路由 |
| Route → Gateway | parentRef 附着列表发生变更 | Gateway | 仅当 attachment 列表实际变化时才 requeue（通过 AttachedRouteTracker 判断） |
| Post-init | `trigger_gateway_route_revalidation()` | 索引中所有已注册路由 | 一次性执行，覆盖 Gateway 先于 Route 到达的情况 |

**注册规则**: 任何在 `parse()` 中调用 `lookup_gateway()` 的 Handler 必须在 `on_change()` 中调用 `update_gateway_route_index()` 并在 `on_delete()` 中调用 `remove_from_gateway_route_index()`。

**关键文件**:
- `gateway_route_index.rs` — 正向/反向索引 + hostname/port 变更检测缓存
- `handlers/gateway.rs` — on_change: 更新缓存并 requeue routes
- `handlers/{http_route,grpc_route,tls_route,edgion_tls}.rs` — on_change: 注册到索引

### 2. SecretRefManager

**用途**: 解析 Secret 依赖关系（TLS 证书、认证凭据）

| 方向 | 触发条件 | 目标资源 | 机制 |
|---|---|---|---|
| Secret → 依赖方 | Secret 创建/更新/删除 | Gateway, EdgionTls, EdgionPlugins, EdgionAcme | `trigger_cascading_requeue()` — 查询正向索引获取所有引用该 Secret 的资源并逐个 requeue |
| Post-init | `trigger_gateway_secret_revalidation()` | 所有 Gateway | 一次性执行，覆盖 Secret 晚于 Gateway 到达的情况 |

**关键文件**:
- `ref_manager.rs` — 通用 `BidirectionalRefManager<ResourceRef>`
- `handlers/secret.rs` — on_change/on_delete: 级联 requeue
- `secret_utils/secret_ref.rs` — SecretRefManager 类型别名

### 3. ServiceRefManager

**用途**: 解析 Service 后端依赖关系，覆盖所有路由类型

| 方向 | 触发条件 | 目标资源 | 机制 |
|---|---|---|---|
| Service → Routes | Service 创建/更新/删除 | HTTPRoute, GRPCRoute, TLSRoute, TCPRoute, UDPRoute | `requeue_dependent_routes()` — 查询正向索引获取所有引用该 Service 的路由并 requeue |

**关键文件**:
- `service_ref.rs` — ServiceRefManager（`BidirectionalRefManager<ResourceRef>` 别名）
- `handlers/service.rs` — on_change/on_delete: requeue 依赖路由
- `route_utils.rs` — `register_service_backend_refs()` 被所有路由 Handler 的 parse() 调用

### 4. CrossNamespaceRefManager

**用途**: 当 ReferenceGrant 变更时，重新校验所有受影响的跨命名空间引用

| 方向 | 触发条件 | 目标资源 | 机制 |
|---|---|---|---|
| ReferenceGrant → 引用方 | ReferenceGrant 创建/更新/删除 | 拥有跨命名空间引用的路由和资源 | `CrossNsRevalidationListener` 监听变更事件，requeue 受影响资源 |
| Post-init | `trigger_full_cross_ns_revalidation()` | 所有注册了跨命名空间引用的资源 | 一次性执行，覆盖 ReferenceGrant 到达顺序问题 |

**关键文件**:
- `ref_grant/cross_ns_ref_manager.rs` — 命名空间级引用管理
- `ref_grant/revalidation_listener.rs` — 监听器 + post-init 重校验函数

### 5. ListenerPortManager

**用途**: 解析 Gateway 间的端口冲突（多个 Gateway 监听同一端口）

| 方向 | 触发条件 | 目标资源 | 条件 |
|---|---|---|---|
| Gateway → Gateway | 端口冲突检测 | 其他占用同端口的 Gateway | `get_conflicting_gateways()` 返回冲突方 |
| Gateway 删除时 | 端口释放 | 之前冲突的 Gateway | requeue 使其清除 Conflicted 状态 |

**关键文件**:
- `listener_port_manager.rs` — 端口占用追踪
- `handlers/gateway.rs` — on_change/on_delete: 冲突检测 + requeue

### 6. AttachedRouteTracker

**用途**: 追踪哪些 Route 附着到哪些 Gateway，用于 Gateway status 中的 attachedRoutes 计数更新

| 方向 | 触发条件 | 目标资源 | 机制 |
|---|---|---|---|
| Route → Gateway | 附着状态变更 | 父 Gateway | `requeue_parent_gateways()` — 仅在附着列表实际变化时触发 |

**关键文件**:
- `attached_route_tracker.rs` — 附着状态存储
- `handlers/mod.rs` — `update_attached_route_tracker()`, `requeue_parent_gateways()`

## Post-Init 重校验（CachesReady）

所有 Processor 完成 `on_init_done()` 后，执行三个重校验函数解决初始化阶段的资源到达顺序问题：

| 序号 | 函数 | 效果 |
|---|---|---|
| 1 | `trigger_full_cross_ns_revalidation()` | requeue 所有拥有跨命名空间引用的资源 |
| 2 | `trigger_gateway_secret_revalidation()` | requeue 所有 Gateway |
| 3 | `trigger_gateway_route_revalidation()` | requeue gateway_route_index 中所有已注册路由 |

调用位置：
- `kubernetes/center.rs`（主初始化 + HA all-serve 重载）
- `file_system/center.rs`

## Handler on_change/on_delete 清单

| Handler | on_change | on_delete | 注册的管理器 |
|---|---|---|---|
| HTTPRoute | 是 | 是 | gateway_route_index, attached_route_tracker, cross_ns_ref, service_ref |
| GRPCRoute | 是 | 是 | gateway_route_index, attached_route_tracker, cross_ns_ref, service_ref |
| TLSRoute | 是 | 是 | gateway_route_index, attached_route_tracker, service_ref |
| EdgionTls | 是 | 是 | gateway_route_index, secret_ref |
| TCPRoute | 是 | 是 | attached_route_tracker, service_ref |
| UDPRoute | 是 | 是 | attached_route_tracker, service_ref |
| Gateway | 是 | 是 | listener_port_manager, gateway_route_index（消费者） |
| Secret | 是 | 是 | 触发级联 requeue |
| Service | 是 | 是 | 触发依赖路由 requeue |
| ReferenceGrant | 是 | 是 | CrossNsRevalidationListener |
| ConfigMap | 是 | 是 | 无 requeue 依赖 |
| EndpointSlice | — | — | 直接存储，Gateway 按需消费 |
| Endpoints | — | — | 直接存储，Gateway 按需消费 |
| GatewayClass | — | — | Gateway 引用，无 requeue |
| EdgionGatewayConfig | — | — | Gateway 引用，无 requeue |

## 环安全

### TriggerChain

`TriggerChain` 记录完整的 requeue 级联路径（如 `HTTPRoute/ns/r1 → Gateway/ns/gw1 → HTTPRoute/ns/r2`），每次调用 `ctx.requeue(kind, key)` 时检查目标 (kind, key) 在链中出现的次数是否超过 `max_trigger_cycles`（默认 5）。若超过则丢弃 requeue 并记录错误。

### 打破 Route ↔ Gateway 循环

Route 和 Gateway 之间存在潜在的双向 requeue：

- **Gateway on_change → requeue Routes**: 仅在 hostname/port **实际变更**时触发（通过 `gateway_route_index` 的缓存比较）
- **Route on_change → requeue Gateway**: 仅在 **attachment 列表实际变化**时触发（通过 `AttachedRouteTracker` 判断）

这两个条件确保大多数情况下不会形成循环。即使形成循环，TriggerChain 的 `max_trigger_cycles=5` 也会强制终止。

### DAG 约束

资源类型间的 requeue 关系本质上是 DAG（有向无环图），除了 Route ↔ Gateway 这一对外无其他环路。上述打破机制确保该唯一环路不会导致无限级联。
