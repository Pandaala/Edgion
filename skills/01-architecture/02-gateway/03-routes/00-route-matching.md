---
name: gateway-route-matching
description: 路由匹配总览：多级流水线（Port→Domain→Path→DeepMatch）、per-port 隔离、注册流程、原子切换。
---

# 路由匹配总览

## 多级匹配流水线

请求到达 Gateway 后，路由匹配按以下层级依次执行：

```
Port → Domain (精确/通配/catch-all) → Path (regex/radix) → Deep match
```

1. **Port 隔离**：根据连接到达的监听端口，选择对应的 per-port 路由表。
2. **Domain 匹配**：在 `DomainRouteRules` 中按主机名查找 `RouteRules`。
3. **Path 匹配**：在 `RouteRules` 中先尝试 regex 引擎，再尝试 radix 引擎。
4. **Deep match**：对候选路由执行 Gateway/Listener 约束、HTTP method、headers、query params 精确校验。

## Per-port 隔离

每个 listener 拥有独立的路由表，由 `GlobalHttpRouteManagers` 管理：

```
GlobalHttpRouteManagers
├── route_cache: DashMap<String, HTTPRoute>        # 全局路由缓存
└── by_port: DashMap<u16, Arc<HttpPortRouteManager>>
    └── HttpPortRouteManager
        └── route_table: ArcSwap<DomainRouteRules>  # per-port 快照
```

- `route_cache` 保存所有 HTTPRoute 的规范副本（`resource_key -> HTTPRoute`）。
- `by_port` 为每个监听端口维护一个 `HttpPortRouteManager`，其中的 `ArcSwap<DomainRouteRules>` 支持无锁读。
- 路由表更新时，仅受影响的端口会被重建；其他端口不受干扰。
- `pg_request_filter` 在每个请求中通过 `load_route_table()` 获取当前快照，整个请求生命周期内引用不变。

其他协议（gRPC / TCP / TLS / UDP）的全局管理器遵循相同的 `Global*RouteManagers → per-port manager → ArcSwap` 模式。

## Domain 匹配

`DomainRouteRules` 内部包含三级域名查找结构：

| 层级 | 数据结构 | 复杂度 | 说明 |
|------|----------|--------|------|
| 精确匹配 | `ArcSwap<HashMap<String, Arc<RouteRules>>>` | O(1) | 主机名小写后直接 HashMap 查找 |
| 通配匹配 | `ArcSwap<Option<RadixHostMatchEngine<RouteRules>>>` | O(log n) | `*.example.com` 类通配域名，使用 RadixHostMatchEngine |
| Catch-all | `ArcSwap<Option<Arc<RouteRules>>>` | O(1) | HTTPRoute 未指定 `spec.hostnames` 时的回退 |

匹配优先级严格按 Gateway API 规范：精确 > 通配 > catch-all。

## Path 匹配

每个 `RouteRules` 内部持有两个引擎：

### RadixRouteMatchEngine（Exact / Prefix）

- 基于 radix tree（`RadixTreeBuilder` + `RadixTree`），所有路由在初始化时一次性插入、冻结为不可变树。
- 单次 `match_all_ext` 调用遍历树，返回所有候选及其 `MatchKind`（`FullyConsumed` / `SegmentBoundary` / `PartialSegment`）。
- Exact 路由仅接受 `FullyConsumed`；Prefix 路由接受 `FullyConsumed` 或 `SegmentBoundary`（拒绝 `/v2` 匹配 `/v2example` 这类部分段匹配）。
- 候选按 `priority_weight` 降序排列后依次执行 deep match。

### RegexRoutesEngine（RegularExpression）

- 使用 `regex::RegexSet` 对所有正则模式做一次性批量匹配，性能 O(M)（M 为路径长度），与路由数量无关。
- 路由按正则模式长度降序排列（越长越优先）。
- 匹配顺序：regex 引擎先于 radix 引擎执行。

## RadixPath 优先级

`RadixPath` 的 `priority_weight` 计算公式：

```
priority_weight = segment_count * 4 + type_bonus
```

`type_bonus` 取值：

| 类型 | is_prefix | has_params | type_bonus |
|------|-----------|------------|------------|
| 静态精确 | false | false | 3 |
| 参数精确 | false | true | 2 |
| 静态前缀 | true | false | 1 |
| 参数前缀 | true | true | 0 |

