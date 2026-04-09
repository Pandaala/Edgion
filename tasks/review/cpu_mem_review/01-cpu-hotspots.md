# CPU 热点问题

> 按严重度排列。每个问题包含文件位置、描述、修复建议。
> **深度复核标注**：每项末尾标注复核结论，含 [确认] / [降级] / [误判] 标记。

---

## Critical

### CPU-01: 条件求值器回退路径每请求编译 Regex (3 处)

**文件**: `src/core/gateway/plugins/runtime/conditions/evaluator.rs`
**行号**: 231-239 (KeyMatchCondition), 363-371 (IncludeCondition), 416-425 (ExcludeCondition)

**问题**: 当 `compiled_regex` 为 `None` 时（即 `compile()` 未被调用），求值器回退到在每个请求中调用 `Regex::new()` 编译正则。Regex 编译开销在微秒到毫秒级。三处代码完全相同的回退逻辑。

**触发条件**: `pre_parse` 被跳过，或 `compile()` 静默失败时触发。

**修复建议**: 移除运行时回退编译，在配置加载时强制编译。若编译失败应在配置验证阶段拒绝。或使用 `OnceLock` 做一次性懒编译。

---

**[深度复核结论] ✅ 确认为 Critical Bug，已修复**

**修复说明**（2026-04-09）：
- `conditions/types.rs`：新增 `Condition::compile()`（转发给 KeyMatch/Include/Exclude）、`PluginConditions::compile()`（遍历 skip/run 所有条件）
- `edgion_plugins/mod.rs`：`preparse()` 开头调 `compile_conditions()`，在 `add_from_*` 之前一次性编译所有 entry 的 conditions regex；`add_from_*` clone 到的 conditions 已是编译后状态，fallback 路径不再触发

**代码确认**：
```rust
// evaluator.rs 231-239（KeyMatchCondition，另两处完全相同）
if let Some(patterns) = &self.regex {
    if self.compiled_regex.is_none() && !patterns.is_empty() {
        let combined = patterns.join("|");
        if let Ok(re) = regex::Regex::new(&combined) {
            return re.is_match(value);
        }
    }
}
```

```rust
// types.rs 189-192
#[serde(skip)]
#[schemars(skip)]
pub compiled_regex: Option<regex::Regex>,
```

**根本原因**：`compiled_regex` 被 `#[serde(skip)]` 标记，反序列化后永远是 `None`。生产代码路径（`plugin_runtime.rs` 中 `add_from_request_filters` 等）通过 `entry.conditions.clone()` 直接使用反序列化的 `PluginConditions`，**整个代码库（除单元测试外）从未调用 `.compile()`**。这意味着所有通过配置文件加载的含 regex 条件的插件，每次请求都会重新编译 Regex。

**修复方案（推荐）**：在构建 `ConditionalFilter` 前，先调用 compile 预处理所有条件：

```rust
// plugin_runtime.rs 中，构建 ConditionalFilter 时
fn compile_conditions(conditions: Option<PluginConditions>) -> Option<PluginConditions> {
    conditions.map(|mut conds| {
        for condition in conds.skip.iter_mut().flatten() {
            condition.compile_mut();
        }
        for condition in conds.run.iter_mut().flatten() {
            condition.compile_mut();
        }
        conds
    })
}
```

为 `Condition` enum 添加 `compile_mut` 方法：
```rust
impl Condition {
    pub fn compile_mut(&mut self) {
        match self {
            Condition::KeyMatch(c) => { let _ = c.compile(); }
            Condition::Include(c)  => { let _ = c.compile(); }
            Condition::Exclude(c)  => { let _ = c.compile(); }
            _ => {}
        }
    }
}
```

**修复副作用**：
- compile 失败的错误当前被静默忽略（回退路径），修复后行为一致
- 建议在 compile 失败时记录 warning，提升可观察性
- 无并发安全问题，compile 发生在共享前

---

## High

### CPU-02: ctx_set 插件每请求编译 Regex

**文件**: `src/core/gateway/plugins/http/ctx_set/plugin.rs`
**行号**: 214

