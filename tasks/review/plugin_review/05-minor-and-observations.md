# Minor & Observation Issues (Supplementary)

> 此文件收录前四份报告中未完全展开的 Minor 和 Observation 级别问题。

---

## 认证插件

### AUTH-OBS-01 OIDC state 参数非恒定时间比较

**模块**: `src/core/gateway/plugins/http/openid_connect/openid_impl.rs:1695`

`state_cookie.state != state_param` 使用标准字符串比较。state 是随机短寿命值且加密在 AES-GCM cookie 中，实际风险极低。如需 defense-in-depth 可用 `subtle::ConstantTimeEq`。

### AUTH-OBS-02 jwt_auth claims_to_headers 大小限制被禁用

**模块**: `src/core/gateway/plugins/http/jwt_auth/plugin.rs:447-449`

`set_claims_headers` 传入 `usize::MAX` 禁用了 per-header 和 total-header 大小限制。恶意 JWT issuer 可通过大 claims 使上游返回 431。建议使用合理默认值（如 OIDC 插件的 4096/16384）。

### AUTH-OBS-03 forward_auth client_headers 未检查控制字符

**模块**: `src/core/gateway/plugins/http/forward_auth/plugin.rs:336-342`

认证服务响应头中的 `client_headers` 被复制到客户端响应时未检查 CRLF。`value.to_str()` 会拒绝非可见 ASCII，但建议添加与其他插件一致的显式检查。

### AUTH-OBS-04 OIDC X-Forwarded-Proto 未验证

**模块**: `src/core/gateway/plugins/http/openid_connect/openid_impl.rs:127-135`

`request_scheme` 从 `X-Forwarded-Proto` 读取 scheme 构造 redirect URI。如果未被可信代理清理，攻击者可将 scheme 降级为 HTTP。建议添加 scheme 验证或配置强制 HTTPS。

### AUTH-OBS-05 ldap_auth 用户名字符限制过严

**模块**: `src/core/gateway/plugins/http/ldap_auth/plugin.rs:153-171`

用户名只允许 `alphanumeric`, `.`, `@`, `-`, `_`，阻止了含空格、重音字符或 CJK 字符的合法 LDAP 用户名。建议文档说明或允许可配置字符集。

### AUTH-OBS-06 auth_failure_delay_ms 默认为 0

所有认证插件默认无延迟。开箱部署无暴力破解保护。建议对 BasicAuth/LdapAuth 默认非零（如 200ms）或当无 rate-limit 插件时输出 warning。

---

## 转换/路由插件

### TRANS-OBS-01 proxy_rewrite 未解析的 $variable 泄露到上游 URI

**模块**: `src/core/gateway/plugins/http/proxy_rewrite/plugin.rs:123-129`

路径参数未找到时 `$name` 字面量留在 URI 中（有单测确认为有意行为）。可能泄露网关配置信息。建议记录 warning。

### TRANS-OBS-02 response_rewrite rename 是顺序执行，可能产生级联效果

**模块**: `src/core/gateway/plugins/http/response_rewrite/plugin.rs:52-65`

`rename: [{from: "A", to: "B"}, {from: "B", to: "C"}]` 中第二条会作用于第一条的结果。用户可能预期基于原始状态执行。建议文档说明。

### TRANS-OBS-03 dynamic_internal_upstream 直接模式下 namespace 匹配不对称

**模块**: `src/core/gateway/plugins/http/dynamic_internal_upstream/plugin.rs:89, 127-133`

直接模式 `target_namespace = None` 会匹配任意 namespace 的同名 backend_ref。建议文档说明或支持 `"namespace/name"` 格式。

### TRANS-OBS-04 direct_endpoint debug header 用 response header 而非 request header

**模块**: `src/core/gateway/plugins/http/direct_endpoint/plugin.rs:143-144`

与 DynamicExternalUpstream/DynamicInternalUpstream 使用 request header 不一致。

---

## DSL 引擎

### DSL-OBS-01 `||` 短路返回 `true` 而非原始左操作数

**模块**: `src/core/gateway/plugins/http/dsl/lang/compiler.rs:624-633`

`"hello" || "world"` 返回 `true` 而非 `"hello"`。这与 JavaScript/Lua 语义不同，但对 boolean 条件上下文是正确的。建议文档说明。

### DSL-OBS-02 反序列化字节码无栈平衡验证

**模块**: `src/core/gateway/plugins/http/dsl/plugin.rs:42-57`

Validator 检查结构完整性但不验证栈效果一致性。VM 通过 RuntimeError 处理栈下溢所以不会 crash。

### DSL-OBS-03 VmLimits 允许高上限

**模块**: `src/core/gateway/plugins/http/dsl/config.rs:133-162`

`max_steps` 允许到 1M，`max_string_len` 允许到 1MB。默认值合理 (10K/8KB)，但运维设置高值时可能影响性能。建议文档说明性能影响。

