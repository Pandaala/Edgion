---
name: gateway-tls-route
description: TLS 路由：SNI 匹配、per-port 路由表、TLS 代理实现。
---

# TLS 路由

## 概述

TLSRoute 实现了 Gateway API 的 TLS 透传路由功能。与 TCPRoute 不同，TLSRoute 在 TLS 握手阶段利用 SNI（Server Name Indication）进行基于主机名的路由，将 TLS 连接原样透传到后端（不终止 TLS）。

## 模块结构

```
src/core/gateway/routes/tls/
├── mod.rs                  # 模块入口与类型导出
├── conf_handler_impl.rs    # ConfHandler<TLSRoute> 实现
├── routes_mgr.rs           # TlsRouteManager / GlobalTlsRouteManagers
├── gateway_tls_routes.rs   # TlsRouteTable：per-port TLS 路由表（SNI 索引）
└── proxy.rs                # EdgionTlsTcpProxy：TLS 透传代理实现
```

## SNI 匹配

`TlsRouteTable` 使用 `HashHost` 数据结构实现基于 SNI 的主机名匹配：

```rust
pub struct TlsRouteTable {
    host_map: HashHost<Vec<Arc<TLSRoute>>>,
    catch_all_routes: Option<Vec<Arc<TLSRoute>>>,
}
```

匹配优先级：

1. **精确主机名匹配**：`HashHost` 精确查找，O(1)。
2. **通配主机名匹配**：`HashHost` 内部处理 `*.example.com` 通配，自动返回最具体的通配匹配。
3. **Catch-all**：无 `spec.hostnames` 的路由作为回退。

`TlsRouteTable::from_routes` 构建过程：

1. 遍历所有路由，按 `spec.hostnames` 分桶（转为小写）。
2. 无 hostname 的路由归入 `catch_all_routes`。
3. 将分桶结果插入 `HashHost` 匹配器。

## Per-port 路由管理

`GlobalTlsRouteManagers` 遵循统一的 per-port 管理模式：

```
GlobalTlsRouteManagers
├── route_cache: DashMap<String, Arc<TLSRoute>>    # 全局路由缓存
└── by_port: DashMap<u16, Arc<TlsRouteManager>>
    └── TlsRouteManager
        └── route_table: ArcSwap<TlsRouteTable>    # per-port 快照
```

### 端口分桶

通过 `resolved_ports`（Controller 预计算）将路由分配到对应端口。无 `resolved_ports` 的路由会打印警告并跳过。

### 重建策略

- `rebuild_all_port_managers`：全量重建所有端口，清理无路由的过期端口。
- `rebuild_affected_port_managers`：仅重建受影响端口（增量更新时使用）。

## TLS 透传代理

`EdgionTlsTcpProxy` 是 TLS 透传代理的核心实现：

- 持有 `Arc<TlsRouteManager>`，通过 `load_route_table()` 获取当前路由快照。
- 在 TLS 握手阶段从 ClientHello 中提取 SNI。
- 调用 `TlsRouteTable::match_route(sni_hostname)` 选择路由。
- 从路由的 `backend_refs` 中选择后端，建立上游 TCP 连接。
- 双向透传 TLS 流量，不终止 TLS。

## 与 TLS 子系统的关系

TLSRoute 和 TLS 子系统（`tls/` 模块）服务于不同场景：

- **TLSRoute**：TLS 透传（Passthrough），Gateway 不终止 TLS，按 SNI 路由到后端。
- **TLS 子系统**：TLS 终止（Terminate），Gateway 终止 TLS 握手，解密后按 HTTP/gRPC 路由处理。

两者在同一端口上可以共存：Gateway Listener 的 `tls.mode` 决定使用哪种模式。

## 关键源文件

| 文件 | 职责 |
|------|------|
| `src/core/gateway/routes/tls/routes_mgr.rs` | GlobalTlsRouteManagers、TlsRouteManager |
| `src/core/gateway/routes/tls/gateway_tls_routes.rs` | TlsRouteTable（SNI 匹配） |
| `src/core/gateway/routes/tls/proxy.rs` | EdgionTlsTcpProxy |
| `src/core/gateway/routes/tls/conf_handler_impl.rs` | ConfHandler<TLSRoute> 实现 |
