# Validation & Performance Issues

## 性能问题

### PERF-01 [Important] Condition regex fallback 每请求重新编译

**模块**: `src/core/gateway/plugins/runtime/conditions/evaluator.rs:232-239, 364-371, 415-426`

**问题**: 当 `compile()` 未被调用时（`compiled_regex` 为 `None`），evaluator 退回到每请求调用 `Regex::new()` 编译正则。`Regex::new()` 开销可达微秒到毫秒级。

影响路径：`KeyMatchCondition::match_value()`, `IncludeCondition::matches_value()`, `ExcludeCondition::matches_value()`

**影响**: 热路径上 O(1) regex match 退化为 O(n) regex compilation + match。

**修复建议**: 删除 fallback（强制 `compile()` 在构造时调用）或使用 `OnceLock` 缓存。至少在首次 fallback 时输出 `tracing::warn!`。

---

### PERF-02 [Important] 用户正则无数量/长度限制

**模块**: `src/core/gateway/plugins/runtime/conditions/types.rs:392-409, 421-439, 445-464`

**问题**: `compile()` 方法将用户提供的正则模式用 `|` 拼接后调用 `Regex::new()`。无正则数量或总长度限制。`regex` crate 避免了指数回溯，但超大模式的编译时间和内存仍可能过高。

**影响**: 恶意或错误配置可导致条件编译时资源耗尽。

**修复建议**: 添加限制：每条件最多 20 个正则、拼接后总长度不超过 4096 字节。在 `compile()` 或反序列化时验证。

---

### PERF-03 [Important] DSL regex_find/regex_replace 每次调用编译正则

**模块**: `src/core/gateway/plugins/http/dsl/lang/builtins.rs:375-408`

**问题**: `regex_find` 和 `regex_replace` 接受运行时参数作为正则模式，每次调用 `Regex::new(&pattern)` 编译。结合调用预算 (500) 和循环限制 (100)，单次请求可触发 500 次正则编译。

**影响**: 可测量的 CPU 消耗，特别在对抗性脚本下。

**修复建议**: 在 VmState 上缓存已编译的正则（小 LRU 或 HashMap，按 pattern 字符串为 key）。同时考虑使用 `RegexBuilder::new().size_limit(...)` 限制自动机大小。

---

### PERF-04 [Minor] IncludeCondition 双重检查 values

**模块**: `src/core/gateway/plugins/runtime/conditions/evaluator.rs:339-354`

**问题**: `matches_value()` 先检查 `values_set`（HashSet）再遍历所有 `values`。当 HashSet 存在时，精确匹配值在 HashSet O(1) 查找后又在 values O(n) 遍历中被重复检查。

**修复建议**: 当 `values_set` 存在时，跳过 values 遍历中的非通配符 entry。

---

## 配置验证缺失

### VAL-01 [Minor] CSRF key 无最小长度要求

**模块**: `src/types/resources/edgion_plugins/plugin_configs/csrf.rs:64-68`

**问题**: CSRF 配置只检查 key 非空。1 字符的 key (256 种可能) 使暴力伪造 token 可行。

**修复建议**: 要求最小 key 长度 16 或 32 字符。

---

### VAL-02 [Minor] CSRF 未将 OPTIONS 列为安全方法

**模块**: `src/core/gateway/plugins/http/csrf/plugin.rs:23`

**问题**: `SAFE_METHODS` 为 `["GET", "HEAD"]`，不含 `OPTIONS`。如果 CSRF 在 CORS 之前执行，浏览器的 preflight OPTIONS 请求会被 CSRF 拒绝。

**修复建议**: 将 `"OPTIONS"` 加入 `SAFE_METHODS`（Django、Rails、Spring Security 均排除 OPTIONS）。

---

### VAL-03 [Minor] proxy_rewrite Host 冲突检查遗漏 headers.add

**模块**: `src/types/resources/edgion_plugins/plugin_configs/proxy_rewrite.rs:165-178`

**问题**: `precompile()` 检测 `host` 和 `headers.set` 中的 Host 冲突，但未检查 `headers.add`。将 Host 放入 `add` 会追加第二个 Host header（格式错误的 HTTP）。

**修复建议**: 将冲突检查扩展到 `headers.add`（大小写不敏感）。

---

### VAL-04 [Minor] proxy_rewrite / response_rewrite header name 未验证

**模块**:
- `src/types/resources/edgion_plugins/plugin_configs/proxy_rewrite.rs:129-157`
- `src/types/resources/edgion_plugins/plugin_configs/response_rewrite.rs:115-159`