**问题**: `apply_transform()` 在处理 `regex_replace` 类型时，每个请求调用 `Regex::new(&pattern)`。其他插件（如 `request_restriction`）已在验证时预编译，但 `ctx_set` 没有。

**修复建议**: 在 `pre_parse()` 阶段预编译 Regex 并存储在配置结构体中。

---

**[深度复核结论] ✅ 确认为 High，已修复**

**修复说明**（2026-04-09）：
- `ctx_set.rs`：`CtxSetConfig` 新增 `compiled_transform_patterns: HashMap<usize, Regex>`（`#[serde(skip)]`）；`validate()` 对 transform.replace 由只验证改为验证并存入该 map；新增 `get_compiled_transform_regex(idx)` getter
- `plugin.rs`：`apply_transform` 加 `compiled_replace: Option<&Regex>` 参数，优先用缓存；`process_rule` 调用前取 `self.config.get_compiled_transform_regex(rule_idx)` 传入

**代码确认**：
```rust
// plugin.rs 212-217（修复前）
TransformType::Replace { pattern, with } => {
    match Regex::new(&pattern) {  // ← 每请求编译
        Ok(re) => Some(re.replace_all(value, with.as_str()).to_string()),
        Err(_) => None,
    }
}
```

注意：extract 路径的 regex 通过 `self.config.get_compiled_regex(rule_idx)` 走预编译路径（正确），**仅 transform.replace 路径未预编译**。

`validate()` 中只验证合法性，不保存编译结果：
```rust
if let Err(e) = Regex::new(pattern) {
    // 只报错，不缓存
}
```

**修复方案**：在 `CtxSetConfig` 增加 transform regex 缓存字段并在 validate 时填充：

```rust
// 在 CtxSetConfig 结构体中增加
#[serde(skip)]
pub compiled_transform_patterns: HashMap<usize, Regex>,

// validate 中
match Regex::new(pattern) {
    Ok(re) => { self.compiled_transform_patterns.insert(idx, re); }
    Err(e) => { /* 报错 */ }
}

// apply_transform 中改用
let re = self.config.compiled_transform_patterns.get(&idx)?;
Some(re.replace_all(value, with.as_str()).to_string())
```

**修复副作用**：
- 需要向 `apply_transform` 传入 `idx` 参数（当前不需要），调用点需同步修改
- `Regex` 实现了 `Clone`（内部用 Arc），不影响现有 `#[derive(Clone)]`

---

### CPU-03: DSL 内置函数每次调用编译 Regex

**文件**: `src/core/gateway/plugins/http/dsl/lang/builtins.rs`
**行号**: 383-384 (RegexFind), 403-404 (RegexReplace)

**问题**: `RegexFind` 和 `RegexReplace` 内置函数每次调用时从用户模式字符串编译新 Regex。虽然有 pattern 长度限制 (1024 bytes) 和 VM call budget，但在循环中（最多 100 次迭代）可能编译 100 次。

**修复建议**: 在 VM 运行时添加 LRU Regex 缓存，或在 VM 编译阶段静态提取并预编译 Regex 常量。

---

**[深度复核结论] ✅ 确认存在，降级为 Medium，已修复**

**进展说明**（2026-04-09）：
- `matches` 操作符已正确预编译（`Vm::new()` 时从 `Constant::Regex` 建 `regex_cache`），报告描述不够精准
- 真正的问题仅限 `regex_find`/`regex_replace` 两个 builtin，pattern 为运行时动态值，无法 load 时预编译
- **不采用 LRU 方案**（需 Mutex，pattern 多样时适得其反）；正确方案为编译器识别字面量 pattern 提升为 `Constant::Regex` + 新 `OpCode::RegexFind/RegexReplace(u16)`，与 `matches` 路径完全一致
- 设计文档：`tasks/todo/dsl-regex-literal-precompile.md`

