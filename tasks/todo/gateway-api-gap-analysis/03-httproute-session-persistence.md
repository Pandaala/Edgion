# HTTPRoute Session Persistence 运行时实现

## 优先级: P1
## 工作量: 中（预计 3-5 天）
## 类型: 功能缺失

---

## 问题描述

HTTPRoute 和 GRPCRoute 的 Session Persistence（会话保持）数据模型已完整定义，但运行时完全未实现。
代码中 `session_persistence: None` 被硬编码在匹配引擎中，无法实际生效。

Gateway API 规范定义了 Cookie-based 和 Header-based 两种会话保持方式，属于 Experimental 级别特性。

## 当前状态

### 已有（数据模型）
- `SessionPersistence` 结构体：`session_name`, `absolute_timeout`, `idle_timeout`, `persistence_type`
- `SessionPersistenceType` 枚举：`Cookie`, `Header`
- `CookieConfig` 结构体：`lifetime_type` (Permanent/Session)
- `CookieLifetimeType` 枚举

### 缺失（运行时逻辑）
- 匹配引擎中 `session_persistence: None` 硬编码
  - `src/core/gateway/routes/grpc/match_engine.rs:394`
  - `src/core/gateway/routes/http/match_engine/regex_routes_engine.rs:181`
- 无 Cookie 注入逻辑（响应中设置 session cookie）
- 无 Cookie/Header 读取逻辑（请求中提取 session 标识）
- 无 backend 亲和性绑定（将 session 标识映射到特定后端）
- 无 session 超时管理（absolute_timeout / idle_timeout）

## 涉及文件

### 数据模型（已有）
1. `src/types/resources/http_route.rs` — SessionPersistence, CookieConfig 定义

### 需要修改
2. `src/core/gateway/routes/http/match_engine/regex_routes_engine.rs` — 解除 `session_persistence: None` 硬编码
3. `src/core/gateway/routes/grpc/match_engine.rs` — 解除 `session_persistence: None` 硬编码
4. `src/core/gateway/routes/http/proxy_http/pg_upstream_peer.rs` — 后端选择时考虑 session 亲和性
5. `src/core/gateway/routes/http/proxy_http/pg_upstream_response_filter.rs` — 响应中注入 session cookie/header

### 需要新增
6. Session 存储模块 — 维护 session_id → backend 映射（内存或 LRU cache）
7. Cookie/Header 解析工具 — 从请求中提取 session 标识

## 实现建议

### 方案 A：内存 Session 表（推荐）
1. 创建 `SessionAffinityStore`：`HashMap<String, (BackendAddr, Instant)>`，带 TTL 清理
2. 请求阶段：从 Cookie/Header 提取 session_id → 查找 store → 如存在且未过期，直接路由到绑定后端
3. 响应阶段：如果是新 session（或 session 不存在），将当前后端绑定到 session_id，通过 Set-Cookie / Response Header 返回
4. 超时管理：absolute_timeout 从创建时间算，idle_timeout 从最后访问时间算

### Cookie 模式实现要点
- Cookie name = `session_name` 字段值（默认 `gateway-api-session`）
- Cookie lifetime：Permanent → 设 Max-Age；Session → 不设 Max-Age（浏览器关闭即失效）
- Cookie 值：backend 标识的 hash 或加密值

### Header 模式实现要点
- Header name = `session_name` 字段值
- 响应中添加 header，请求中读取 header

### 注意事项
- Session store 应支持并发访问（Arc<RwLock> 或无锁结构）
- 需要考虑后端不可用时的 fallback（session 绑定的后端下线，需要重新选择）
- 多 Gateway 实例间 session 不共享（内存方案限制），如需共享可考虑 Redis（已有 LinkSys）

## 参考 Skills

- `skills/01-architecture/02-gateway/12-edgion-plugin-dev.md` — 插件开发模式（session 逻辑可作为内置模块）
- `skills/01-architecture/02-gateway/SKILL.md` — Gateway 数据面架构
- `skills/03-coding/observability/00-access-log.md` — 日志中记录 session 信息
- `skills/05-testing/00-integration-testing.md` — 集成测试

## 验证标准

- Cookie-based session persistence：同一 session cookie 的请求路由到同一后端
- Header-based session persistence：同一 header 值的请求路由到同一后端
- absolute_timeout 过期后 session 失效，重新选择后端
- idle_timeout 过期后 session 失效
- 后端下线后 session 自动重绑定
- `cargo check --all-targets` 通过
- `cargo clippy --all-targets` 通过
- 集成测试覆盖 Cookie 和 Header 两种模式
