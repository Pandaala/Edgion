# Correctness Issues

## COR-01 [Important] real_ip `recursive` 字段被忽略

**模块**:
- `src/core/gateway/plugins/http/real_ip/plugin.rs:114-150`
- `src/types/resources/edgion_plugins/plugin_configs/real_ip.rs:64-71`

**问题**: `RealIpConfig` 有 `recursive` 字段（默认 `true`），对应 Nginx 的 `real_ip_recursive`。但 `extract_real_ip_with_extractor()` 始终执行从右到左的递归遍历，不管 `recursive` 的值。

当 `recursive: false` 时，Nginx 行为是：只取 header 最后（最右）一个 IP，如果受信任则用 client address；如果不受信任则直接使用。当前实现始终遍历所有 IP。

**影响**: 配置 `recursive: false` 的用户得到非预期行为。对 multi-IP header（如 X-Forwarded-For）影响明显。

**修复建议**: 将 `recursive` 传递给 `extract_real_ip_with_extractor()`，当 `recursive: false` 时只提取最右侧 IP。

---

## COR-02 [Important] real_ip 双重实现路径

**模块**:
- `src/core/gateway/plugins/http/real_ip/plugin.rs:67-88`（插件实现）
- `src/core/common/utils/real_ip_extractor.rs:124-171`（工具类实现）

**问题**: real_ip 插件创建了 `RealIpExtractor` 但没有使用其 `extract_real_ip()` 方法，而是在 `extract_real_ip_with_extractor()` 中重新实现了提取逻辑。两个实现存在行为差异（见 COR-03）。

**影响**: 维护风险——修复一个实现时另一个可能遗漏。

**修复建议**: 重构 `RealIpExtractor` 暴露 `fn extract(client_addr: &str, header_value: &str) -> Option<String>` 方法，让插件直接使用，消除重复。

---

## COR-03 [Important] real_ip 所有 IP 受信任时 fallback 不一致

**模块**:
- `src/core/gateway/plugins/http/real_ip/plugin.rs:145-149`
- `src/core/common/utils/real_ip_extractor.rs:170`

**问题**: 当 X-Forwarded-For 中所有 IP 都受信任时：
- 插件 (`extract_real_ip_with_extractor`): 返回最左侧 IP（Nginx 行为）
- 工具类 (`RealIpExtractor::extract_real_ip`): 返回 `client_addr`

如果两个代码路径在不同场景下使用，同一请求可能得到不同的 real IP 解析结果。

**影响**: 行为不一致，取决于使用哪条代码路径。

**修复建议**: 统一到 Nginx 兼容行为（返回 header 最左侧 IP）。

---

## COR-04 [Important] UrlRewrite 在 EdgionPlugins 路径中静默丢弃

**模块**: `src/core/gateway/plugins/runtime/plugin_runtime.rs:353-393`

**问题**: `EdgionPlugin::UrlRewrite` variant 存在于 enum 和 `get_plugin_name` 中，但未在 `create_request_filter_from_edgion` 中处理，落入 `_ => None`。用户在 EdgionPlugins YAML 中配置 `type: UrlRewrite` 时，插件被静默忽略。

注意：`UrlRewrite` 通过 HTTPRoute filter 路径 (`from_httproute_filters`) 正常工作，问题仅限于 EdgionPlugins CRD 路径。

**影响**: 用户认为 UrlRewrite 已启用但实际未执行。

**修复建议**:
```rust
EdgionPlugin::UrlRewrite(config) => Some(Box::new(URLRewriteFilter::new(config.clone()))),
```

---

## COR-05 [Important] ExtensionRef 内 ErrResponse 在 sync 阶段被吞掉

**模块**: `src/core/gateway/plugins/runtime/gateway_api_filters/extension_ref.rs:127-135`

**问题**: ExtensionRef 的 sync `run_extension` 路径返回 `ErrResponse` 给外层 `run_upstream_response_plugins_sync`，但外层循环（plugin_runtime.rs:674）只检查 `ErrTerminateRequest`。`ErrResponse` 被当作 `GoodNext` 处理，插件链继续执行。

**影响**: ExtensionRef 内插件在 upstream_response_filter 阶段返回的错误响应被静默忽略。

**修复建议**: 在 `run_upstream_response_plugins_sync` 和 `run_upstream_response_plugins_async` 中处理 `ErrResponse`，类似 `run_request_plugins` 的处理方式。

---

## COR-06 [Important] header_cert_auth verify_depth 未实施

**模块**: `src/core/gateway/plugins/http/header_cert_auth/plugin.rs:198-225`

**问题**: 配置有 `verify_depth` 字段（默认 1），但 `verify_against_ca` 只检查叶子证书的 issuer 是否匹配任一 CA 的 subject 并直接验证签名。不遍历中间证书链，`verify_depth` 字段从未被使用。

**影响**:
1. 依赖中间 CA 的部署中，非直接 CA 签发的证书会被错误拒绝
2. `verify_depth > 1` 给运维人员虚假的安全感