**修复说明**（2026-04-09）：
- `bytecode.rs`：新增 `OpCode::RegexFind(u16)` / `OpCode::RegexReplace(u16)`，`u16` 为 constants 池索引
- `compiler.rs`：`regex_find`/`regex_replace` 调用时，若 pattern 参数为字面量 String，调用 `add_regex_const()` 存入 `Constant::Regex` 并 emit 专用 OpCode；动态 pattern 仍 emit `CallBuiltin(RegexFind, ...)`
- `vm.rs`：`Vm::new()` 已有 `Constant::Regex` 预编译逻辑，`OpCode::RegexFind`/`OpCode::RegexReplace` 分支直接从 `regex_cache` 取用，零运行时编译开销
- `validator.rs`：新增对 `OpCode::RegexFind`/`OpCode::RegexReplace` 的 constant 索引合法性检查

**代码确认**：
```rust
// builtins.rs 375-408
BuiltinId::RegexFind => {
    let pattern = state.pop()?.into_string();  // 运行时动态值
    let s = state.pop()?.into_string();
    let re = regex::Regex::new(&pattern).map_err(|e| RuntimeError::RegexError { ... })?;
    // ...
}
```

**降级原因**：
- DSL pattern 是**运行时动态值**（从 VM 栈弹出），不一定是常量
- DSL 不在所有请求路径上，仅 DSL 插件用户受影响
- 每次调用编译是真实问题，但影响面小于 CPU-01/02

**修复方案（推荐：全局 LRU 缓存）**：

```rust
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use lru::LruCache;

static REGEX_CACHE: Lazy<Mutex<LruCache<String, regex::Regex>>> = Lazy::new(|| {
    Mutex::new(LruCache::new(NonZeroUsize::new(256).unwrap()))
});

fn get_or_compile_regex(pattern: &str) -> Result<regex::Regex, regex::Error> {
    let mut cache = REGEX_CACHE.lock();
    if let Some(re) = cache.get(pattern) {
        return Ok(re.clone());
    }
    let re = regex::Regex::new(pattern)?;
    cache.put(pattern.to_string(), re.clone());
    Ok(re)
}
```

**修复副作用**：
- 引入全局锁竞争，高并发时 `Mutex` 可能成为瓶颈（可改用 DashMap 分片锁）
- `Regex::clone()` 开销极小（内部 Arc）
- pattern 多样性极高时缓存命中率低，但内存增长有上限（256 条）

---

### ~~CPU-04: `lookup_gateway()` 全量 JSON 序列化→反序列化→线性扫描~~

**文件**: `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/route_utils.rs`
**行号**: 421-430

~~**问题**: `lookup_gateway()` 调用 `processor.as_watch_obj().list_json()` 将所有 Gateway 对象序列化为 JSON 字符串，再反序列化回来，然后线性扫描匹配 namespace/name。~~

---

**[深度复核结论] ❌ 误判，无需修复**

**实际代码**：
```rust
// route_utils.rs 427-431
pub fn lookup_gateway(namespace: &str, name: &str) -> Option<Arc<Gateway>> {
    let processor = PROCESSOR_REGISTRY.get("Gateway")?;
    let key = format!("{}/{}", namespace, name);
    processor.get_raw_any(&key)?.downcast::<Gateway>().ok()
}
```

**实际情况**：`get_raw_any` 走的是 `KubeRawStore`（内部为 kube reflector `Store`，基于 DashMap）或 `FsRawStore`（`RwLock<HashMap>`），均为 **O(1) 查找**。`downcast::<Gateway>()` 只做 `TypeId` 比较，开销极小。**没有 JSON 序列化，没有线性扫描**，代码正确高效。

原分析是对旧代码的过时描述，或对 `get_raw_any` 实现有误解。

---

### ~~CPU-05: `lookup_service_cluster_ip()` 同样模式的全量 JSON round-trip~~

**文件**: `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/gateway.rs`
**行号**: 816-832

---

**[深度复核结论] ❌ 误判，无需修复**

**实际代码**：
```rust
// gateway.rs 817-825
fn lookup_service_cluster_ip(namespace: &str, name: &str) -> Option<String> {
    let processor = PROCESSOR_REGISTRY.get("Service")?;
    let key = format!("{}/{}", namespace, name);
    let svc: Arc<Service> = processor.get_raw_any(&key)?.downcast::<Service>().ok()?;
    svc.spec.as_ref()
        .and_then(|spec| spec.cluster_ip.clone())
        .filter(|ip| !ip.is_empty() && ip != "None")
}
```

