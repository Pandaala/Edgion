---
name: resource-http-route
description: HTTPRoute 资源：路由规则、匹配条件、BackendRef、Filter、插件绑定、路由注册。
---

# HTTPRoute 资源

> **通用流程**: 参见 [00-resource-flow.md](00-resource-flow.md)

HTTPRoute 是 Edgion 中最复杂的资源，定义 HTTP 层的路由规则，包括路径/头部/查询参数匹配、后端引用、过滤器和插件执行。

## 源码位置

- Controller Handler: `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/http_route.rs`
- Gateway ConfHandler: `src/core/gateway/routes/http/conf_handler_impl.rs`
- 路由管理器: `src/core/gateway/routes/http/routes_mgr.rs`
- 匹配引擎: `src/core/gateway/routes/http/match_engine/`
- 类型定义: `src/types/resources/http_route.rs`
- 预解析: `src/types/resources/http_route_preparse.rs`

## Controller 侧处理

### validate

1. 调用 `validate_http_route_if_enabled()` 执行 ReferenceGrant 校验
2. 调用 `validate_backend_refs()` 校验所有 backendRef 的 kind/存在性

### preparse（类型层面）

HTTPRoute 的 `preparse()` 方法在类型层面构建运行时结构：
- 将 `filters` 编译为 `PluginRuntime`（插件执行链）
- 预编译正则表达式匹配器
- 构建路径匹配信息

### parse

1. **记录跨命名空间引用**：将 backendRef 中的跨命名空间引用注册到 CrossNsRefManager，ReferenceGrant 变更时自动 requeue
2. **注册 Service 引用**：将所有 Service 类型的 backendRef 注册到 ServiceRefManager，Service 变更时自动 requeue
3. **清除并重算 resolved_ports**：从 parentRef 解析目标 Listener 的端口号，用于 Gateway 侧的 per-port 路由隔离。解析逻辑：
   - parentRef.port 已指定 → 直接使用
   - parentRef.sectionName 已指定 → 查找对应 Listener 获取端口
   - 都未指定 → 使用 Gateway 所有 Listener 的端口（需通过 allowedRoutes 命名空间检查）
4. **解析 resolved_hostnames**：计算路由 hostnames 与 Gateway Listener hostnames 的交集
5. **标记被拒绝的跨命名空间引用**：对每个跨命名空间的 backendRef 检查 ReferenceGrant，未授权的设置 `ref_denied` 字段，Gateway 侧据此拒绝请求

### on_change

1. 注册到 `GatewayRouteIndex`（Gateway 变更时 requeue 此路由）
2. 更新 `AttachedRouteTracker`，若挂载关系变化则 requeue 父 Gateway（更新 attachedRoutes 计数）

### on_delete

- 清除 CrossNsRefManager、ServiceRefManager、GatewayRouteIndex、AttachedRouteTracker 注册
- 若 AttachedRouteTracker 有变化则 requeue 父 Gateway

### update_status

为每个 parentRef 生成独立的 RouteParentStatus：
- `Accepted`：校验 parentRef 对应的 Gateway/Listener 是否存在且允许此路由（hostname 交集非空、命名空间策略允许）
- `ResolvedRefs`：校验所有 backendRef 指向的 Service 是否存在、跨命名空间引用是否被拒绝
- 清理不再存在的 parentRef 对应的旧 status 条目

## Gateway 侧处理

HTTPRoute 的 ConfHandler 实现路由的编译和注册：

1. **parentRef 过滤**：仅编译 `Accepted=True` 的 parentRef（尚无 status 时乐观全部编译）
2. **per-port 路由管理**：根据 resolved_ports 将路由规则注册到对应端口的 `HttpRouteManager`
3. **域名路由表**：每个端口维护 `domain → route_rules` 映射，无 hostname 的路由注册到 `*`（全匹配）
4. **多级匹配引擎**：
   - `RadixHostMatchEngine`：基于 Radix Tree 的域名匹配
   - `RadixRouteMatchEngine`：基于 Radix Tree 的路径前缀匹配
   - `RegexRoutesEngine`：正则路径匹配
5. **请求处理流程**：`port → domain match → path match → header/query deep match → 选中规则 → 执行插件链 → proxy_http`
6. **LB 策略同步**：路由变更时同步更新对应的负载均衡策略

## 跨资源关联

| 关联方向 | 目标资源 | 关联机制 | 说明 |
|---------|---------|---------|------|
| HTTPRoute → Gateway | Gateway | parentRef | 路由挂载到 Gateway 的 Listener |
| HTTPRoute → Service | Service | backendRef | 路由的后端目标服务 |
| HTTPRoute → EdgionPlugins | EdgionPlugins | ExtensionRef filter | 通过 ExtensionRef 引用插件配置 |
| HTTPRoute → Secret | Secret | 通过插件间接引用 | JWT/BasicAuth 等插件引用 Secret |
| HTTPRoute ← ReferenceGrant | ReferenceGrant | 跨命名空间 backendRef | 控制跨命名空间后端引用的授权 |