### DSL-OBS-04 split() 无元素数量限制

**模块**: `src/core/gateway/plugins/http/dsl/lang/builtins.rs:413-418`

8KB 字符串按单字符分割可产生 ~8192 个 String 分配。建议参考 `range()` 的 1000 上限使用 `splitn`。

---

## Stream 插件

### STREAM-OBS-01 connection_filter_bridge 引用资源未找到时静默放行

**模块**: `src/core/gateway/plugins/stream/connection_filter_bridge.rs:68-88`

store 中找不到引用的资源时返回 `true`（允许）。注解拼写错误会导致静默放行。建议输出 `tracing::warn!`。

### STREAM-OBS-02 stream_plugin_store update 非 CAS

**模块**: `src/core/gateway/plugins/stream/stream_plugin_store.rs:70-85`

load-clone-modify-store 模式非原子。当前调用方是串行的所以安全，但 `pub` 方法可能被并发调用。建议使用 `ArcSwap::rcu()`。

### STREAM-OBS-03 TLS proxy backward compat fallback 可能导致双重执行

**模块**: `src/core/gateway/routes/tls/proxy.rs:374-441`

Stage 2 为空时退回 Stage 1 runtime。如果 Stage 1 (ConnectionFilter) 在 listener 级别也活跃，同一 IP 限制检查可能执行两次。建议文档说明并在 fallback 时输出 log。

---

## 插件生命周期

### LIFE-OBS-01 create_upstream_response_from_edgion 始终返回 None

**模块**: `src/core/gateway/plugins/runtime/plugin_runtime.rs:418-424`

当前无插件实现此阶段。用户在 `upstream_response_plugins` 中配置的插件被静默忽略。建议配置此阶段的条目时输出 warning。

### LIFE-OBS-02 Secret 解析只扫描 request_plugins

**模块**: `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/edgion_plugins.rs`

七个 `resolve_*` 方法只遍历 `ep.spec.request_plugins`。当前所有 Secret 依赖的插件都是 request 阶段的，所以正确。但如果未来 Secret 依赖的插件出现在其他阶段，会静默缺少解析。

---

## 杂项

### MISC-OBS-01 DefaultHasher 跨 Rust 版本不稳定

**模块**: `src/core/gateway/plugins/runtime/conditions/evaluator.rs:290-293`

ProbabilityCondition 使用 `DefaultHasher` 做确定性采样。Rust 不保证其算法跨版本稳定。实际影响极低，但违反"确定性"契约。

### MISC-OBS-02 session_adapter append_request_header 用逗号拼接

**模块**: `src/core/gateway/plugins/runtime/session_adapter.rs:346-354`

Multi-value header 被逗号拼接成单个值。RFC 7230 对多数 header 语义等价，但 Pingora 有 `append_header` 更惯用。

### MISC-OBS-03 session_adapter get_query_param 不做 URL decode

**模块**: `src/core/gateway/plugins/runtime/session_adapter.rs:164-178`

`?name=hello%20world` 返回 `"hello%20world"` 而非 `"hello world"`。这是常见代理行为，但应文档说明。

### MISC-OBS-04 CORS preflight 拒绝时缺少 Vary: Origin

**模块**: `src/core/gateway/plugins/runtime/gateway_api_filters/cors_filter.rs:231-239`

非匹配 origin 的 preflight 返回裸 204 无 `Vary: Origin`。中间缓存代理可能为不同 origin 缓存此响应。

### MISC-OBS-05 CSRF token expires 字段命名误导

**模块**: `src/core/gateway/plugins/http/csrf/token.rs:9-14`

`expires` 实际存储创建时间戳而非过期时间。建议重命名为 `created_at` 或 `timestamp`（序列化 breaking change，可在 major version 时处理）。

### MISC-OBS-06 rate_limit 自定义 header name 未验证

**模块**: rate_limit / rate_limit_redis 配置

`LimitHeaderNames` 中的自定义 header name 未按 RFC 7230 token 格式验证。无效名称会被 Pingora 静默丢弃。

### MISC-OBS-07 jwe_decrypt cached key 不会失效

**模块**: `src/core/gateway/plugins/http/jwe_decrypt/plugin.rs:166-181`

一旦加载 key 永不刷新。Secret rotation 需要网关重启或配置变更触发插件重建。

### MISC-OBS-08 request_mirror TLS 基于 port 443 启发式判断

**模块**: `src/core/gateway/plugins/http/request_mirror/plugin.rs:124`

`if port == 443 { "https" } else { "http" }`。非 443 端口的 TLS 服务会被以明文联系。已有 TODO 标记。

### MISC-OBS-09 debug_access_log 无生产环境保护

**模块**: `src/types/resources/edgion_plugins/plugin_configs/debug_access_log.rs`

空配置结构体，无字段控制信息范围或环境限制。生产环境误启用会通过响应头泄露内部信息。
