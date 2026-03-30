# CPU 热点问题

> 按严重度排列。每个问题包含文件位置、描述、修复建议。

---

## Critical

### CPU-01: 条件求值器回退路径每请求编译 Regex (3 处)

**文件**: `src/core/gateway/plugins/runtime/conditions/evaluator.rs`
**行号**: 231-239 (KeyMatchCondition), 363-371 (IncludeCondition), 416-425 (ExcludeCondition)

**问题**: 当 `compiled_regex` 为 `None` 时（即 `compile()` 未被调用），求值器回退到在每个请求中调用 `Regex::new()` 编译正则。Regex 编译开销在微秒到毫秒级。三处代码完全相同的回退逻辑。

**触发条件**: `pre_parse` 被跳过，或 `compile()` 静默失败时触发。

**修复建议**: 移除运行时回退编译，在配置加载时强制编译。若编译失败应在配置验证阶段拒绝。或使用 `OnceLock` 做一次性懒编译。

---

## High

### CPU-02: ctx_set 插件每请求编译 Regex

**文件**: `src/core/gateway/plugins/http/ctx_set/plugin.rs`
**行号**: 214

**问题**: `apply_transform()` 在处理 `regex_replace` 类型时，每个请求调用 `Regex::new(&pattern)`。其他插件（如 `request_restriction`）已在验证时预编译，但 `ctx_set` 没有。

**修复建议**: 在 `pre_parse()` 阶段预编译 Regex 并存储在配置结构体中。

---

### CPU-03: DSL 内置函数每次调用编译 Regex

**文件**: `src/core/gateway/plugins/http/dsl/lang/builtins.rs`
**行号**: 383-384 (RegexFind), 403-404 (RegexReplace)

**问题**: `RegexFind` 和 `RegexReplace` 内置函数每次调用时从用户模式字符串编译新 Regex。虽然有 pattern 长度限制 (1024 bytes) 和 VM call budget，但在循环中（最多 100 次迭代）可能编译 100 次。

**修复建议**: 在 VM 运行时添加 LRU Regex 缓存，或在 VM 编译阶段静态提取并预编译 Regex 常量。

---

### CPU-04: `lookup_gateway()` 全量 JSON 序列化→反序列化→线性扫描

**文件**: `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/route_utils.rs`
**行号**: 421-430

**问题**: `lookup_gateway()` 调用 `processor.as_watch_obj().list_json()` 将所有 Gateway 对象序列化为 JSON 字符串，再反序列化回来，然后线性扫描匹配 namespace/name。该函数在每个 Route 处理中被调用 3-4 次（per parentRef）：
- `validate_parent_ref_accepted()` (update_status 中每个 parentRef 一次)
- `resolve_listener_keys()` (route parse 中每个 parentRef 一次)
- `resolve_effective_hostnames()` (hostname 解析中每个 parentRef 一次)
- `update_tracker_accepted_only()` (update_status 中每个 parentRef 一次)

对于有 P 个 parentRef 的 Route，一次处理就要做 3-4P 次全量序列化。100 个 Gateway + 1000 个 Route（各 2 个 parentRef）= 4000-8000 次 JSON round-trip。

**修复建议**: 为 `ProcessorObj` trait 添加 `get_by_key()` 方法，直接从缓存查询单个对象，避免 JSON 序列化。

---

### CPU-05: `lookup_service_cluster_ip()` 同样模式的全量 JSON round-trip

**文件**: `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/gateway.rs`
**行号**: 816-832

**问题**: 与 CPU-04 相同模式，序列化所有 Service 对象再扫描。在有数千 Service 的集群中非常昂贵。

**修复建议**: 同 CPU-04，添加直接缓存查询。

---

## Medium

### CPU-06: `extract_status_json()` 双重序列化

**文件**: `src/core/controller/conf_mgr/sync_runtime/resource_processor/processor.rs`
**行号**: 67-78

**问题**: `extract_status_json()` 先 `serde_json::to_value(obj)` 序列化整个对象为 Value 树，再 `.get("status")`，再 `serde_json::to_string(status)`。在 `process_resource_inner` 中被调用两次（494 行和 513 行）：获取旧 status 和新 status。每个资源处理 4 次完整序列化。

**修复建议**: 仅序列化 status 字段，或使用 hash 比较检测 status 是否变化。

---

### CPU-07: `update_tracker_accepted_only()` 冗余调用 `lookup_gateway()`