排序规则：更高 `priority_weight` 优先；同权时按 `header_matcher_count()` 降序（更多 header 匹配条件更具体）。

示例：`/api/users`（静态精确，weight=11）优先于 `/api/:id`（参数精确，weight=10）优先于 `/api/users`（静态前缀，weight=9）。

## Deep match

路由候选通过路径匹配后，`deep_match_common` 执行以下精确校验：

1. **Gateway/Listener 约束**（`check_gateway_listener_match`）：
   - 验证 parentRef 的 namespace + name 匹配 Gateway。
   - 验证 sectionName 匹配 Listener（如指定）。
   - 验证请求主机名匹配 Listener hostname（HTTP Listener Isolation）。
   - 若同端口存在更具体的 Listener 也匹配该主机名，则拒绝（防止模糊匹配）。
   - 验证 AllowedRoutes（namespace 限制 + kind 限制）。
2. **HTTP Method**：`match_item.method` 与请求 method 对比。
3. **Headers**：所有 header 匹配条件必须全部满足（AND 逻辑），支持 `Exact` 和 `RegularExpression`（预编译 regex）。
4. **Query Params**：所有 query param 匹配条件必须全部满足（AND 逻辑），支持 `Exact` 和 `RegularExpression`。

## 路由注册流程

```
Controller 推送 HTTPRoute
       │
       ▼
ConfHandler<HTTPRoute>::full_set / partial_update
       │
       ▼
route_cache 更新（DashMap insert/remove）
       │
       ▼
sync_lb_policies_for_routes（同步 LB 策略到全局 PolicyStore）
       │
       ▼
rebuild_all_port_managers / rebuild_affected_port_managers
       │
       ├── bucket_routes_by_port（按 resolved_ports 分桶）
       │   └── 回退：parentRef.port → 全部端口
       │
       ├── 对每个受影响端口：
       │   ├── parse_http_routes_to_domain_rules（解析路由为 domain→rules）
       │   │   ├── filter_accepted_parent_refs（过滤已 Accepted 的 parentRef）
       │   │   ├── get_effective_hostnames（resolved_hostnames > spec.hostnames > "*"）
       │   │   └── 分流：regex routes vs normal routes
       │   ├── build_domain_route_rules_from_routes
       │   │   ├── RadixRouteMatchEngine::build（radix tree 冻结）
       │   │   ├── RegexRoutesEngine::build（RegexSet 编译）
       │   │   └── RadixHostMatchEngine 初始化（通配域名）
       │   └── HttpPortRouteManager::store_route_table（ArcSwap 原子切换）
       │
       └── 清理无路由且无活跃 Listener 的过期端口
```

整个更新过程中，已有请求持有旧快照引用，不会被中断。新请求自动获取最新快照。

## 多 Gateway 端口共享

当多个 Gateway 的 Listener 监听同一端口时，所有 Gateway 的路由会合并到同一个 per-port `DomainRouteRules` 中。在 deep match 阶段，`check_gateway_listener_match` 通过遍历 `gateway_infos`（来自 `PortGatewayInfoStore`）来验证每条路由是否属于当前请求所匹配的 Gateway/Listener，从而实现逻辑上的 Listener 隔离。

## 关键源文件

| 文件 | 职责 |
|------|------|
| `src/core/gateway/routes/http/routes_mgr.rs` | `GlobalHttpRouteManagers`、`DomainRouteRules`、`RouteRules` |
| `src/core/gateway/routes/http/conf_handler_impl.rs` | `ConfHandler<HTTPRoute>` 实现、路由解析与构建 |
| `src/core/gateway/routes/http/match_engine/radix_route_match.rs` | `RadixRouteMatchEngine` |
| `src/core/gateway/routes/http/match_engine/regex_routes_engine.rs` | `RegexRoutesEngine` |
| `src/core/gateway/routes/http/match_engine/radix_path.rs` | `RadixPath` 优先级计算 |
| `src/core/gateway/routes/http/match_unit.rs` | `HttpRouteRuleUnit`、`deep_match_common` |
| `src/core/gateway/runtime/matching/route.rs` | `check_gateway_listener_match`、`hostname_matches_listener` |
