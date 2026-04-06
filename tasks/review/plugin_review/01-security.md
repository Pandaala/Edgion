# Security Issues

## SEC-01 [Important] ctx_set 模板注入

**模块**: `src/core/gateway/plugins/http/ctx_set/plugin.rs:73-83`

**问题**: `resolve_template` 方法使用 `while` 循环搜索 `${`，替换后的值如果本身包含 `${}`，会被二次展开。攻击者控制 header 值（如 `X-User: ${header:Authorization}`）即可通过模板引擎泄露其他请求头数据到 context 变量中。

**影响**: 请求数据泄露。攻击者通过构造特定 header 值，可以读取同一请求中的其他敏感 header（如 Authorization、Cookie）。

**修复建议**: 替换循环不应重新扫描已替换的内容。使用游标递增构建结果：

```rust
fn resolve_template(&self, template: &str, session: &dyn PluginSession) -> String {
    let mut result = String::new();
    let mut remaining = template;
    while let Some(start) = remaining.find("${") {
        result.push_str(&remaining[..start]);
        let after_start = &remaining[start + 2..];
        match after_start.find('}') {
            Some(end) => {
                let var_expr = &after_start[..end];
                result.push_str(&self.resolve_var_expr(var_expr, session));
                remaining = &after_start[end + 1..];
            }
            None => {
                result.push_str(&remaining[start..]);
                remaining = "";
            }
        }
    }
    result.push_str(remaining);
    result
}
```

---

## SEC-02 [Important] IPv4-mapped IPv6 地址绕过 IP 限制

**模块**:
- `src/core/gateway/plugins/http/ip_restriction/plugin.rs:59-71`
- `src/core/common/matcher/ip_radix_tree/matcher.rs:62-67`

**问题**: 当客户端通过 IPv6 连接时，内核可能报告 IPv4 地址为 IPv4-mapped IPv6 形式（如 `::ffff:192.168.1.100`）。Rust 的 `IpAddr::parse` 将其解析为 `IpAddr::V6`，而 `IpRadixMatcher::match_ip` 将 V6 地址只发送到 IPv6 树。如果管理员只配置了 IPv4 规则（如 `deny: ["192.168.1.100"]`），IPv4-mapped IPv6 地址不会匹配任何规则。

**影响**: 双栈环境下 IP 限制完全失效。

**修复建议**: 在匹配前标准化 IPv4-mapped IPv6 地址：

```rust
fn match_ip(&self, ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => self.match_ipv4(v4),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return self.match_ipv4(&v4);
            }
            self.match_ipv6(v6)
        }
    }
}
```

---

## SEC-03 [Important] Stream IP restriction 验证失败后 fail-open

**模块**:
- `src/core/gateway/plugins/stream/ip_restriction/stream_ip_restriction.rs:35-37`
- `src/core/gateway/plugins/stream/tls_route/ip_restriction.rs:21-23`

**问题**: 当 `config.validate_and_init()` 失败时，错误被记录但插件仍然被构建，`allow_matcher` 和 `deny_matcher` 均为 `None`。此时 `check_ip_access` fallthrough 到 `default_action`（默认 `Allow`），导致配置错误的 IP 限制插件静默放行所有流量。

**影响**: 配置错误（如无效 CIDR）使得安全插件完全失效，且无任何告警。

**修复建议**: 构造函数返回 `None` 使插件不被添加到 runtime，或设置一个标志让 `on_connection` 在初始化失败时始终返回 `Deny`。

---

## SEC-04 [Important] TLS passthrough 缺失 Stage 2 插件执行

**模块**: `src/core/gateway/routes/tls/passthrough_proxy.rs` — 整个 `handle_connection` 方法

**问题**: `EdgionTlsPassthroughProxy` 不执行任何 Stage 2 (TLS route) stream 插件。当 TLSRoute 配置了 `edgion.io/edgion-stream-plugins` 注解且资源定义了 `tls_route_plugins` 时，passthrough 连接完全绕过路由级插件检查。

**影响**: 管理员为 TLSRoute 配置的 IP 限制在 passthrough 模式下不生效。Stage 1 (ConnectionFilter) 仍在 listener 级别工作，但路由级粒度丢失。