与 CPU-04 完全相同情况：DashMap O(1) 查找 + Arc downcast。**代码正确高效，无需修复**。

---

## Medium

### CPU-06: `extract_status_json()` 双重序列化

**文件**: `src/core/controller/conf_mgr/sync_runtime/resource_processor/processor.rs`
**行号**: 67-78（函数定义）, 526、546（调用位置）

**问题**: `extract_status_json()` 先 `serde_json::to_value(obj)` 序列化整个对象为 Value 树，再 `.get("status")`，再 `serde_json::to_string(status)`。在 `process_resource_inner` 中每个资源处理触发 1-2 次完整序列化。

**修复建议**: 仅序列化 status 字段，或使用 hash 比较检测 status 是否变化。

---

**[深度复核结论] ✅ 确认存在，降级为 Low**

**实际情况**：此路径是**控制平面**（K8s 资源变更事件处理），不在 HTTP 请求热路径上，每秒触发频率远低于请求处理。对象大小有限（CRD）。双重序列化确实存在但影响有限。

**修复优先级**：Optional。若要优化，可让 T 实现 `status_json() -> Option<String>` trait，编译期只序列化 status 字段，但改造成本高，性价比低。

---

### CPU-07: `update_tracker_accepted_only()` 冗余调用 `lookup_gateway()`

**文件**: `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/route_utils.rs`
**行号**: 343-380

**问题**: `update_tracker_accepted_only()` 内部再次调用 `validate_parent_ref_accepted()`，后者又调用 `lookup_gateway()`。而 `update_status()` 已经直接调用过 `validate_parent_ref_accepted()`。每个 parentRef 的 `lookup_gateway()` 被执行至少两次。

**修复建议**: 将 acceptance 结果缓存传递给 `update_tracker_accepted_only()`，避免重复计算。

---

**[深度复核结论] ⚠️ 部分确认，优先级低**

`lookup_gateway` 单次是 O(1) DashMap 查找（见 CPU-04 复核），开销极小。"冗余"问题技术上存在，但由于 lookup 代价可忽略，且这是控制平面路径，**实际影响可以忽略**。

**修复建议**：No（除非 profiling 显示此处是瓶颈）

---

### CPU-08: `process_resource_inner` 全量 Clone 处理对象

**文件**: `src/core/controller/conf_mgr/sync_runtime/resource_processor/processor.rs`
**行号**: 562

**问题**: `let obj_for_result = parsed_obj.clone()` 深拷贝整个资源对象。对于大资源（多 listener 的 Gateway、多 rule 的 HTTPRoute），这是不必要的分配。

**修复建议**: 在 save 前提取 status 值，避免全量克隆。

---

**[深度复核结论] ✅ 确认存在，评级 Low~Medium**

**实际代码**：
```rust
// processor.rs 601-606
let obj_for_result = parsed_obj.clone();  // 必须 clone，因为后续需要 move
if is_init {
    self.cache.apply_change(ResourceChange::InitAdd, parsed_obj, Some(sv));
} else {
    self.save(parsed_obj, Some(sv));
}
```

这是一次**必要的 clone**（Rust 不支持一个值同时被移动和使用）。优化路径：将 `parsed_obj` 改为 `Arc<T>` 包裹，clone 降为 O(1) 引用计数操作。但需评估 `WorkItemResult::Processed.obj` 是否在调用方真正被使用。

**修复建议**：Optional（改造成本中等，控制平面路径影响有限）

---

### ~~CPU-09: 每请求 Metrics 标签 String 分配~~

**文件**: `src/core/gateway/observe/metrics/registry.rs`
**行号**: 259-284 (`record_backend_request`), 338-364 (`record_request_duration`)

~~**问题**: 每请求调用 `record_backend_request()` 时构造 10 个 label 维度，每个 `.to_string()` 堆分配。`record_request_duration()` 同样创建 `Vec::with_capacity(8)` 并 push 8 个 `(str, String)` 元组。~~

