---
name: resource-tcp-route
description: TCPRoute 资源：L4 TCP 路由、per-port 路由表、first-match 语义。
---

# TCPRoute 资源

> **通用流程**: 参见 [00-resource-flow.md](00-resource-flow.md)

TCPRoute 定义 L4 TCP 层的路由规则，将 TCP 连接转发到后端 Service。TCPRoute 工作在端口级别，每个端口维护独立的路由表，采用 first-match 语义。

## 源码位置

- Controller Handler: `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/tcp_route.rs`
- Gateway ConfHandler: `src/core/gateway/routes/tcp/conf_handler_impl.rs`
- 路由管理器: `src/core/gateway/routes/tcp/routes_mgr.rs`
- 类型定义: `src/types/resources/tcp_route.rs`

## Controller 侧处理

### validate

1. 调用 `validate_tcp_route_if_enabled()` 执行 ReferenceGrant 校验
2. 调用 `validate_backend_refs()` 校验所有 backendRef

### parse

1. 记录跨命名空间引用到 CrossNsRefManager
2. 注册 Service 引用到 ServiceRefManager
3. 标记被拒绝的跨命名空间 backendRef（设置 `ref_denied`）
4. 清除并重算 `resolved_ports`：从 parentRef 解析 Listener 端口，逻辑与 HTTPRoute 相同（parentRef.port 优先，否则通过 sectionName 或全部 Listener 获取，受 allowedRoutes 命名空间策略约束）
5. 若 resolved_ports 为 None，记录 debug 日志（Gateway 可能尚未到达）

### on_change

注册到 GatewayRouteIndex（Gateway 变更时 requeue），更新 AttachedRouteTracker，必要时 requeue 父 Gateway。

### on_delete

清除 CrossNsRefManager、ServiceRefManager、GatewayRouteIndex、AttachedRouteTracker 注册。

### update_status

为每个 parentRef 生成 RouteParentStatus，设置 Accepted 和 ResolvedRefs Conditions。TCPRoute 不传递 route_hostnames（TCP 无域名概念）。

## Gateway 侧处理

TCPRoute 的 ConfHandler 将路由编译到 `GlobalTcpRouteManagers`：

1. **路由初始化**：调用 `initialize_route()` 验证并准备路由数据
2. **per-port 管理**：根据 resolved_ports 注册到对应端口的路由管理器
3. **全量/增量更新**：full_set 时清空并重建所有路由缓存和端口管理器；partial_update 时收集受影响的端口（旧端口 + 新端口），仅重建这些端口的路由表
4. **first-match 语义**：同一端口上多个 TCPRoute 时按注册顺序匹配第一个

## 跨资源关联

| 关联方向 | 目标资源 | 关联机制 | 说明 |
|---------|---------|---------|------|
| TCPRoute → Gateway | Gateway | parentRef | 路由挂载到 Gateway 的 TCP 协议 Listener |
| TCPRoute → Service | Service | backendRef | TCP 连接转发的后端目标 |
| TCPRoute ← ReferenceGrant | ReferenceGrant | 跨命名空间 backendRef | 控制跨命名空间后端引用的授权 |
