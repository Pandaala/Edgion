---
name: gateway-udp-route
description: UDP 路由：per-port 路由表、无状态代理、ConfHandler。
---

# UDP 路由

## 概述

UDPRoute 实现了 Gateway API 的 L4 UDP 路由功能。UDP 路由是最简单的路由类型：无连接状态、无主机名匹配维度，每个端口绑定至多一条路由，使用 first-match 语义选择后端。

## 模块结构

```
src/core/gateway/routes/udp/
├── mod.rs                # 模块入口与类型导出
├── conf_handler_impl.rs  # ConfHandler<UDPRoute> 实现
├── routes_mgr.rs         # UdpPortRouteManager / GlobalUdpRouteManagers
├── udp_route_table.rs    # UdpRouteTable：per-port 不可变路由快照
└── edgion_udp.rs         # EdgionUdpProxy：UDP 代理实现
```

## Per-port 路由表

`UdpRouteTable` 结构与 `TcpRouteTable` 完全一致：

```rust
pub struct UdpRouteTable {
    routes: Vec<Arc<UDPRoute>>,
}
```

- **构建**：`from_routes` 从 `HashMap<String, Arc<UDPRoute>>` 收集所有路由。
- **匹配**：`match_route()` 使用 first-match 语义，返回 `routes.first()`。
- 无主机名维度（UDP 没有类似 SNI 的机制）。
- 按 Gateway API 规范，每个 UDP listener 通常至多绑定一个 UDPRoute。

## 路由管理

`GlobalUdpRouteManagers` 遵循统一的 per-port 管理模式：

```
GlobalUdpRouteManagers
├── route_cache: DashMap<String, Arc<UDPRoute>>    # 全局路由缓存
└── by_port: DashMap<u16, Arc<UdpPortRouteManager>>
    └── UdpPortRouteManager
        └── route_table: ArcSwap<UdpRouteTable>    # per-port 快照
```

### 端口分桶

通过 `resolved_ports`（Controller 预计算）将路由分配到对应端口。无 `resolved_ports` 的路由会打印警告并跳过。

### 重建策略

- `rebuild_all_port_managers`：全量重建所有端口路由表，清理无路由的过期端口。
- `rebuild_affected_port_managers`：仅重建受影响端口（增量更新时使用）。

## UDP 无状态代理

`EdgionUdpProxy` 是 UDP 代理的核心实现：

- 持有 `Arc<UdpPortRouteManager>`，通过 `load_route_table()` 获取当前路由快照（per-packet 热路径）。
- 每个数据包调用 `match_route()` 获取路由。
- 从路由的 `backend_refs` 中选择后端。
- UDP 是无连接协议，每个数据包独立转发，不维护连接状态。
- 支持加权后端选择（与 TCP/HTTP 相同的 `BackendSelector` 机制）。

## ConfHandler 处理

`ConfHandler<UDPRoute>` 实现与 TCPRoute 完全一致：

- `full_set`：清空 route_cache，全量替换，调用 `rebuild_all_port_managers`。
- `partial_update`：计算受影响端口，更新 route_cache，调用 `rebuild_affected_port_managers`。

路由的 `ArcSwap` 原子切换确保已有数据包处理不受影响。

## 关键源文件

| 文件 | 职责 |
|------|------|
| `src/core/gateway/routes/udp/routes_mgr.rs` | GlobalUdpRouteManagers、UdpPortRouteManager |
| `src/core/gateway/routes/udp/udp_route_table.rs` | UdpRouteTable |
| `src/core/gateway/routes/udp/edgion_udp.rs` | EdgionUdpProxy |
| `src/core/gateway/routes/udp/conf_handler_impl.rs` | ConfHandler<UDPRoute> 实现 |