**修复建议**: 实现证书链遍历（按 `verify_depth` 层级检查 issuer->subject 链），或删除 `verify_depth` 配置字段避免误导。

---

## COR-07 [Important] jwt_auth RwLock 中毒导致永久认证失败

**模块**: `src/core/gateway/plugins/http/jwt_auth/plugin.rs:133-157`

**问题**: `load_credentials` 使用 `std::sync::RwLock`。如果线程在持有写锁时 panic（如 secret 加载过程中），RwLock 被标记为 poisoned，所有后续 JWT 认证请求永久失败并返回 "JWT credential cache lock poisoned"，直到网关重启。

**影响**: 单次 panic 导致所有 JWT 认证流量永久 DoS。错误以 `debug` 级别记录并返回 401，运维人员可能无法察觉根因。

**修复建议**: 替换为 `parking_lot::RwLock`（不会 poison）或 `tokio::sync::RwLock`，或使用 `ArcSwap` 实现无锁模式。

---

## COR-08 [Important] mock delay 无上限

**模块**:
- `src/types/resources/edgion_plugins/plugin_configs/mock.rs:46`
- `src/core/gateway/plugins/http/mock/plugin.rs:42-44`

**问题**: `delay` 字段是 `Option<u64>` 无上限验证。极端值（如 `u64::MAX` 或 86400000 = 24h）会导致 `tokio::time::sleep` 长期阻塞连接和 Tokio task slot。

**影响**: 错误配置可耗尽连接池和 task 资源。

**修复建议**: 在 `validate()` 中限制 delay 上限（如 30000ms）:
```rust
if let Some(delay) = self.delay {
    if delay > 30_000 {
        return Err("delay must not exceed 30000ms".to_string());
    }
}
```

---

## COR-09 [Minor] request_restriction `All` mode 在 Skip 规则下逻辑错误

**模块**: `src/core/gateway/plugins/http/request_restriction/plugin.rs:139-152`

**问题**: `All` 模式下检查 `denied_count > 0 && denied_count == total_rules`。如果有规则返回 `None` (Skip)，它不计入 `denied_count` 但计入 `total_rules`。3 条规则中 1 条 Skip、2 条 Deny，判定 `2 == 3 = false`，请求被放行。

**修复建议**: 单独追踪 non-skipped 规则数：
```rust
let non_skipped = total_rules - skip_count;
(non_skipped > 0 && denied_count == non_skipped, denied_rule_name)
```

---

## COR-10 [Minor] session_adapter append_response_header 在请求阶段丢失 append 语义

**模块**: `src/core/gateway/plugins/runtime/session_adapter.rs:292-302`

**问题**: 请求阶段 `response_header` 为 `None` 时，`set_response_header` 和 `append_response_header` 都 push 到同一个 `response_headers_to_add` Vec。Drain 时统一用 `insert_header`（覆盖），导致 `append_response_header` 实际执行 set 操作。

**影响**: CORS filter 的 `append_response_header("Vary", "Origin")` 可能覆盖上游已有的 `Vary` header。

**修复建议**: 在队列中存储 `(name, value, is_append)` 元组，drain 时分别使用 `append_header` / `insert_header`。

---

## COR-11 [Minor] remove_response_header 在请求阶段静默无操作

**模块**: `src/core/gateway/plugins/runtime/session_adapter.rs:305-310`

**问题**: 请求阶段调用 `remove_response_header` 时 `self.response_header` 为 `None`，方法什么都不做直接返回 `Ok(())`，且无队列机制。

**影响**: 当前 `ResponseHeaderModifier` 只注册为 `UpstreamResponseFilter` 所以安全，但 API 行为可能误导。

---

## COR-12 [Minor] forward_auth cache key 缺少 path/method 导致跨路径缓存

**模块**: `src/core/gateway/plugins/http/forward_auth/plugin.rs:96-116, 252-262`

**问题**: Cache key 基于配置 header 值的 SHA-256，不包含请求 path 或 method。如果认证服务基于路径做访问控制（通过 `X-Forwarded-Uri`/`X-Forwarded-Method`），缓存会返回错误的授权结果。

**影响**: 用户 A 访问 `/admin` 获准后，同 header 访问 `/secret` 也会命中缓存获准。

**修复建议**: Cache key 计算中包含 request path、method 和 query string。

---

## COR-13 [Minor] DSL range() 产生字符串列表而非整数列表

**模块**: `src/core/gateway/plugins/http/dsl/lang/builtins.rs:419-425`

**问题**: `range(start, end)` 创建 `Value::List` 包含字符串表示的整数 ("0", "1", ...)。`for-in` 循环变量为字符串，做算术运算时会类型不匹配。

**修复建议**: 文档说明 `range()` 返回字符串，推荐使用 `for i in range(0, 100)` (ForRange 语法) 而非自由函数版本。
