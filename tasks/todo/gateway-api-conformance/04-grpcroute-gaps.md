# GRPCRoute 功能差距

优先级: P1

## 1. RegularExpression 方法匹配

### 现状

- `GRPCMethodMatchType` 枚举定义了 `Exact` 和 `RegularExpression` 两种类型
- `match_engine.rs:31` 注释明确说明 "We only support Exact match type"
- 匹配引擎使用 HashMap 以 `(service, method)` 为 key 做精确查找
- 正则模式被静默忽略，用户配置了也不生效

### 需要实现

1. 类似 HTTPRoute 的双引擎架构：精确匹配走 HashMap，正则匹配走 `RegexSet`
2. 精确匹配优先于正则匹配（规范要求）
3. 正则匹配间按模式长度排序
4. 若不实现，至少在配置验证阶段拒绝正则类型并报错，而非静默忽略

### 涉及文件

- `src/core/gateway/routes/grpc/match_engine.rs`

---

## 2. 未匹配请求返回 UNIMPLEMENTED

### 现状

- `match_engine.rs:117` 对未匹配请求返回 `EdError::RouteNotFound()`
- 调用方可能返回通用 HTTP 404/503 错误
- Gateway API 规范要求：GRPCRoute 未匹配时应返回 gRPC status code 12 (UNIMPLEMENTED)

### 需要实现

1. 在 gRPC proxy 的错误处理中，将 `RouteNotFound` 映射为 gRPC UNIMPLEMENTED
2. 返回正确的 gRPC 错误帧：HTTP/2 200 + `grpc-status: 12` + `grpc-message: ...`
3. 确保错误响应符合 gRPC wire protocol

### 涉及文件

- `src/core/gateway/routes/grpc/match_engine.rs`
- `src/core/gateway/routes/grpc/integration.rs` — 错误响应处理

---

## 3. Hostname 唯一性（与 HTTPRoute 共享 listener）

### 现状

- GRPCRoute 有 `effective_hostnames` 和 hostname 匹配
- 但与 HTTPRoute 的 hostname 冲突检测是隐式的（分离的路由表）
- 规范要求同一 listener 上 GRPCRoute 和 HTTPRoute 的 hostname 具有唯一性

### 建议

- 在 Controller 的 hostname resolution 阶段增加跨 route 类型的 hostname 冲突检测
- 冲突时设置适当的 status condition

### 涉及文件

- `src/core/controller/sync_runtime/resource_processor/handlers/hostname_resolution.rs`