~~**修复建议**: 预分配 label buffer 复用，或使用 `&str` 引用替代 `String`。~~

---

**[深度复核结论] ❌ 误判，关闭**

**根本原因**：`metrics` 0.24 的 `SharedString = Cow<'static, str>`。动态 label 值（gateway name、namespace、route 等均非 `'static`）**必须**是 `Cow::Owned(String)`，`.to_string()` 是 API 要求的最小转换，不可避免。

原复核建议的 `Arc<str>` 方案不可行：`Arc<str>` 没有 `'static` lifetime，无法转换为 `Cow<'static, str>`。`&str` 方案同样不可行：非 `'static` 的 `&str` 无法绕过 owned 要求。

metrics 调用的真正开销主要来自**注册表 lookup**（hash + atomic），而非 string alloc 本身。无需修复。

---

### CPU-10: 全局插件每请求 `format!()` + 不必要的 clone

**文件**: `src/core/gateway/routes/http/proxy_http/pg_request_filter.rs`
**行号**: 164-185

**问题**: 每请求循环 `global_plugins_ref`，为每个全局插件 `format!("{}/{}", ns, name)` 构造 key，再通过 `PluginStore.get()` 查找。`get()` 内部 clone 整个 `EdgionPlugins` 结构体（含多个 `Option<Vec<...>>`），实际只用到其中的 `Arc<PluginRuntime>`。

**修复建议**: 用已有的 `with_plugin()` 替换 `get()`，避免不必要的 clone。

---

**[深度复核结论] ✅ 确认存在，已修复**

**Store 实现澄清**：原描述"DashMap lookup"有误。实际实现是 `ArcSwap<HashMap<String, EdgionPlugins>>`（`plugin_store.rs`），读路径无锁（atomic load + HashMap O(1) 查找），远比 DashMap 轻量。

**真正的开销**：
1. `format!()` per plugin per request — 小字符串 heap alloc，minor
2. `get()` 返回 `Option<EdgionPlugins>`（cloned），但调用方只用 `spec.plugin_runtime`（一个 `Arc`） — 这是多余 clone

**修复说明**（2026-04-09）：
- `plugin_store.rs`：新增 `PluginStore::get_plugin_runtime(key)` 方法，内部借用访问 `Arc<PluginRuntime>`，只 clone Arc（O(1)），不 clone 整个 `EdgionPlugins`
- `pg_request_filter.rs`：`store.get()` → `get_plugin_runtime()`，消除每请求 `EdgionPlugins` 深拷贝
- 同步修复 `extension_ref.rs`、`pg_upstream_response_body_filter.rs` 中相同模式

---

### CPU-11: 每请求 `format!()` 分配 (多处热路径)

**文件**:
- `src/core/gateway/routes/http/proxy_http/pg_request_filter.rs:167-170`
- `src/core/gateway/routes/http/proxy_http/pg_upstream_peer.rs:362`
- `src/core/gateway/routes/http/proxy_http/pg_upstream_response_body_filter.rs:50-53`

**问题**: 多个热路径中 `format!("{}/{}", namespace, name)` 构造 plugin/service key。在 body_filter 中甚至每个 body chunk 都执行。

**修复建议**: 在配置加载时预计算 key 字符串。

---

**[深度复核结论] ✅ 确认存在，严重度不一**

- **位置1/3**（global plugin key）：与 CPU-10 重叠，随 CPU-10 修复一并解决
- **位置2**（session persistence service_key）：仅在 `sp_active && token 存在` 条件分支内执行，非每请求必经路径，优先级 Low

**修复建议**：Optional（位置2 可预计算存储在 BackendRef 中）

---

### CPU-12: 每请求 Access Log JSON 序列化

**文件**: `src/core/gateway/observe/access_log/entry.rs:87-92`
**文件**: `src/core/gateway/routes/http/proxy_http/pg_logging.rs:54`

**问题**: 每请求将整个 `AccessLogEntry`（含嵌套结构、ctx_map）序列化为 JSON。`from_context()` 还对 ctx_map 做 denylist 过滤，每个 key `.to_lowercase().contains()` 扫描，O(K*D)。