**问题**: Header name 允许空字符串、含空格/冒号/控制字符的字符串。运行时 Pingora 的 `insert_header` 会拒绝无效名称，但应在配置时验证提供更好反馈。

**修复建议**: 按 RFC 7230 token 格式验证 header name：`^[a-zA-Z0-9][a-zA-Z0-9-]*$`。

---

### VAL-05 [Minor] dynamic_external_upstream override_host/sni 未验证

**模块**: `src/types/resources/edgion_plugins/plugin_configs/dynamic_external_upstream.rs:111-126`

**问题**: `override_host` 和 `sni` 接受任意字符串，包括空串、含空格/null 的字符串。

**修复建议**: 拒绝空值（显式设置时）、含空格/null/控制字符的值。

---

### VAL-06 [Minor] DirectEndpointConfig port 0 未被拒绝

**模块**: `src/types/resources/edgion_plugins/plugin_configs/direct_endpoint.rs:143-154`

**问题**: Port 0 对 TCP 无效但未在 `validate()` 中检查。

**修复建议**: 添加 `if self.port == Some(0) { ... }` 验证。

---

### VAL-07 [Minor] mock validate() 在构造函数中未被调用

**模块**: `src/core/gateway/plugins/http/mock/plugin.rs:26-31`

**问题**: `Mock::new()` 直接 clone 配置不调用 `validate()`。无效 status code（如 0 或 999）可能在运行时导致异常。

**修复建议**: 在构造函数中调用 `validate()`。

---

### VAL-08 [Minor] TimeRangeCondition 对无效 RFC3339 时间戳静默忽略

**模块**: `src/core/gateway/plugins/runtime/conditions/evaluator.rs:247-269`

**问题**: 时间戳解析用 `if let Ok(...)`，格式错误时静默跳过该边界，使条件变为单边无界。

**修复建议**: 在配置加载时验证时间戳格式，而非运行时。

---

### VAL-09 [Minor] auth_failure_delay_ms / jwe auth_failure_delay_ms 无上限

**模块**:
- 所有 auth 插件配置（默认为 0）
- `src/types/resources/edgion_plugins/plugin_configs/jwe_decrypt.rs:119`

**问题**: `auth_failure_delay_ms` 为 `u64` 无上限验证。极大值会长时间阻塞 Tokio task。

**修复建议**: 限制上限（如 5000ms）。

---

### VAL-10 [Minor] CORS allow_origins_by_regex 未与 allow_credentials 联合验证

**模块**: `src/types/resources/edgion_plugins/plugin_configs/cors.rs:131-153`

**问题**: 验证阻止 `allow_credentials: true` 与 `allow_origins: "*"` 组合，但 `allow_origins_by_regex` 中功能等价的宽泛模式（如 `^https://.*$`）不受此检查。

**修复建议**: 当 `allow_credentials: true` 与 `allow_origins_by_regex` 同时使用时输出 warning。

---

### VAL-11 [Minor] real_ip real_ip_header 未验证 header name 格式

**模块**: `src/types/resources/edgion_plugins/plugin_configs/real_ip.rs:115-116`

**问题**: 只检查非空和非纯空格，不验证是否为有效 HTTP header name。

---

### VAL-12 [Observation] Controller edgion_stream_plugins handler parse 中无验证

**模块**: `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/edgion_stream_plugins.rs:31-33`

**问题**: Controller handler 的 `parse` 方法不验证 `EdgionStreamPlugins` 资源，无条件返回 `ProcessResult::Continue`。无效配置被接受并同步到 Gateway。

**修复建议**: 在 `parse` 中对每个 plugin config 调用 `validate_and_init()`，收集错误并暴露到 K8s status。

---

### VAL-13 [Observation] hmac_auth min_secret_length 配置未在运行时执行

**模块**: `src/types/resources/edgion_plugins/plugin_configs/hmac_auth.rs:103-106`

**问题**: `min_secret_length` 文档描述会在凭证解析时触发警告，但运行时插件未检查 secret 长度。

**修复建议**: 在插件创建或验证时检查 `credential.secret.len() >= config.min_secret_length`。

---

### VAL-14 [Observation] bandwidth_limit 无效 rate 时 fail-open

**模块**: `src/core/gateway/plugins/http/bandwidth_limit/plugin.rs:41-42, 63-64`

**问题**: rate 字符串无法解析时 `rate_bps` 设为 0，插件变为 no-op，静默禁用带宽限制。

**修复建议**: 在插件创建时验证 rate 字符串并返回错误。
