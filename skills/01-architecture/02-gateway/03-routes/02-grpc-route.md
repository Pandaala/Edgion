---
name: gateway-grpc-route-engine
description: gRPC 路由引擎：Service/Method 匹配、gRPC-Web 支持、HTTP 管线集成。
---

# gRPC 路由引擎

## 概述

gRPC 路由基于 Gateway API `GRPCRoute` CRD 实现，通过解析 HTTP/2 路径中的 `/{service}/{method}` 进行服务和方法级别的路由。gRPC 路由集成在 HTTP 管道中，共享 Pingora ProxyHttp 基础设施。

## 模块结构

```
src/core/gateway/routes/grpc/
├── mod.rs                # 模块入口与类型导出
├── conf_handler_impl.rs  # ConfHandler<GRPCRoute> 实现
├── match_engine.rs       # GrpcMatchEngine：service/method 路由匹配
├── match_unit.rs         # GrpcRouteRuleUnit：路由匹配单元
├── routes_mgr.rs         # GrpcRouteRules / GlobalGrpcRouteManagers
└── integration.rs        # HTTP 管道集成接口
```

## Service/Method 匹配

`GrpcMatchEngine` 支持 Exact 和 RegularExpression 两种匹配类型，实现七级优先级匹配：

| 优先级 | 匹配类型 | 数据结构 | 说明 |
|--------|----------|----------|------|
| 1 | Exact service + Exact method | `HashMap<(String, String), Vec<...>>` | 精确匹配，O(1) |
| 2 | Exact service + Regex method | `Vec<Arc<GrpcRouteRuleUnit>>` | service 精确 + method 正则 |
| 3 | Regex service + Exact method | `Vec<Arc<GrpcRouteRuleUnit>>` | service 正则 + method 精确 |
| 4 | Regex service + Regex method | `Vec<Arc<GrpcRouteRuleUnit>>` | 双正则匹配 |
| 5 | Exact service only | `HashMap<String, Vec<...>>` | 仅指定 service，匹配所有 method |
| 6 | Regex service only | `Vec<Arc<GrpcRouteRuleUnit>>` | service 正则，method 未指定 |
| 7 | catch-all | `Option<Arc<GrpcRouteRuleUnit>>` | 未指定 service 和 method |

gRPC 路径解析规则（`parse_grpc_path` 函数）：

- 输入：`/helloworld.Greeter/SayHello`
- 输出：service = `helloworld.Greeter`，method = `SayHello`
- 路径必须恰好两段，否则返回 `InvalidGrpcPath` 错误。

同一个 (service, method) 组合可以有多条路由（不同 hostname 或 header 条件），存储为 `Vec<Arc<GrpcRouteRuleUnit>>`，按顺序执行 `deep_match` 直到首个匹配。

### 正则匹配

当 `GRPCMethodMatch.type = RegularExpression` 时，`type` 同时作用于 service 和 method 两个字段（Gateway API 规范语义）。正则模式在路由加载时预编译为 `Arc<Regex>`，存储在 `GrpcMatchInfo` 的 `compiled_service_regex` / `compiled_method_regex` 字段中。正则路由按模式长度降序排列（更长 = 更具体 = 更高优先级）。

与 HTTPRoute 的 `RegexRoutesEngine` 不同，gRPC 正则匹配不使用 `RegexSet`，而是线性遍历预编译正则——因为 gRPC 正则路由数量通常很少。

## gRPC-Web 支持

gRPC-Web 协议在 `early_request_filter` 阶段通过请求 Content-Type 识别：

- `application/grpc` → 标记为 `grpc`
- `application/grpc-web` 或 `application/grpc-web+proto` → 标记为 `grpc-web`

识别结果存储在 `ctx.request_info.discover_protocol` 中。`is_grpc_protocol` 辅助函数检查该字段，gRPC 和 gRPC-Web 使用相同的路由匹配逻辑。

## HTTP 管道集成

gRPC 路由通过 `integration.rs` 模块集成到 HTTP 请求处理管道中：

### try_match_grpc_route

在 `pg_request_filter` 中调用，尝试匹配 gRPC 路由：

1. 解析 gRPC 路径为 service/method，存入 `ctx.request_info`。
2. 调用 `DomainGrpcRouteRules::match_route` 执行匹配。
3. 匹配成功：设置 `ctx.grpc_route_unit` 和 `ctx.gateway_info`，标记 `ctx.is_grpc_route_matched = true`。
4. 匹配失败：返回 `Ok(false)`，回退到 HTTP 路由匹配。

### handle_grpc_upstream

在 `upstream_peer` 中调用，处理 gRPC 后端选择：

1. 检查是否已有选定后端（`ctx.selected_grpc_backend`）。
2. 从 `grpc_route_unit` 的 rule 中通过 `GrpcRouteRules::select_backend` 加权选择后端。
3. 建立上游连接（与 HTTP 路由共享后端连接逻辑）。

### 后端选择

`GrpcRouteRules::select_backend` 使用 `BackendSelector` 进行加权随机选择：

- 默认权重为 1（未指定时）。
- 支持 `ref_denied` 检查（跨命名空间引用需要 ReferenceGrant）。

## 路由管理

`GlobalGrpcRouteManagers` 遵循与 HTTP 相同的 per-port 管理模式：

```
GlobalGrpcRouteManagers
├── route_cache: DashMap<String, GRPCRoute>
├── by_port: DashMap<u16, Arc<GrpcPortRouteManager>>
└── route_units_cache: Mutex<HashMap<String, Vec<Arc<GrpcRouteRuleUnit>>>>
```

特有设计：

- `route_units_cache` 缓存解析后的路由单元，避免每次端口重建时重复解析。
- gRPC 不使用 hostname 维度分隔（与 HTTP 不同），所有路由放入单个 `GrpcRouteRules`。
- 在 `GrpcMatchEngine::match_route` 中通过 `deep_match` 验证请求 hostname 与路由 `effective_hostnames` 的关系。
- 端口分桶逻辑与 HTTP 一致：`resolved_ports` → `parentRef.port` → 全部端口（回退）。

## 关键源文件

| 文件 | 职责 |
|------|------|
| `src/core/gateway/routes/grpc/match_engine.rs` | GrpcMatchEngine、parse_grpc_path |
| `src/core/gateway/routes/grpc/match_unit.rs` | GrpcRouteRuleUnit、GrpcMatchInfo |
| `src/core/gateway/routes/grpc/routes_mgr.rs` | GlobalGrpcRouteManagers、GrpcRouteRules |
| `src/core/gateway/routes/grpc/conf_handler_impl.rs` | ConfHandler<GRPCRoute> 实现 |
| `src/core/gateway/routes/grpc/integration.rs` | try_match_grpc_route、handle_grpc_upstream |
