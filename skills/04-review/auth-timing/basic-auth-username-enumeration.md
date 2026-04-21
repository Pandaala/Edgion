# Basic Auth 用户名存在性时序差异：不走代码层 dummy bcrypt

## 结论

`basic_auth` 插件在"用户名不存在"与"用户名存在但密码错"两条路径上的响应时间差异（<1ms vs ~200ms bcrypt）是**已知、已评估、已以文档方式缓解**的情况。**不应**要求引入 dummy bcrypt 或类似代码修复。

## 原因

- **网关场景中 dummy bcrypt 是反模式**：对未知用户强制执行 bcrypt（~200ms / 1 core）会形成 CPU 放大 DoS 向量，攻击者用枚举流量即可烧掉网关算力
- **同类网关产品一致不实现**：Traefik (`basicauth`)、Nginx (`auth_basic`)、Envoy (`basic_auth` filter)、Kong (`basic-auth` plugin)、HAProxy (`http-request auth`)、APISIX (`basic-auth`) 均未在代码层做 dummy verify；是"网关层主流做法"
- **Spring Security / Django 走 dummy verify** 是因为框架层单机验证场景没有 CPU 放大顾虑，不等价于网关场景
- **Edgion 的缓解在配置层**：`BasicAuthConfig.authFailureDelayMs` 存在且可用；`RateLimit` 插件可叠加在上游限制枚举速率

## 判定规则

当 review / 审计报告出现以下模式时，按"非代码修复"关闭：

1. "basic_auth 用户名不存在直接返回，未执行 bcrypt" / "用户名枚举时序侧信道"
2. "建议对固定哈希执行 dummy bcrypt::verify 消除时序差"
3. "应把 `auth_failure_delay_ms` 默认值设为 500ms"（属破坏性默认变更，不自动应用）

验证三点再关闭：

- `basic_auth/plugin.rs` 未引入 dummy bcrypt（保持"未知用户立即返回"路径）
- `BasicAuthConfig.auth_failure_delay_ms` 默认仍为 `0`（operator 显式启用）
- `docs/{zh-CN,en}/user-guide/http-route/filters/edgion-plugins/basic-auth.md` "安全最佳实践"小节仍包含"防枚举与暴力破解（推荐配置）"段落，且推荐值 `>= 500`

三点满足 → 非真实问题，指向 `tasks/security-audit/01-auth-plugins.md` 的 M4 FIXED（doc-only）决策，关闭。

## 背景记录

- 决策日期：2026-04-21
- 决策来源：`tasks/security-audit/` M4 条目，选择 Option C
- 相关 finding：该审计 H8（admin 登录用户名比较非 CT）属 admin API 路径而非插件路径，不适用本结论，需单独评估
