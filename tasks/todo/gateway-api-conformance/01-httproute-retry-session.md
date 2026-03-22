# HTTPRoute Retry 策略与 Session Persistence

优先级: P0

## 1. Retry 策略运行时接入

### 现状

- `HTTPRouteRetry` 结构体已定义（`src/types/resources/http_route.rs`），包含 `attempts`、`backoff`、`codes` 字段
- 运行时完全不使用该结构体
- 实际重试逻辑在 `pg_fail_to_connect.rs` 中读取 annotation `edgion.io/max-retries` 或全局 `max_retries` 配置
- 仅在连接失败时触发重试，不支持基于 HTTP 状态码的重试
- 无退避延迟，Pingora 立即重试
- `pg_error_while_proxy.rs` 有注释 `// todo need add retry logic?`

### 需要实现

1. **读取 route 级别 retry 配置**: 在 `pg_fail_to_connect` 和 `pg_error_while_proxy` 中优先使用 `rule.retry` 的值，annotation 作为向后兼容降级
2. **状态码重试**: 在 `upstream_response_filter` 阶段检查响应状态码是否匹配 `retry.codes`，若匹配则触发重试
3. **退避延迟**: 实现指数退避，参考 `backoff` 字段配置
4. **重试计数**: 使用 `retry.attempts` 而非 annotation 值
5. **与 request timeout 协调**: 重试不应超过 end-to-end request timeout

### 涉及文件

- `src/types/resources/http_route.rs` — 类型定义（已有）
- `src/core/gateway/routes/http/proxy_http/pg_fail_to_connect.rs` — 连接失败重试
- `src/core/gateway/routes/http/proxy_http/pg_upstream_peer.rs` — 超时检查
- `src/core/gateway/routes/http/proxy_http/pg_response_filter.rs` — 状态码重试触发点

---

## 2. Session Persistence 实现

### 现状

- `SessionPersistence` 结构体已定义（`src/types/resources/http_route.rs:577-608`）
- 支持 `Cookie` 和 `Header` 类型
- `cookieConfig.lifetimeType` 支持 `Permanent` / `Session`
- `absoluteTimeout` 和 `idleTimeout` 已定义
- **运行时零消费** — grep `session_persistence` 在 gateway core 中无命中
- **缺少 `SourceIP` 类型**（Gateway API Extended 要求）

### 需要实现

1. **添加 `SourceIP` 持久化类型到枚举**
2. **Cookie-based 持久化**:
   - 首次请求时向响应注入 affinity cookie（包含选中的 backend 标识）
   - 后续请求读取 cookie 并路由到同一 backend
   - 支持 Session（浏览器关闭过期）和 Permanent（带 Max-Age）两种生命周期
   - 实现 `absoluteTimeout` 和 `idleTimeout`
3. **Header-based 持久化**: 基于指定请求头值做 backend 亲和
4. **SourceIP 持久化**: 基于客户端 IP 做 backend 亲和（需要一致性哈希或映射表）
5. **集成到 backend 选择流程**: 在 `pg_upstream_peer` 中 `BackendSelector` 选择之前检查 session affinity

### 涉及文件

- `src/types/resources/http_route.rs` — 添加 SourceIP 类型
- `src/core/gateway/routes/http/proxy_http/pg_upstream_peer.rs` — backend 选择集成
- `src/core/gateway/routes/http/proxy_http/pg_response_filter.rs` — cookie 注入
- `src/core/gateway/lb/` — 与负载均衡器协调