**文件**: `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/route_utils.rs`
**行号**: 343-380

**问题**: `update_tracker_accepted_only()` 内部再次调用 `validate_parent_ref_accepted()`，后者又调用 `lookup_gateway()`。而 `update_status()` 已经直接调用过 `validate_parent_ref_accepted()`。每个 parentRef 的 `lookup_gateway()` 被执行至少两次。

**修复建议**: 将 acceptance 结果缓存传递给 `update_tracker_accepted_only()`，避免重复计算。

---

### CPU-08: `process_resource_inner` 全量 Clone 处理对象

**文件**: `src/core/controller/conf_mgr/sync_runtime/resource_processor/processor.rs`
**行号**: 562

**问题**: `let obj_for_result = parsed_obj.clone()` 深拷贝整个资源对象。对于大资源（多 listener 的 Gateway、多 rule 的 HTTPRoute），这是不必要的分配。

**修复建议**: 在 save 前提取 status 值，避免全量克隆。

---

### CPU-09: 每请求 Metrics 标签 String 分配

**文件**: `src/core/gateway/observe/metrics/registry.rs`
**行号**: 259-284 (`record_backend_request`), 338-364 (`record_request_duration`)

**问题**: 每请求调用 `record_backend_request()` 时构造 10 个 label 维度，每个 `.to_string()` 堆分配。`record_request_duration()` 同样创建 `Vec::with_capacity(8)` 并 push 8 个 `(str, String)` 元组。

**修复建议**: 预分配 label buffer 复用，或使用 `&str` 引用替代 `String`。

---

### CPU-10: 全局插件每请求 `format!()` + DashMap lookup

**文件**: `src/core/gateway/routes/http/proxy_http/pg_request_filter.rs`
**行号**: 164-185

**问题**: 每请求循环 `global_plugins_ref`，为每个全局插件 `format!("{}/{}", ns, name)` 构造 key，再做 DashMap lookup。N 个全局插件 = N 次 `format!()` + N 次 DashMap 查找。

**修复建议**: 在配置重载时预解析全局插件为直接 `Arc<EdgionPlugins>` 引用。

---

### CPU-11: 每请求 `format!()` 分配 (多处热路径)

**文件**:
- `src/core/gateway/routes/http/proxy_http/pg_request_filter.rs:167-170`
- `src/core/gateway/routes/http/proxy_http/pg_upstream_peer.rs:362`
- `src/core/gateway/routes/http/proxy_http/pg_upstream_response_body_filter.rs:50-53`

**问题**: 多个热路径中 `format!("{}/{}", namespace, name)` 构造 plugin/service key。在 body_filter 中甚至每个 body chunk 都执行。

**修复建议**: 在配置加载时预计算 key 字符串。

---

### CPU-12: 每请求 Access Log JSON 序列化

**文件**: `src/core/gateway/observe/access_log/entry.rs:87-92`
**文件**: `src/core/gateway/routes/http/proxy_http/pg_logging.rs:54`

**问题**: 每请求将整个 `AccessLogEntry`（含嵌套结构、ctx_map）序列化为 JSON。`from_context()` 还对 ctx_map 做 denylist 过滤，每个 key `.to_lowercase().contains()` 扫描，O(K*D)。

**修复建议**: 使用预分配 buffer 复用或更轻量的日志格式。

---

### CPU-13: `match_header` / `match_query_param` 回退 Regex 编译

**文件**: `src/core/gateway/routes/http/match_unit.rs`
**行号**: 270-274 (header), 305-309 (query param)

**问题**: 当 `compiled_regex` 查找返回 `None` 时，回退到每请求 `Regex::new()`。正常情况下 `compile_match_regexes()` 已预编译，但索引不匹配时会触发回退。

**修复建议**: 移除运行时回退，确保编译阶段覆盖所有 case。

---

### CPU-14: DNS 无缓存每请求解析

**文件**: `src/core/gateway/routes/http/proxy_http/pg_upstream_peer.rs`
**行号**: 507-530

**问题**: `resolve_domain()` 每次调用 `tokio::net::lookup_host()`，无缓存。对外部后端高流量时可打满 OS resolver。

**修复建议**: 引入 DNS 缓存层（如 trust-dns 或自建 TTL 缓存）。

---

### CPU-15: `ResourceKind::from_content()` 每次调用编译 Regex

**文件**: `src/types/resource/kind.rs`
**行号**: 114-130