**修复建议**: 使用预分配 buffer 复用或更轻量的日志格式。

---

**[深度复核结论] ✅ 确认存在，额外发现不必要的 `.clone()`**

**实际代码**：
```rust
// pg_logging.rs 53-65
let entry = AccessLogEntry::from_context(ctx);
let entry_json = entry.to_json();
// ...
edgion_http.access_logger.send(entry_json.clone()).await;  // ← 不必要的 clone
```

`send(entry_json.clone())` 在 `send` 之后 `entry_json` 不再被使用（debug log 用 `%entry_json` 可在 send 前使用），多了一次 String 拷贝。

**修复方案（最小修复，立即收益）**：
1. 调整 debug log 和 send 顺序，直接 `send(entry_json)` 消除 clone
2. `ctx_map` 过滤中 `k.to_lowercase()` 可预计算或改为 ASCII 大小写不敏感比较

**修复副作用**：调整顺序无逻辑变更；denylist 优化需注意 Unicode 情况。

---

### ~~CPU-13: `match_header` / `match_query_param` 回退 Regex 编译~~

**文件**: `src/core/gateway/routes/http/match_unit.rs`
**行号**: 270-274 (header), 305-309 (query param)

~~**问题**: 当 `compiled_regex` 查找返回 `None` 时，回退到每请求 `Regex::new()`。~~

---

**[深度复核结论] ❌ 基本不存在，无需修复**

**实际情况**：`MatchItem` 结构体已有 `compiled_header_regexes` 和 `compiled_query_regexes` 字段，在路由创建时预先编译，请求匹配时通过索引传入。回退的 `Regex::new()` 分支只在 index 越界（数组长度与 matcher 数量不一致）时触发，属于配置 bug 场景，正常情况下永远不走。

原代码存在的防御性回退可改为直接返回错误，语义更清晰，但不是性能问题。

---

### CPU-14: DNS 无缓存每请求解析

**文件**: `src/core/gateway/routes/http/proxy_http/pg_upstream_peer.rs`
**行号**: 507-530

**问题**: `resolve_domain()` 每次调用 `tokio::net::lookup_host()`，无缓存。对外部后端高流量时可打满 OS resolver。

**修复建议**: 引入 DNS 缓存层（如 trust-dns 或自建 TTL 缓存）。

---

**[深度复核结论] ✅ 确认存在，评级 Medium（仅影响 ExternalJump）**

**实际代码**：
```rust
async fn resolve_domain(domain: &str, port: u16) -> pingora_core::Result<std::net::SocketAddr> {
    let addr_str = format!("{}:{}", domain, port);
    let mut addrs = tokio::net::lookup_host(&addr_str).await.map_err(...)?;
    // 无任何缓存
}
```

`tokio::net::lookup_host` 底层调用 `getaddrinfo`，在 Linux 默认不缓存（nscd 未启用时）。此函数**仅在 ExternalJump 路径**调用，普通 K8s Service 不经过此路径。

**修复方案**：引入带 TTL 的本地缓存：
```rust
static DNS_CACHE: LazyLock<DashMap<String, (SocketAddr, Instant)>> = LazyLock::new(DashMap::new);
const DNS_CACHE_TTL: Duration = Duration::from_secs(30);

async fn resolve_domain(domain: &str, port: u16) -> ... {
    let key = format!("{}:{}", domain, port);
    if let Some(entry) = DNS_CACHE.get(&key) {
        if entry.1.elapsed() < DNS_CACHE_TTL { return Ok(entry.0); }
    }
    // ...原有解析...
    DNS_CACHE.insert(key, (addr, Instant::now()));
    Ok(addr)
}
```

**修复副作用**：DNS 变更后最长 30s 生效；并发 stampede 可用 singleflight 进一步优化。

---

### CPU-15: `ResourceKind::from_content()` 每次调用编译 Regex

**文件**: `src/types/resource/kind.rs`
**行号**: 114-130

**问题**: `from_content()` 每次调用时用 `Regex::new()` 编译 JSON 和 YAML 两个模式。虽然不在 HTTP 请求热路径上，但在 gRPC 配置同步和文件 watcher 路径中每个资源事件都调用。

