# Filter 校验与约束执行

优先级: P1

## 1. URLRewrite + RequestRedirect 冲突校验

### 现状

- Gateway API 规范明确禁止同一 rule 同时使用 URLRewrite 和 RequestRedirect
- `plugin_runtime.rs` 按顺序执行 filters，不检查冲突
- 若两者共存：rewrite 先修改 header，redirect 再短路返回，产生未定义行为

### 需要实现

1. 在 `PluginRuntime::from_httproute_filters()` 中添加校验
2. 检测到冲突时拒绝该 rule，设置 Route condition `Accepted: False`，reason `UnsupportedValue`
3. 同样适用于 per-backend filters

### 涉及文件

- `src/core/gateway/plugins/runtime/plugin_runtime.rs`
- `src/core/controller/sync_runtime/resource_processor/handlers/http_route.rs` — controller 侧也可校验

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