**修复建议**: 在 `handle_connection` 的路由匹配后添加 `run_tls_route_plugins` 调用（类似 `EdgionTlsTcpProxy::run_tls_route_plugins`）。注意 passthrough 模式下 `is_mtls` 和 `tls_id` 不可用，context 应使用适当默认值。

---

## SEC-05 [Important] IPv6 客户端绕过 Stage 2 stream 插件

**模块**:
- `src/core/gateway/routes/tls/proxy.rs:129-133`
- `src/core/gateway/routes/tls/passthrough_proxy.rs:53`

**问题**: 客户端地址提取使用 `.as_inet()` 只匹配 IPv4 `SocketAddr`。IPv6 连接 fall through 到 `unwrap_or_else` 分支，`client_addr` 被设为 `"unknown"`。下游 `ctx.client_addr.parse()` 对 `"unknown"` 失败，导致 stream 插件检查被完全跳过。

**影响**: IPv6 客户端绕过所有 Stage 2 stream 插件检查。

**修复建议**:

```rust
.and_then(|addr| {
    addr.as_inet().map(|inet| (inet.ip().to_string(), inet.port()))
        .or_else(|| addr.as_inet6().map(|inet6| (inet6.ip().to_string(), inet6.port())))
})
```

---

## SEC-06 [Important] dynamic_external_upstream domain 验证过弱

**模块**: `src/types/resources/edgion_plugins/plugin_configs/dynamic_external_upstream.rs:231-234`

**问题**: Domain 验证只检查空格和斜杠：`target.domain.contains(' ') || target.domain.contains('/')`。允许换行、null bytes、冒号等可能在 TLS SNI、Host header 或 DNS 解析中造成问题的字符。

**影响**: 畸形 domain 可能导致错误的 TLS 连接或 DNS 混淆。攻击面限于运维配置错误。

**修复建议**: 按 RFC 1123 验证域名（字母数字、连字符、点；标签 1-63 字符；总长 <=253）。拒绝 null bytes、控制字符。

---

## SEC-07 [Important] rate_limit JSON 注入

**模块**:
- `src/core/gateway/plugins/http/rate_limit/plugin.rs:195`
- `src/core/gateway/plugins/http/rate_limit_redis/plugin.rs:461`

**问题**: `reject_message` 直接通过 `format!` 插入 JSON：
```rust
let body = Bytes::from(format!(r#"{{"message":"{}"}}"#, message));
```
如果 `reject_message` 包含双引号，生成的 JSON 格式错误。

**影响**: 产生格式错误的 JSON 响应，混淆下游客户端。

**修复建议**: 使用 `serde_json::json!({"message": message}).to_string()`（ip_restriction 插件已使用此方式）。

---

## SEC-08 [Important] forward_auth 无限制响应体

**模块**: `src/core/gateway/plugins/http/forward_auth/plugin.rs:345-349`

**问题**: 认证服务返回非 2xx 响应时，插件读取完整响应体 (`resp.text().await`) 并原样转发给客户端。无大小限制，无内容过滤。

**影响**:
1. **内存耗尽**: 被入侵或错误配置的认证服务可发送任意大小的响应
2. **XSS**: 如果认证服务返回 HTML/script 内容并被浏览器渲染

**修复建议**: 限制响应体读取大小（如 64KB），或替换为通用错误消息只转发结构化数据。

---

## SEC-09 [Important] key_auth HashMap timing 侧信道

**模块**: `src/core/gateway/plugins/http/key_auth/plugin.rs:99-101`

**问题**: `validate_key` 使用 `self.keys.get(key)` 进行标准 HashMap 查找。当 key 的 hash bucket 匹配时，后续的字节比较会泄露 key 长度和前缀信息。

**影响**: API key 是长期存在的 secret。与 JWT 不同，API key 持久有效。timing oracle 允许离线 key 发现。

**修复建议**: 对传入 key 进行 SHA-256 hash 后用 hash 作为 map key（类似 `ldap_auth` 的 cache 策略），消除提交 key 和存储 key 之间的 timing 关联。