**修复建议**: 使用 `LazyLock<Regex>` 静态编译。

---

**[深度复核结论] ✅ 确认存在，评级 Low**

**修复方案（极简，无副作用）**：
```rust
use std::sync::LazyLock;

static JSON_PATTERN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"^\s*\{.*"kind""#).unwrap());
static YAML_PATTERN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"^kind:\s*\w+"#).unwrap());

// from_content 中直接使用 &*JSON_PATTERN 和 &*YAML_PATTERN
```

**修复副作用**：无，纯优化，内存占用几乎不变（全局 2 个编译好的 Regex）。

---

### CPU-16: Hostname 双重反转

**文件**: `src/core/common/matcher/host_match/radix_match/radix_host_match.rs:67,107`
**文件**: `src/core/common/matcher/host_match/radix_match/radix_host.rs:109-118`

**问题**: `match_host()` 在第 67 行反转 hostname 用于 Radix 树查找，然后 `radix_host.matches()` 在内部（131 行）再次反转同一 hostname。每次调用 `reverse_hostname()` 分配 `Vec<&str>` + `String`。每请求执行两次。

**修复建议**: 传入已反转的 hostname，或添加 `matches_reversed()` 方法。

---

**[深度复核结论] ✅ 确认存在，评级 Low（低成本修复）**

**实际代码**：
```rust
// radix_host_match.rs:67
let reversed = RadixHost::<T>::reverse_hostname(hostname);
// ...
// radix_host.rs:130-141 - matches() 内部再次反转
pub fn matches(&self, request_host: &str) -> bool {
    let reversed_request = Self::reverse_hostname(request_host);  // 二次反转
    // ...
}
```

**修复方案（低成本）**：
```rust
// radix_host.rs 新增
pub fn matches_reversed(&self, reversed_request: &str) -> bool {
    if self.is_wildcard { self.wildcard_matches(reversed_request) }
    else { reversed_request == self.radix_key }
}

// radix_host_match.rs 中复用 reversed
if radix_host.matches_reversed(&reversed) { ... }
```

**修复副作用**：无，纯重构，需更新所有调用 `matches()` 的地方。

---

## Low

### CPU-17: `Rsc.kind` 从静态字面量分配 String

**文件**: `src/types/ctx.rs:23-27, 54-58, 618-622`

**问题**: `Rsc` 结构体的 `kind: String` 字段始终为常量值（如 `"HTTPRoute"`、`"Service"`），但每请求从字面量 `String::from("...")` 堆分配。每请求至少 2 次。

**修复建议**: 改为 `&'static str` 或 `Cow<'static, str>`。

---

**[深度复核结论] ✅ 确认存在，评级 Low**

改为 `&'static str` 即可完全消除此分配，改动量小但需同步修改使用 `kind` 字段的代码。**Optional**。

---

### CPU-18: gRPC 压缩事件处理 100ms 轮询循环

**文件**: `src/core/gateway/conf_sync/cache_client/cache_data.rs:103-117`

**问题**: 每种资源类型（约 17 种）各 spawn 一个 tokio task，每 100ms 获取 write lock 检查事件。空闲时约 170 次/秒无意义的锁获取。

**修复建议**: 改为 `Notify`-based 唤醒，有事件时才处理。

---

**[深度复核结论] ✅ 确认存在，评级 Low**

空闲时 170 次/秒的轻量 write lock 获取对 tokio runtime 有一定调度开销，但实际 CPU 占用极小（每次 < 1μs）。`Notify`-based 改造后可完全消除空闲开销，改造难度低。**建议作为代码质量改进**。

---

### CPU-19: UUID v4 每请求生成 Trace ID

**文件**: `src/core/gateway/routes/http/proxy_http/pg_request_filter.rs:297-305`

**问题**: 无 `x-trace-id` 头时每请求 `uuid::Uuid::new_v4().to_string()`（含 CSPRNG + 格式化）。

---

**[深度复核结论] ✅ 确认存在，评级 Low**

