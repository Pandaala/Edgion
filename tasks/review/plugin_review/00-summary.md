# Edgion Plugin System Review Summary

> Review date: 2026-03-30
> Scope: 全部插件机制 + 全部 28 个 HTTP 插件 + Stream 插件 + DSL 引擎 + 插件生命周期管理
> Reviewer: Claude Opus 4.6

## 统计

| 严重度 | 数量 |
|--------|------|
| Critical | 0 |
| Important | 21 |
| Minor | 27 |
| Observation | 16 |

## Important 问题总览

| ID | 分类 | 模块 | 问题 | 详情文件 |
|----|------|------|------|----------|
| SEC-01 | 安全 | ctx_set | 模板注入：替换值中的 `${}` 会被二次展开，攻击者可通过 header 泄露其他请求数据 | [01-security.md](01-security.md) |
| SEC-02 | 安全 | ip_restriction | IPv4-mapped IPv6 地址绕过 IPv4 规则 | [01-security.md](01-security.md) |
| SEC-03 | 安全 | stream ip_restriction | `validate_and_init` 失败后 fail-open，错误配置静默放行所有流量 | [01-security.md](01-security.md) |
| SEC-04 | 安全 | tls passthrough | TLS passthrough 模式未执行 Stage 2 插件，路由级策略被绕过 | [01-security.md](01-security.md) |
| SEC-05 | 安全 | tls proxy | IPv6 客户端地址被丢弃为 "unknown"，绕过所有 Stage 2 stream 插件检查 | [01-security.md](01-security.md) |
| SEC-06 | 安全 | dynamic_external_upstream | Domain 验证过弱，允许 null bytes、控制字符等 | [01-security.md](01-security.md) |
| SEC-07 | 安全 | rate_limit, rate_limit_redis | `reject_message` 直接插入 JSON 格式串，未转义导致 JSON 注入 | [01-security.md](01-security.md) |
| SEC-08 | 安全 | forward_auth | 认证服务响应体无大小限制，可导致内存耗尽 | [01-security.md](01-security.md) |
| SEC-09 | 安全 | key_auth | HashMap lookup 存在 timing 侧信道 | [01-security.md](01-security.md) |
| COR-01 | 正确性 | real_ip | `recursive` 配置字段被忽略，始终执行递归遍历 | [02-correctness.md](02-correctness.md) |
| COR-02 | 正确性 | real_ip | 双重实现路径，fallback 行为不一致 | [02-correctness.md](02-correctness.md) |
| COR-03 | 正确性 | real_ip | 所有 IP 都受信任时 fallback 逻辑不一致 | [02-correctness.md](02-correctness.md) |
| COR-04 | 正确性 | plugin_runtime | `UrlRewrite` variant 未在 `create_request_filter_from_edgion` 中处理，静默丢弃 | [02-correctness.md](02-correctness.md) |
| COR-05 | 正确性 | extension_ref | `ErrResponse` 在 sync upstream_response 阶段被吞掉 | [02-correctness.md](02-correctness.md) |
| COR-06 | 正确性 | header_cert_auth | `verify_depth` 已配置但未实施，无证书链验证 | [02-correctness.md](02-correctness.md) |
| COR-07 | 正确性 | jwt_auth | `std::sync::RwLock` 中毒导致永久认证失败 | [02-correctness.md](02-correctness.md) |
| COR-08 | 正确性 | mock | `delay` 字段无上限，可耗尽连接和 Tokio task | [02-correctness.md](02-correctness.md) |
| ARCH-01 | 架构 | plugin_runtime | `_ => None` 通配符屏蔽了新 variant 的编译时检查 | [03-architecture.md](03-architecture.md) |
| PERF-01 | 性能 | conditions | Regex fallback 路径每请求重新编译 | [04-validation-and-perf.md](04-validation-and-perf.md) |
| PERF-02 | 性能 | conditions | 用户正则无数量/长度限制（资源耗尽风险） | [04-validation-and-perf.md](04-validation-and-perf.md) |
| PERF-03 | 性能 | dsl | `regex_find`/`regex_replace` 每次调用编译正则，无缓存 | [04-validation-and-perf.md](04-validation-and-perf.md) |

## 详细报告

| 文件 | 内容 |
|------|------|
| [01-security.md](01-security.md) | 安全类问题（注入、绕过、信息泄露、资源耗尽） |
| [02-correctness.md](02-correctness.md) | 正确性问题（逻辑错误、未实现功能、静默失败） |
| [03-architecture.md](03-architecture.md) | 架构和设计问题 |
| [04-validation-and-perf.md](04-validation-and-perf.md) | 配置验证缺失 + 性能问题 |
| [05-minor-and-observations.md](05-minor-and-observations.md) | Minor 和 Observation 级别问题汇总 |

## 优先处理建议

### P0 — 立即修复
1. **SEC-01 (ctx_set 模板注入)** — 攻击者可通过 header 值泄露其他请求头数据
2. **SEC-02 + SEC-05 (IPv4-mapped IPv6 + IPv6 bypass)** — 双栈环境下 IP 限制完全失效
3. **SEC-03 + SEC-04 (stream fail-open + passthrough 缺失)** — 安全插件静默降级

### P1 — 尽快修复
4. **COR-01~03 (real_ip 三个问题)** — 用户面功能不符合预期
5. **COR-04 (UrlRewrite 静默丢弃)** — 已配置的插件不执行
6. **COR-05 (ErrResponse 被吞)** — ExtensionRef 内插件的错误响应丢失
7. **ARCH-01 (wildcard match)** — 新插件上线时容易遗漏

### P2 — 计划修复
8. **COR-06 (verify_depth)** — 配置字段未实施
9. **COR-07 (RwLock 中毒)** — 极端场景下永久认证失败
10. **SEC-06~09 + PERF-01~03** — 验证加固和性能优化