**问题**: `from_content()` 每次调用时用 `Regex::new()` 编译 JSON 和 YAML 两个模式。虽然不在 HTTP 请求热路径上，但在 gRPC 配置同步和文件 watcher 路径中每个资源事件都调用。

**修复建议**: 使用 `LazyLock<Regex>` 静态编译。

---

### CPU-16: Hostname 双重反转

**文件**: `src/core/common/matcher/host_match/radix_match/radix_host_match.rs:67,107`
**文件**: `src/core/common/matcher/host_match/radix_match/radix_host.rs:109-118`

**问题**: `match_host()` 在第 67 行反转 hostname 用于 Radix 树查找，然后 `radix_host.matches()` 在内部（131 行）再次反转同一 hostname。每次调用 `reverse_hostname()` 分配 `Vec<&str>` + `String`。每请求执行两次。

**修复建议**: 传入已反转的 hostname，或添加 `matches_reversed()` 方法。

---

### CPU-17: `Rsc.kind` 从静态字面量分配 String

**文件**: `src/types/ctx.rs:23-27, 54-58, 618-622`

**问题**: `Rsc` 结构体的 `kind: String` 字段始终为常量值（如 `"HTTPRoute"`、`"Service"`），但每请求从字面量 `String::from("...")` 堆分配。每请求至少 2 次。

**修复建议**: 改为 `&'static str` 或 `Cow<'static, str>`。

---

### CPU-18: gRPC 压缩事件处理 100ms 轮询循环

**文件**: `src/core/gateway/conf_sync/cache_client/cache_data.rs:103-117`

**问题**: 每种资源类型（约 17 种）各 spawn 一个 tokio task，每 100ms 获取 write lock 检查事件。空闲时约 170 次/秒无意义的锁获取。

**修复建议**: 改为 `Notify`-based 唤醒，有事件时才处理。

---

## Low

### CPU-19: UUID v4 每请求生成 Trace ID

**文件**: `src/core/gateway/routes/http/proxy_http/pg_request_filter.rs:297-305`

**问题**: 无 `x-trace-id` 头时每请求 `uuid::Uuid::new_v4().to_string()`（含 CSPRNG + 格式化）。

---

### CPU-20: `HTTPBackendRef` 非 Arc clone

**文件**: `src/core/gateway/routes/http/proxy_http/pg_response_filter.rs:40`
**文件**: `src/core/gateway/routes/http/proxy_http/pg_upstream_response_filter.rs:122`

**问题**: `ctx.selected_backend.clone()` 包含 String/Vec 字段，每请求在 response 阶段至少 clone 两次。

---

### CPU-21: `key_name()` 每次调用分配 String

**文件**: `src/types/resource/meta/traits.rs:77-83`

**问题**: `format!("{}/{}", namespace, name)` 在紧凑循环中频繁调用。

---

### CPU-22: `GatewayBaseConf::add_gateway()` O(n) 线性扫描

**文件**: `src/types/gateway_base_conf.rs:76-89`

**问题**: 更新已存在 Gateway 时，`self.gateways.iter_mut().find()` 线性扫描，每个 `g.key_name()` 调用分配 String。

---

### CPU-23: `HashHost::get()` 冗余的 `is_valid_domain()` 检查

**文件**: `src/core/common/matcher/host_match/hash_match/hash_host.rs:65-88`

**问题**: hostname 已被 `NormalizedHostname::new()` 规范化，`get()` 再做一次全域名字节扫描是多余的。

---

### CPU-24: `all_resource_type_names()` 每次调用分配 Vec

**文件**: `src/types/resource/registry.rs:77-98`

**问题**: 资源定义是静态的，但函数每次都 collect 新 Vec。

---

### CPU-25: Watch 事件双重 JSON 解析 + Value 深拷贝

**文件**: `src/core/gateway/conf_sync/cache_client/event_dispatch.rs:299-316`

**问题**: Watch 事件先 parse 为 `Vec<serde_json::Value>`，再 `.clone()` 每个 Value 后反序列化为具体类型。大资源（如 EndpointSlice）的 Value clone 开销大。

---

### CPU-26: LDAP 每请求创建 TCP 连接（缓存 miss 时）

**文件**: `src/core/gateway/plugins/http/ldap_auth/plugin.rs:181`

**问题**: 无连接池，每次缓存 miss 建立新 LDAP TCP 连接。暴力破解攻击可打满文件描述符。