UUID v4 生成含 CSPRNG 调用（约 100-200ns），大多数 trace 工具已提供更轻量的 ID 生成。但此开销通常可接受，且只在无 `x-trace-id` 头时触发。**Low 优先级，可选优化**。

---

### CPU-20: `HTTPBackendRef` 非 Arc clone

**文件**: `src/core/gateway/routes/http/proxy_http/pg_response_filter.rs:40`
**文件**: `src/core/gateway/routes/http/proxy_http/pg_upstream_response_filter.rs:122`

**问题**: `ctx.selected_backend.clone()` 包含 String/Vec 字段，每请求在 response 阶段至少 clone 两次。

---

**[深度复核结论] ✅ 确认存在，评级 Low**

将 `selected_backend` 改为 `Arc<HTTPBackendRef>` 后 clone 变为 O(1)，但需修改类型和相关赋值。**Optional**。

---

### CPU-21: `key_name()` 每次调用分配 String

**文件**: `src/types/resource/meta/traits.rs:77-83`

**问题**: `format!("{}/{}", namespace, name)` 在紧凑循环中频繁调用。

---

**[深度复核结论] ✅ 确认存在，评级 Low**

可将 `key_name` 作为预计算字段存储在资源元数据中，避免每次动态生成。改造范围较广，性价比低。**Optional**。

---

### CPU-22: `GatewayBaseConf::add_gateway()` O(n) 线性扫描

**文件**: `src/types/gateway_base_conf.rs:76-89`

**问题**: 更新已存在 Gateway 时，`self.gateways.iter_mut().find()` 线性扫描，每个 `g.key_name()` 调用分配 String。

---

**[深度复核结论] ✅ 确认存在，评级 Low**

将 `gateways: Vec` 改为 `HashMap`（key 为 `key_name`）可解决，但需评估调用方是否依赖顺序。**Optional**。

---

### CPU-23: `HashHost::get()` 冗余的 `is_valid_domain()` 检查

**文件**: `src/core/common/matcher/host_match/hash_match/hash_host.rs:65-88`

**问题**: hostname 已被 `NormalizedHostname::new()` 规范化，`get()` 再做一次全域名字节扫描是多余的。

---

**[深度复核结论] ✅ 确认存在，评级 Low**

冗余检查每次约 O(n) 字节扫描（n = hostname 长度），在请求热路径上，但实际 hostname 通常 < 100 bytes，耗时约 20-50ns。**Optional**。

---

### CPU-24: `all_resource_type_names()` 每次调用分配 Vec

**文件**: `src/types/resource/registry.rs:77-98`

**问题**: 资源定义是静态的，但函数每次都 collect 新 Vec。

---

**[深度复核结论] ✅ 确认存在，评级 Low**

用 `LazyLock<Vec<&'static str>>` 即可修复，极简改动。不在热路径上。**Optional**。

---

### CPU-25: Watch 事件双重 JSON 解析 + Value 深拷贝

**文件**: `src/core/gateway/conf_sync/cache_client/event_dispatch.rs:299-316`

**问题**: Watch 事件先 parse 为 `Vec<serde_json::Value>`，再 `.clone()` 每个 Value 后反序列化为具体类型。大资源（如 EndpointSlice）的 Value clone 开销大。

---

**[深度复核结论] ✅ 确认存在，评级 Low**

Value clone 对大资源（如含数千 endpoints 的 EndpointSlice）有显著开销。可改为直接从原始字节反序列化为目标类型，跳过中间 Value 表示。**建议修复但优先级低**。

---

### CPU-26: LDAP 每请求创建 TCP 连接（缓存 miss 时）

**文件**: `src/core/gateway/plugins/http/ldap_auth/plugin.rs:181`

**问题**: 无连接池，每次缓存 miss 建立新 LDAP TCP 连接。暴力破解攻击可打满文件描述符。

---

**[深度复核结论] ✅ 确认存在，评级 Medium（安全影响）**

除性能问题外还有安全影响：暴力破解攻击触发大量 cache miss，每次建立新 TCP 连接可耗尽文件描述符。**建议引入连接池**（如 `deadpool` crate），但 LDAP 协议特殊性（连接绑定状态）需要仔细设计。
