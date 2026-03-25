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

## ~~2. backendRequest <= request 超时约束~~ ✅ 已完成

在 `ParsedRouteTimeouts::from_config()` 中实现 clamp：`backendRequest > request` 时自动截断到 request 值并输出 warn 日志。

- 涉及文件：`src/types/resources/http_route.rs:860-868`
- 单元测试：3 个（clamp、within limit、backend only）

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

## 4. ~~parametersRef 消费~~ ✅ 已完成

### 实现方案

- **Controller 侧**：`GatewayClassHandler::validate()` 校验 `parametersRef`（如设置），`group` 必须为 `edgion.io`，`kind` 必须为 `EdgionGatewayConfig`，不合法则 `Accepted: False`。未设置 `parametersRef` 不报错。
- **Gateway 侧**：`parametersRef` 为空时使用 `EdgionGatewayConfigSpec::default()` 内置默认值，不再跳过 Gateway。
- **设计决策**：明确 1 Controller : 1 GatewayClass（与 Envoy Gateway、NGINX Gateway Fabric 一致），不支持单实例多 GatewayClass。

### 涉及文件

- `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/gateway_class.rs` — `validate()`
- `src/core/gateway/runtime/server/base.rs` — 默认值回退
- `src/types/resources/edgion_gateway_config.rs` — `Default for EdgionGatewayConfigSpec`
