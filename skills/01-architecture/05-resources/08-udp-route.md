---
name: resource-udp-route
description: UDPRoute 资源：L4 UDP 路由、per-port 路由表、无状态 per-packet 转发。
---

# UDPRoute 资源

> **通用流程**: 参见 [00-resource-flow.md](00-resource-flow.md)

UDPRoute 定义 L4 UDP 层的路由规则，将 UDP 数据包转发到后端 Service。UDP 是无状态协议，路由工作在端口级别，采用 per-packet 转发模式。

## 源码位置

- Controller Handler: `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/udp_route.rs`
- Gateway ConfHandler: `src/core/gateway/routes/udp/conf_handler_impl.rs`
- 路由管理器: `src/core/gateway/routes/udp/routes_mgr.rs`（GlobalUdpRouteManagers）
- 类型定义: `src/types/resources/udp_route.rs`

## Controller 侧处理

### validate

1. 调用 `validate_udp_route_if_enabled()` 执行 ReferenceGrant 校验
2. 调用 `validate_backend_refs()` 校验所有 backendRef

### parse

1. 记录跨命名空间引用到 CrossNsRefManager
2. 注册 Service 引用到 ServiceRefManager
3. 标记被拒绝的跨命名空间 backendRef
4. 清除并重算 `resolved_ports`：从 parentRef 解析 Listener 端口（逻辑与 TCPRoute/TLSRoute 一致）
5. 若 resolved_ports 为 None，记录 debug 日志

### on_change

注册到 GatewayRouteIndex（Gateway 变更时 requeue），更新 AttachedRouteTracker，必要时 requeue 父 Gateway。

### on_delete

清除 CrossNsRefManager、ServiceRefManager、GatewayRouteIndex、AttachedRouteTracker 注册。

### update_status

为每个 parentRef 生成 RouteParentStatus，设置 Accepted 和 ResolvedRefs Conditions。UDPRoute 不传递 route_hostnames（UDP 无域名概念）。

## Gateway 侧处理

UDPRoute 的 ConfHandler 将路由编译到 `GlobalUdpRouteManagers`：

1. **路由初始化**：调用 `initialize_route()` 验证并准备路由数据
2. **per-port 管理**：根据 resolved_ports 注册到对应端口的路由管理器
3. **全量/增量更新**：full_set 时清空并重建；partial_update 时收集受影响端口仅重建这些端口
4. **无状态转发**：UDP 为 per-packet 转发，每个数据包独立路由到后端

## 跨资源关联

| 关联方向 | 目标资源 | 关联机制 | 说明 |
|---------|---------|---------|------|
| UDPRoute → Gateway | Gateway | parentRef | 路由挂载到 Gateway 的 UDP 协议 Listener |
| UDPRoute → Service | Service | backendRef | UDP 数据包转发的后端目标 |
| UDPRoute ← ReferenceGrant | ReferenceGrant | 跨命名空间 backendRef | 控制跨命名空间后端引用的授权 |
