---
name: gateway-http-route-engine
description: HTTP 路由匹配引擎：RadixPath、RegexSet、ConfHandler、代理实现、LB Policy 同步。
---

# HTTP 路由引擎

## 模块结构

```
src/core/gateway/routes/http/
├── conf_handler_impl.rs        # ConfHandler<HTTPRoute> 实现，路由解析与 DomainRouteRules 构建
├── match_engine/
│   ├── mod.rs
│   ├── radix_path.rs           # RadixPath：路径模式编译、优先级计算
│   ├── radix_route_match.rs    # RadixRouteMatchEngine：radix tree 匹配引擎
│   └── regex_routes_engine.rs  # RegexRoutesEngine：RegexSet 批量正则匹配
├── match_unit.rs               # HttpRouteRuleUnit：路由匹配单元、deep match 逻辑
├── routes_mgr.rs               # RouteRules / DomainRouteRules / GlobalHttpRouteManagers
├── proxy_http/                 # Pingora ProxyHttp trait 实现
│   ├── mod.rs                  # EdgionHttpProxy 主结构
│   ├── pg_request_filter.rs    # 请求过滤与路由匹配入口
│   ├── pg_upstream_peer.rs     # 后端选择与连接
│   ├── pg_response_filter.rs   # 响应过滤
│   └── ...                     # 其他 Pingora 回调
├── redirect_http.rs            # EdgionHttpRedirectProxy：HTTP→HTTPS 301 重定向
├── lb_policy_sync.rs           # LB 策略同步到全局 PolicyStore
└── tests.rs
```

## Radix Path 匹配

`RadixRouteMatchEngine` 基于不可变 radix tree 实现 Exact 和 Prefix 路径匹配：

1. **构建阶段**（`build`）：遍历所有 `HttpRouteRuleUnit`，为每条路径创建 `RadixPath`，插入 `RadixTreeBuilder`，最终 `freeze()` 为不可变 `RadixTree`。
2. **匹配阶段**（`match_route`）：
   - 调用 `tree.match_all_ext(path)` 一次性获取所有候选及 `MatchKind`。
   - 过滤：Exact 仅接受 `FullyConsumed`；Prefix 接受 `FullyConsumed` 或 `SegmentBoundary`。
   - 按 `priority_weight` 降序排列，同权按 `header_matcher_count` 降序。
   - 依次对候选执行 `deep_match`，首个通过者即为最终匹配。

`RadixPath` 核心字段：

| 字段 | 说明 |
|------|------|
| `original` | 原始路径模式 |
| `normalized` | 路径归一化（合并连续斜杠） |
| `priority_weight` | `segment_count * 4 + type_bonus` |
| `is_prefix_match` | 是否前缀匹配 |
| `has_params` | 是否包含 `:param` 参数段 |
| `segment_count` | 路径段数 |

radix tree 内部支持 `:param` 参数匹配；`::` 双冒号转义为字面冒号。

## Regex 路由匹配

`RegexRoutesEngine` 使用 `regex::RegexSet` 实现高效批量正则匹配：

- 所有正则模式一次编译为 `RegexSet`，匹配复杂度 O(M)（M = 输入路径长度）。
- 路由按模式字符串长度降序排列，优先匹配更长（更具体）的模式。
- 当 `RegexSet` 构建失败时回退到线性逐一匹配。
- regex 引擎在 `RouteRules.match_route` 中先于 radix 引擎执行。

## proxy_http 实现

`EdgionHttpProxy` 实现 Pingora 的 `ProxyHttp` trait，请求生命周期中的关键回调：

