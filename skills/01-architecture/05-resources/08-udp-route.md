---
name: resource-udp-route
description: UDPRoute 资源：L4 UDP 路由、per-listener 路由表、客户端会话响应路径。
---

# UDPRoute 资源

> **通用流程**: 参见 [00-resource-flow.md](00-resource-flow.md)

UDPRoute 定义 L4 UDP 层的路由规则，将 UDP 数据包转发到后端 Service。UDP 协议本身无连接；Gateway 在 client→backend 方向按包查路由，在 backend→client 响应方向维护客户端会话。

## 源码位置

- Controller Handler: `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/udp_route.rs`
- Gateway ConfHandler: `src/core/gateway/routes/udp/conf_handler_impl.rs`
- 路由管理器: `src/core/gateway/routes/udp/routes_mgr.rs`（GlobalUdpRouteManagers）
- 类型定义: `src/types/resources/udp_route.rs`

## Controller 侧处理

### validate

1. 调用 `validate_udp_route_if_enabled()`（当前为空操作，跨命名空间校验由 parse 阶段的 `ref_denied` 处理）
2. 调用 `validate_backend_refs()` 校验所有 backendRef

### parse

1. 记录跨命名空间引用到 CrossNsRefManager
2. 注册 Service 引用到 ServiceRefManager
3. 标记被拒绝的跨命名空间 backendRef
4. 清除并重算 `resolved_listeners`：通过 `resolve_listener_keys()` 从 parentRef 解析 `Vec<ListenerKey>`（逻辑与 TCPRoute/TLSRoute 一致）
5. 若 resolved_listeners 为 None，记录 debug 日志

### on_change

注册到 GatewayRouteIndex（Gateway 变更时 requeue），更新 AttachedRouteTracker，必要时 requeue 父 Gateway。

### on_delete

清除 CrossNsRefManager、ServiceRefManager、GatewayRouteIndex、AttachedRouteTracker 注册。

### update_status

为每个 parentRef 生成 RouteParentStatus，设置 Accepted 和 ResolvedRefs Conditions。UDPRoute 不传递 route_hostnames（UDP 无域名概念）。

## Gateway 侧处理

UDPRoute 的 ConfHandler 将路由编译到 `GlobalUdpRouteManagers`：

1. **路由初始化**：调用 `initialize_route()` 验证并准备路由数据
2. **per-listener 管理**：根据 resolved_listeners 注册到对应 `ListenerKey` 的路由管理器
3. **全量/增量更新**：full_set 时清空并重建；partial_update 时收集受影响 listener 仅重建这些 listener
4. **数据面模型**：client→backend 每个包独立加载当前路由表并选择后端；backend→client 依赖客户端会话回写响应

### 数据面会话

- 会话键为客户端地址；首次收到客户端包时创建会话，并为该会话绑定一个独立 upstream UDP socket
- 会话上限 `MAX_UDP_SESSIONS = 10000`，超过后新客户端包记录 `SessionLimitReached` 并丢弃
- `SESSION_TIMEOUT = 60s`，以双向包活动刷新；清理循环每 10s 移除超时会话并写 UDP access log
- 主监听退出时取消清理循环和所有 per-session upstream 接收任务

运维上需按并发客户端数预估 fd、任务和内存占用；会话不是纯无状态转发，后端响应必须经对应 upstream socket 关联回客户端。

## 跨资源关联

| 关联方向 | 目标资源 | 关联机制 | 说明 |
|---------|---------|---------|------|
| UDPRoute → Gateway | Gateway | parentRef | 路由挂载到 Gateway 的 UDP 协议 Listener |
| UDPRoute → Service | Service | backendRef | UDP 数据包转发的后端目标 |
| UDPRoute ← ReferenceGrant | ReferenceGrant | 跨命名空间 backendRef | 控制跨命名空间后端引用的授权 |
