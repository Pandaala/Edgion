---
name: ip-source-client-ip-false-positive
description: IP 限制 / 限流 ClientIp finding 判定规则。审查"未配置 RealIp 时 ClientIp 可被 XFF 伪造"类 finding 时先读此文件。
---

# IpSource::ClientIp / KeyGet::ClientIp — NOT-AN-ISSUE

## 结论

"未启用 RealIp 插件时，攻击者可通过伪造 `X-Forwarded-For` 绕过 IP 白名单/限流" 这类 finding 前提错误，判定为 NOT-AN-ISSUE。

## 代码路径（关键证据）

```
IpSource::ClientIp  → session.remote_addr() → ctx.request_info.remote_addr
KeyGet::ClientIp    → self.remote_addr()    → ctx.request_info.remote_addr
```

`remote_addr` 在 `src/core/gateway/routes/http/proxy_http/pg_request_filter.rs:261-266` 初始化：

```rust
ctx.request_info.remote_addr = if let Some(extractor) = &edgion_http.real_ip_extractor {
    extractor.extract_real_ip(&client_addr_str, req_header)
} else {
    // No extractor configured, use client_addr (already IP only)
    ctx.request_info.client_addr.clone()
};
```

**未配置 `real_ip_extractor` 时，`remote_addr` 直接等于 `client_addr`（TCP 连接 IP），完全不读取 XFF/X-Real-IP 请求头。**

## 为何 doc comment 看起来像"读 XFF"

`IpSource::ClientIp` 的原始 doc comment 写的是 "Extract from X-Forwarded-For (first IP) or fallback to X-Real-IP"，描述误导。
实际上，读 XFF/X-Real-IP 的工作由 **RealIp 插件**或**全局 `spec.realIp` 配置**完成，结果写入 `remote_addr`。
`IpSource::ClientIp` 只是**消费**已经处理过的 `remote_addr`，自身不解析任何 Header。

doc comment 已于 2026-04-22 修正（`ip_restriction.rs`）。

## 配置场景分析

| 场景 | `remote_addr` 内容 | XFF 是否可伪造 |
|------|------------------|---------------|
| 未配置 RealIp | TCP IP（`client_addr`） | ❌ 无效，XFF 不被读取 |
| 配置了 RealIp 插件 | RealIp 萃取结果 | 取决于 `trustedIps` 配置是否精确 |
| 配置了全局 `spec.realIp` | 全局 RealIp 萃取结果 | 同上 |

operator 需要"基于真实 client IP"时，正确做法是显式配置 RealIp，不是改插件默认行为。

## 不接受的修复建议

- "当 RealIp 未配置时自动回退到 TCP IP" — **已经是这个行为**，不需要额外修改
- "在文档里强制说明 ClientIp 必须与 RealIp 配合" — doc comment 已修正，文档无需重复强调错误前提
- "增加 RemoteAddr key 选项用于限流" — `RemoteAddr` 选项已存在（`IpSource::RemoteAddr`）

## `KeyGet::Header` 的情况

`KeyGet::Header` 依赖可伪造的请求头是**已知设计取舍**，与 Nginx/Envoy/Kong 限流行为一致。
Operator 选择 Header 作为限流 key 时，需自行评估上游可信度（如 CDN 固定注入的 `X-Real-IP`）。
这不属于安全漏洞，是配置权衡，文档有说明。