| 回调 | 文件 | 职责 |
|------|------|------|
| `new_ctx` | `pg_new_ctx.rs` | 创建 `EdgionHttpContext` |
| `early_request_filter` | `pg_early_request_filter.rs` | 协议发现（gRPC/gRPC-Web）、请求信息提取 |
| `request_filter` | `pg_request_filter.rs` | 路由匹配、插件执行、gRPC 路由尝试 |
| `upstream_peer` | `pg_upstream_peer.rs` | 后端选择、BackendTLSPolicy 查询、连接建立 |
| `request_body_filter` | `pg_request_body_filter.rs` | 请求体过滤 |
| `upstream_response_filter` | `pg_upstream_response_filter.rs` | 上游响应头处理 |
| `upstream_response_body_filter` | `pg_upstream_response_body_filter.rs` | 上游响应体处理 |
| `response_filter` | `pg_response_filter.rs` | 下游响应过滤 |
| `logging` | `pg_logging.rs` | 访问日志记录 |
| `fail_to_connect` | `pg_fail_to_connect.rs` | 连接失败处理 |
| `error_while_proxy` | `pg_error_while_proxy.rs` | 代理期间错误处理 |

## Redirect 处理

`EdgionHttpRedirectProxy` 是一个轻量级 `ProxyHttp` 实现，处理 HTTP→HTTPS 重定向：

- 通过 Gateway 注解 `edgion.io/http-to-https-redirect: "true"` 启用。
- 返回 301 状态码，`Location` 头指向 HTTPS URL。
- 自动处理端口：HTTPS 端口为 443 时省略端口号。

## LB Policy 同步

`lb_policy_sync.rs` 在路由更新时提取 LB 策略并同步到全局 `PolicyStore`：

- `sync_lb_policies_for_routes`：遍历所有 HTTPRoute 规则的 `backend_refs`，提取 `extension_info.lb_policy` 并更新 `PolicyStore`。
- `cleanup_lb_policies_for_routes`：移除被删除路由关联的 LB 策略。
- 支持的策略类型：`LBPolicyConsistentHash`（header.xxx / cookie.xxx / arg.xxx）、`LBPolicyLeastConn`。

## conf_handler_impl 处理流程

`ConfHandler<HTTPRoute>` 实现分为全量和增量两种模式：

### full_set

1. 同步 LB 策略。
2. 清空 `route_cache`，全量替换。
3. 调用 `rebuild_all_port_managers` 重建所有端口路由表。

### partial_update

1. 计算受影响端口集合（从变更路由的 `resolved_ports` / `parentRef.port` 获取）。
2. 合并 add+update，同步 LB 策略，清理 remove 的 LB 策略。
3. 更新 `route_cache`。
4. 调用 `rebuild_affected_port_managers` 仅重建受影响端口。

### 路由解析

`parse_http_routes_to_domain_rules` 将 HTTPRoute 解析为 `DomainRouteRulesMap`：

1. 验证路由有效性（有 parent_refs、rules、namespace、name）。
2. `filter_accepted_parent_refs`：过滤状态为 `Accepted=True` 的 parentRef。
3. `get_effective_hostnames`：优先使用 `resolved_hostnames`（Controller 预计算的交集），回退到 `spec.hostnames`，最后回退到 `"*"`（catch-all）。
4. 对每个 hostname × rule × match 组合，分流为 regex route 或 normal route。
5. 为每个 match_item 预编译 header/query RegularExpression 正则。

`build_domain_route_rules_from_routes` 将解析结果构建为最终数据结构：精确域名进入 `exact_domain_map`，通配域名构建 `RadixHostMatchEngine`，无主机名路由成为 `catch_all_routes`。

## 关键源文件

| 文件 | 职责 |
|------|------|
| `src/core/gateway/routes/http/routes_mgr.rs` | GlobalHttpRouteManagers、RouteRules、DomainRouteRules |
| `src/core/gateway/routes/http/conf_handler_impl.rs` | ConfHandler 实现、路由解析 |
| `src/core/gateway/routes/http/match_engine/radix_route_match.rs` | RadixRouteMatchEngine |
| `src/core/gateway/routes/http/match_engine/regex_routes_engine.rs` | RegexRoutesEngine |
| `src/core/gateway/routes/http/match_engine/radix_path.rs` | RadixPath 优先级计算 |
| `src/core/gateway/routes/http/match_unit.rs` | HttpRouteRuleUnit、deep_match |
| `src/core/gateway/routes/http/proxy_http/mod.rs` | EdgionHttpProxy |
| `src/core/gateway/routes/http/redirect_http.rs` | EdgionHttpRedirectProxy |
| `src/core/gateway/routes/http/lb_policy_sync.rs` | LB 策略同步 |
