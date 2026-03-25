---
name: gateway-tcp-route
description: TCP 路由：per-port 路由表、TCP 代理实现、ConfHandler。
---

# TCP 路由

## 概述

TCPRoute 实现了 Gateway API 的 L4 TCP 路由功能。TCP 路由的语义比 HTTP/gRPC 简单得多：没有基于主机名或路径的匹配维度，每个端口绑定一组路由，使用 first-match 语义选择后端。

## 模块结构

```
src/core/gateway/routes/tcp/
├── mod.rs                # 模块入口与类型导出
├── conf_handler_impl.rs  # ConfHandler<TCPRoute> 实现
├── routes_mgr.rs         # TcpPortRouteManager / GlobalTcpRouteManagers
├── tcp_route_table.rs    # TcpRouteTable：per-port 不可变路由快照
└── edgion_tcp.rs         # EdgionTcpProxy：TCP 代理实现
```

## Per-port 路由表

`TcpRouteTable` 是一个极简的不可变快照：

```rust
pub struct TcpRouteTable {
    routes: Vec<Arc<TCPRoute>>,
}
```

- **构建**：`from_routes` 从 `HashMap<String, Arc<TCPRoute>>` 收集所有路由到一个 Vec。
- **匹配**：`match_route()` 使用 first-match 语义，直接返回 `routes.first()`。
- TCP 没有主机名维度（不同于 TLS 的 SNI 匹配），因此不需要 `HashHost` 索引。
- 按 Gateway API 规范，每个 TCP listener 通常至多绑定一个 TCPRoute。

## 路由管理

`GlobalTcpRouteManagers` 管理全局 TCP 路由，结构遵循统一的 per-port 模式：

```
GlobalTcpRouteManagers
├── route_cache: DashMap<String, Arc<TCPRoute>>    # 全局路由缓存
└── by_port: DashMap<u16, Arc<TcpPortRouteManager>>
    └── TcpPortRouteManager
        └── route_table: ArcSwap<TcpRouteTable>    # per-port 快照
```

### 端口分桶

`bucket_routes_by_port` 通过 `resolved_ports`（Controller 预计算）将路由分配到对应端口。若路由无 `resolved_ports`，会打印警告并跳过（TCP 路由必须绑定到具体端口）。

### 重建策略

- `rebuild_all_port_managers`：全量重建所有端口路由表，清理无路由的过期端口。
- `rebuild_affected_port_managers`：仅重建受影响端口（增量更新时使用）。

## TCP 代理

`EdgionTcpProxy` 是 TCP 代理的核心实现：

- 持有 `Arc<TcpPortRouteManager>`，通过 `load_route_table()` 获取当前路由快照。
- 每个连接调用 `match_route()` 获取路由。
- 从路由的 `backend_refs` 中选择后端，建立上游 TCP 连接。
- 双向透明数据转发，不解析应用层协议。

## ConfHandler 处理

`ConfHandler<TCPRoute>` 实现：

- `full_set`：清空 route_cache，全量替换，调用 `rebuild_all_port_managers`。
- `partial_update`：计算受影响端口，更新 route_cache，调用 `rebuild_affected_port_managers`。

路由的 `ArcSwap` 原子切换确保已有连接不受影响，新连接自动获取最新快照。

## 关键源文件

| 文件 | 职责 |
|------|------|
| `src/core/gateway/routes/tcp/routes_mgr.rs` | GlobalTcpRouteManagers、TcpPortRouteManager |
| `src/core/gateway/routes/tcp/tcp_route_table.rs` | TcpRouteTable |
| `src/core/gateway/routes/tcp/edgion_tcp.rs` | EdgionTcpProxy |
| `src/core/gateway/routes/tcp/conf_handler_impl.rs` | ConfHandler<TCPRoute> 实现 |
