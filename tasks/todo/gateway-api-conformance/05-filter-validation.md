# Filter 校验与约束执行

优先级: P1

## 1. ~~URLRewrite + RequestRedirect 冲突校验~~ ✅ 已完成

### 实现方案

在 Controller 侧 `find_invalid_rule_indices()` 中检测冲突，走 `PartiallyInvalid` 条件链路（per-rule 粒度），非冲突 rule 不受影响。

覆盖三种冲突场景：
1. Rule-level：同一 rule 的 filters 同时含 URLRewrite + RequestRedirect
2. Backend-level：同一 backend ref 的 filters 同时含两者
3. Cross-level：rule filters 含其一 + backend filters 含另一

### 涉及文件

- `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/http_route.rs` — `has_rewrite_redirect_conflict()` + `find_invalid_rule_indices()`

### 集成测试

- `examples/test/conf/HTTPRoute/Filters/FilterConflict/` — 3 rule 场景（rewrite-only → 200、conflict → 404、redirect-only → 302）
- `examples/code/client/suites/http_route/filters/filter_conflict.rs`
- 运行：`./examples/test/scripts/integration/run_integration.sh --no-prepare -r HTTPRoute -i Filters/FilterConflict`

---

## 2. backendRequest <= request 超时约束

### 现状

- `request` 和 `backendRequest` 超时均已实现
- 规范要求 `backendRequest` 必须 <= `request`
- 当前无此约束校验

### 需要实现

1. 在 route pre-parse 阶段（`ParsedRouteTimeouts::from_config`）验证约束
2. 不满足时：拒绝该 rule 或自动截断 backendRequest 到 request 值

### 涉及文件

- `src/types/resources/http_route_preparse.rs`

---

## 3. TCP/UDPRoute 单路由约束

### 现状

- 规范要求每个 listener 最多绑定一个 TCPRoute / UDPRoute
- `tcp_route_table.rs:33` 和 `udp_route_table.rs:33` 用 `first()` 取第一个匹配，静默忽略多余的

### 需要实现

1. Controller 侧：检测同一 listener port 上的重复 route 绑定
2. 多余 route 设置 `Accepted: False`，reason `RouteConflict`
3. 按规范的冲突解决规则（older creation timestamp 优先）

### 涉及文件

- `src/core/controller/sync_runtime/resource_processor/handlers/tcp_route.rs`
- `src/core/controller/sync_runtime/resource_processor/handlers/udp_route.rs`

---

## 4. parametersRef 消费

### 现状

- `GatewayClassSpec` 有 `ParametersReference` 字段
- handler 的 `parse()` 是空操作，用户设置后被静默忽略

### 建议

- 短期：在校验阶段对设置了 parametersRef 的 GatewayClass 发出 warning condition
- 长期：解析 parametersRef 引用的资源，用于 Gateway 级别配置
