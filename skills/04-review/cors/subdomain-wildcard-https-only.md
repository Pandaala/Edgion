# CORS `*.domain` 简写仅匹配 HTTPS（已修复）

## 结论

`CorsConfig.allow_origins` 的 `*.example.com` 简写**仅匹配 `https://` origin**。这是 2026-04-21 修复 M13 后的明确语义——之前版本允许 `http://`，属 Edgion 自创便利语法糖的意外行为。**不应**把简写改回 `https?://` 以"兼容 http"——operator 若需 http 匹配应显式使用 `allow_origins_by_regex`。

## 原因

1. **`*.domain` 简写是 Edgion 自创的便利语法糖**，不是行业惯例（Traefik / Envoy / Kong 都让 operator 自写 regex）；既然是 Edgion 自己定义的语法糖，语义就应该符合 operator 心理模型
2. **Operator 直觉是 HTTPS**：几乎所有现代 CORS 生产配置针对 https 前端；写 `*.example.com` 时的默认期待是 https 范畴
3. **http scheme 允许扩大攻击面**：
   - 子域接管：未认领的子域（如 `dev-api.example.com` 指向已下线服务）被攻击者用 HTTP 服务填补 → 跨域读取 API 响应
   - 遗留 http 子域：企业内网常有测试/legacy 环境在 http 暴露，被 operator 无意识纳入 CORS 白名单
4. **显式 regex 作为逃生门**：需要 http 匹配的场景（本地 dev、遗留内部服务）仍可用 `allow_origins_by_regex: ["^https?://[^/]+\\.example\\.com$"]` 显式声明
5. **浏览器安全趋势**：`SameSite=None` 要求 `Secure`；mixed content 限制；COOP/COEP 需安全上下文——业界方向是 HTTPS-only CORS

## 判定规则

当 review / 审计报告出现以下模式时，按"已修复 + 设计意图"关闭：

1. "CORS `*.domain` 简写不支持 http，operator 配置失效"
2. "建议回退为 `https?://` 以保持向后兼容"
3. "应新增 `allowHttpSubdomainWildcard: bool` 配置"

验证三点再关闭：

- `cors.rs:167` 与 `cors.rs:180` 的正则仍是 `^https://[^/]+\.{}$`（非 `https?://`）
- `test_subdomain_wildcard` 测试仍 assert `http://test.example.com` **不**被允许
- `test_subdomain_wildcard_http_origins_via_regex` 仍存在，证明显式 regex 是已文档化的逃生门

三点满足 → 按设计意图关闭，指向 `tasks/security-audit/02-access-control.md` 的 M13 FIXED 决策。

## 例外场景（需重评）

- 如果浏览器 CORS 规范回退到鼓励 http（极不可能）→ 重评
- 如果 Edgion 主动添加 `allowHttpSubdomainWildcard` 配置字段 → 本规则需同步更新
- 如果出现 operator 大规模反馈"迁移成本不可接受" → 考虑加新配置字段 opt-in

## 背景记录

- 决策日期：2026-04-21
- 决策来源：`tasks/security-audit/02-access-control.md` M13 条目（Option A — 代码修复，破坏性）
- 本次 MEDIUM 审计中**第一个代码层破坏性修复**——与 M4/M8 doc-only 形成对照：
  - **doc-only 适用**：行业主流默认一致，改默认会与行业背离
  - **代码修复适用**：Edgion 自创语法糖的意外行为，不是行业惯例
- 其他同批次 review 判定：
  - `forward-auth/default-header-forwarding.md`（M7，NOT-AN-ISSUE，与 Traefik/Envoy 一致）
  - `oidc/nonce-pkce-defaults.md`（M8，doc-only，与主流网关一致）
  - 本文件（M13，代码修复，Edgion 独有语法糖）
